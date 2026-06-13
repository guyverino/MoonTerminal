//! Чарт в GPUI-оболочке (вариант A, доказан Лабой 2b): собственный wgpu-девайс
//! рендерит панели контейнера движком `moon_chart::paint::render_panes` в offscreen-
//! текстуру, та читается в CPU (readback) и отдаётся GPUI как `RenderImage`. Движок
//! переносится 1:1 — здесь только обвязка offscreen+readback вместо egui-wgpu surface.
//!
//! Размер offscreen ДИНАМИЧЕСКИЙ: подгоняется под реальный размер слота чарта в
//! девайс-пикселях (его меряет canvas-оверлей окна, см. main.rs). Текстура — точно
//! по размеру слота (без искажений), а вот readback требует `bytes_per_row`,
//! кратного 256 (wgpu COPY_BYTES_PER_ROW_ALIGNMENT): копируем в padded-буфер и при
//! разборке отрезаем хвост каждой строки. `ppp` = scale_factor окна → жёлоба осей
//! (PRICE_AXIS_W·ppp / TIME_AXIS_H·ppp) масштабируются как в egui-версии.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

use crate::axes::CrossStyle;
use moon_chart::axes::AxisSnapshot;
use moon_chart::container::{Container, ContainerKind, Mode};
use moon_chart::paint::{now_unix_ms, render_panes};
use moon_chart::view::Rect as ChartRect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::session::{CoreId, SessionManager};

/// Выравнивание строки readback-буфера (wgpu COPY_BYTES_PER_ROW_ALIGNMENT).
const ROW_ALIGN: u32 = 256;
/// Стартовый размер offscreen до первого замера слота (девайс-пиксели).
const DEFAULT_W: u32 = 1024;
const DEFAULT_H: u32 = 576;

/// Округляет `n` вверх до кратного `a`.
fn align_up(n: u32, a: u32) -> u32 {
    n.div_ceil(a) * a
}

// ── ВРЕМЕННАЯ ИНСТРУМЕНТАЦИЯ (диагностика фриза мультиокон) ──────────────────
/// Сколько раз `ChartPanel::render` отработал (инкремент в panels/chart.rs).
pub static DBG_RENDERS: AtomicU64 = AtomicU64::new(0);
/// Сколько readback'ов реально собрано (poll_image вернул картинку).
pub static DBG_READBACKS: AtomicU64 = AtomicU64::new(0);
/// Суммарно байт скопировано из staging при readback (объём через PCIe/CPU).
pub static DBG_READBACK_BYTES: AtomicU64 = AtomicU64::new(0);
/// Максимальная длительность одного poll_image (мкс) за окно лога — блокирует ли поток.
pub static DBG_POLL_MAX_US: AtomicU64 = AtomicU64::new(0);

/// ОДИН общий wgpu device/queue на ВСЕ чарты. Создание адаптера+девайса — сотни мс и
/// блокирует UI-поток; раньше каждый `ChartGpu` делал свой → фриз при появлении новой
/// вкладки (Main + каждый AddToChart). Device/Queue в wgpu не `Clone`, поэтому держим
/// под `Arc` и раздаём клоны Arc (дёшево). Создаётся лениво при первом чарте.
fn shared_gpu() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static GPU: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    let (d, q) = GPU.get_or_init(|| {
        // ВАЖНО (перф): пустые флаги — иначе в DEBUG wgpu включает DX12 validation layer
        // (десятки раз медленнее каждый GPU-вызов, + `HRESULT 0x887A002D` без Graphics Tools).
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            flags: wgpu::InstanceFlags::empty(),
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .expect("wgpu adapter");
        let info = adapter.get_info();
        log::info!("chart wgpu backend: {:?} | adapter: {} | driver: {}", info.backend, info.name, info.driver);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .expect("wgpu device");
        (Arc::new(device), Arc::new(queue))
    });
    (d.clone(), q.clone())
}

pub struct ChartGpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    staging: wgpu::Buffer,
    /// Текущий размер offscreen (девайс-пиксели) и выровненный шаг строки readback.
    w: u32,
    h: u32,
    padded_bpr: u32,
    pub container: Container,
    epoch: f64,
    theme: ChartTheme,
    orders: OrdersStyle,
    /// Применённый масштаб цены (Y): None = «Авто». Совпадает с дефолтом контейнера.
    scale: Option<f32>,
    /// Применённый live-follow (вид бежит за «сейчас»). Совпадает с дефолтом панелей.
    follow: bool,
    /// Readback в полёте: кадр отправлен в GPU, ждём завершения `map_async` (НЕ
    /// блокируя UI-поток). Готовность сигналит `map_done` (ставится в колбэке при poll).
    pending: bool,
    map_done: Arc<AtomicBool>,
    /// Раскладка панелей отправленного кадра — отдаётся вводу, когда картинка готова.
    pending_layout: Vec<(usize, ChartRect)>,
}

impl ChartGpu {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        Self::new_kind(epoch, theme, ContainerKind::Main)
    }

    /// ChartGpu с заданным видом контейнера (Main / Chart{num} для AddToChart-вкладок).
    /// Девайс/очередь — ОБЩИЕ (см. `shared_gpu`): новый чарт не создаёт свой девайс,
    /// поэтому появление вкладки больше не фризит UI-поток.
    pub fn new_kind(epoch: f64, theme: ChartTheme, kind: ContainerKind) -> Self {
        let (device, queue) = shared_gpu();

        // BGRA sRGB — как surface egui-версии: байты сразу в порядке, который GPUI
        // ждёт от RenderImage (BGRA), иначе каналы R↔B свопаются. + sRGB-таргет.
        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let (texture, view, staging, padded_bpr) =
            Self::alloc(&device, format, DEFAULT_W, DEFAULT_H);

        Self {
            device,
            queue,
            format,
            texture,
            view,
            staging,
            w: DEFAULT_W,
            h: DEFAULT_H,
            padded_bpr,
            container: Container::new(kind),
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            pending: false,
            map_done: Arc::new(AtomicBool::new(false)),
            pending_layout: Vec::new(),
        }
    }

    /// AddToChart: добавить монету авто-панелью (Tiled-мультичарт) с TTL (KeepInChart).
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container
            .push_auto(core, market, now_ms, ttl_ms, &self.device, self.format, self.epoch);
    }

    /// Убрать истёкшие AddToChart-панели. True — если что-то удалили (→ пере-рендер).
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        self.container.prune_ttl(now_ms)
    }

    /// Сигнатура рыночных данных всех панелей (ticks_rev+book_rev по (ядро,рынок)).
    /// Сменилась → пришли новые данные, нужен пере-рендер; не сменилась → кадр можно
    /// пропустить. offscreen-readback дорог (блокирует UI-поток), поэтому НЕ гоняем его
    /// на холостом ходу — иначе дёрганье при перетаскивании окна и лаг dock-вкладок.
    pub fn data_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        for p in &self.container.panes {
            if let Some(v) = session.market_view(p.core, &p.market) {
                sig = sig.wrapping_mul(31).wrapping_add(v.ticks_rev).wrapping_add(v.book_rev);
            }
        }
        sig
    }

    /// Есть ли TTL-панели (нужно гонять кадры под их истечение).
    #[allow(dead_code)]
    pub fn has_ttl_panes(&self) -> bool {
        self.container.has_ttl_panes()
    }

    /// Создаёт offscreen-текстуру `w×h`, её view и staging-буфер под readback с
    /// выровненным шагом строки. Возвращает (texture, view, staging, padded_bpr).
    fn alloc(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        w: u32,
        h: u32,
    ) -> (wgpu::Texture, wgpu::TextureView, wgpu::Buffer, u32) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpui-chart-offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_bpr = align_up(w * 4, ROW_ALIGN);
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpui-chart-staging"),
            size: (padded_bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        (texture, view, staging, padded_bpr)
    }

    /// Подгоняет offscreen под размер слота (девайс-пиксели). Пересоздаёт ресурсы
    /// только при реальной смене размера. Размер клампится к ≥1.
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if w == self.w && h == self.h {
            return;
        }
        let (texture, view, staging, padded_bpr) = Self::alloc(&self.device, self.format, w, h);
        self.texture = texture;
        self.view = view;
        self.staging = staging;
        self.padded_bpr = padded_bpr;
        self.w = w;
        self.h = h;
        // Staging пересоздан → in-flight readback больше не валиден, абортируем его
        // (старый колбэк сработает на старом Arc и будет проигнорирован).
        self.pending = false;
    }

    /// Открыть монету (фулскрин-панель) — 1:1 как egui host.
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container
            .open_manual(core, market, &self.device, self.format, self.epoch);
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.container.is_empty()
    }

    /// Рынок активной (фулскрин) панели — для подписи вкладки чарта.
    pub fn active_market(&self) -> Option<String> {
        let idx = match self.container.mode {
            Mode::Fullscreen(i) => i,
            Mode::Tiled => 0,
        };
        self.container.panes.get(idx).map(|p| p.market.clone())
    }

    /// Снимок вида активной (фулскрин) панели для шкал. None — пустой контейнер.
    /// Значения текущего кадра — звать ПОСЛЕ `render`.
    #[allow(dead_code)]
    pub fn axis_snapshot(&self, tz_offset_sec: i64) -> Option<AxisSnapshot> {
        let idx = match self.container.mode {
            Mode::Fullscreen(i) => i,
            Mode::Tiled => 0,
        };
        let v = &self.container.panes.get(idx)?.chart.view;
        Some(AxisSnapshot {
            px_per_ms: v.px_per_ms,
            right_margin_frac: v.right_margin_frac,
            render_center: v.render_center,
            render_range: v.render_range,
            epoch_ms: v.epoch_ms,
            right_time_ms: v.right_time_ms,
            tz_offset_sec,
        })
    }

    /// Снимки осей ПО ВИДИМЫМ ПАНЕЛЯМ: (индекс, прямоугольник девайс-px, снимок).
    /// Для Tiled-мультичарта — по снимку на каждую полосу, чтобы оси были привязаны
    /// к своему графику (не один общий). Звать ПОСЛЕ render.
    pub fn axis_panes(&self, tz_offset_sec: i64) -> Vec<(usize, ChartRect, AxisSnapshot)> {
        let area = ChartRect { x: 0.0, y: 0.0, w: self.w as f32, h: self.h as f32 };
        self.container
            .layout(area)
            .into_iter()
            .filter_map(|(idx, rect)| {
                let v = &self.container.panes.get(idx)?.chart.view;
                Some((
                    idx,
                    rect,
                    AxisSnapshot {
                        px_per_ms: v.px_per_ms,
                        right_margin_frac: v.right_margin_frac,
                        render_center: v.render_center,
                        render_range: v.render_range,
                        epoch_ms: v.epoch_ms,
                        right_time_ms: v.right_time_ms,
                        tz_offset_sec,
                    },
                ))
            })
            .collect()
    }

    /// Применить тему (из настроек). Возвращает true, если изменилась (→ пере-рендер).
    pub fn set_theme(&mut self, theme: ChartTheme) -> bool {
        if self.theme != theme {
            self.theme = theme;
            true
        } else {
            false
        }
    }

    /// Применить стиль ордер-линий. Возвращает true, если изменился.
    pub fn set_orders(&mut self, orders: OrdersStyle) -> bool {
        if self.orders != orders {
            self.orders = orders;
            true
        } else {
            false
        }
    }

    /// Применить масштаб цены (Y) ко ВСЕМ панелям контейнера (порт тулбара egui).
    /// `None` = «Авто». Запоминается в контейнере → новые графики откроются с ним же.
    /// Возвращает true, если изменился (→ пере-рендер).
    pub fn set_scale(&mut self, pct: Option<f32>) -> bool {
        if self.scale == pct {
            return false;
        }
        self.scale = pct;
        self.container.set_scale(pct);
        true
    }

    /// Применить live-follow ко ВСЕМ панелям: `true` = к «сейчас» (resume_live), `false` =
    /// заморозить вид. Порт кнопки Live/Пауза тулбара. Возвращает true, если изменился.
    pub fn set_follow(&mut self, follow: bool, now_ms: f64) -> bool {
        if self.follow == follow {
            return false;
        }
        self.follow = follow;
        for p in &mut self.container.panes {
            if follow {
                p.chart.view.resume_live(now_ms);
            } else {
                p.chart.view.follow = false;
            }
        }
        true
    }

    /// Стиль перекрестия из темы (рисуем крест GPUI-оверлеем, не движком).
    pub fn crosshair_style(&self) -> CrossStyle {
        CrossStyle {
            color: self.theme.cross,
            alpha: self.theme.cross_alpha,
            thickness: self.theme.cross_thickness,
            halo_radius: self.theme.halo_radius,
            halo_intensity: self.theme.halo_intensity,
        }
    }

    /// Идёт ли readback (кадр отправлен, картинка ещё не забрана).
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// Отправить кадр в GPU (движок → offscreen → копия в staging) и начать readback
    /// БЕЗ блокировки UI-потока (`map_async`, без `poll(Wait)`). Готовность заберёт
    /// [`poll_image`]. Не отправляет, пока предыдущий readback не забран. `ppp` =
    /// scale_factor окна (масштаб жёлобов осей, как в egui).
    pub fn submit(&mut self, session: &SessionManager, ppp: f32) {
        if self.pending {
            return;
        }
        let (w, h) = (self.w, self.h);
        let area = ChartRect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gpui-chart") });

        // Движок 1:1: рисует панели контейнера в наш offscreen-view (без перекрестия).
        let layout = render_panes(
            &mut self.container,
            &self.device,
            &self.queue,
            &mut enc,
            &self.view,
            area,
            [w as f32, h as f32],
            ppp,
            now_unix_ms(),
            &self.theme,
            &self.orders,
            None,
            None,
            session,
        );

        enc.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.staging,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));

        // Свежий флаг на этот readback (старый абортированный колбэк его не тронет).
        let flag = Arc::new(AtomicBool::new(false));
        let cb = flag.clone();
        self.staging.slice(..).map_async(wgpu::MapMode::Read, move |res| {
            if res.is_ok() {
                cb.store(true, Ordering::Release);
            }
        });
        self.map_done = flag;
        self.pending = true;
        self.pending_layout = layout;
    }

    /// Неблокирующе продвинуть mapping (`poll(Poll)`); когда готово — собрать картинку
    /// и вернуть (с раскладкой панелей). None — readback не запущен или ещё не готов.
    pub fn poll_image(&mut self) -> Option<(Arc<RenderImage>, Vec<(usize, ChartRect)>)> {
        if !self.pending {
            return None;
        }
        let _ = self.device.poll(wgpu::Maintain::Poll);
        if !self.map_done.load(Ordering::Acquire) {
            return None;
        }
        let t0 = Instant::now();
        let (w, h) = (self.w, self.h);
        let slice = self.staging.slice(..);
        let data = slice.get_mapped_range();

        // Отрезаем хвост-паддинг каждой строки: padded_bpr → ровно w*4 байт.
        let row = (w * 4) as usize;
        let bpr = self.padded_bpr as usize;
        let mut pixels = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            let off = y * bpr;
            pixels.extend_from_slice(&data[off..off + row]);
        }
        drop(data);
        self.staging.unmap();
        self.pending = false;
        let layout = std::mem::take(&mut self.pending_layout);
        DBG_READBACKS.fetch_add(1, Ordering::Relaxed);
        DBG_READBACK_BYTES.fetch_add((row * h as usize) as u64, Ordering::Relaxed);

        let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, pixels).expect("chart buf");
        let img = Arc::new(RenderImage::new(vec![Frame::new(buf)]));
        let us = t0.elapsed().as_micros() as u64;
        DBG_POLL_MAX_US.fetch_max(us, Ordering::Relaxed);
        Some((img, layout))
    }
}
