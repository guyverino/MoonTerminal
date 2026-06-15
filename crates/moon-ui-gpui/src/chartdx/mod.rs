//! Own-pass DX11 рендер чарта (замена wgpu-offscreen+readback). Слои по природе данных
//! (см. `docs/RENDER_PLAN.md`): Combo (рыночная история) / OrderBook (срез) /
//! UserData (мутирующее юзерское) + хром (Grid/Background) + текст/курсор поверх в GPUI.
//!
//! Доменная специфика чарта живёт ЗДЕСЬ (в терминале); форк gpui отдаёт только generic-хук
//! `RawGpuAccess`. Файл на слой; здесь — оркестратор `ChartEngine`: prepare данных per pane
//! (БЕЗ рисования) + регистрация own-pass, который и рисует в кадре GPUI.

mod backend;
#[cfg(windows)]
pub mod background;
#[cfg(windows)]
pub mod combo;
#[cfg(windows)]
pub mod gpu;
#[cfg(windows)]
pub mod grid;
#[cfg(target_os = "macos")]
mod metal_backend;
#[cfg(windows)]
pub mod orderbook;
pub mod pane;
pub mod types;
#[cfg(windows)]
pub mod userdata;
pub mod view;
#[cfg(target_os = "linux")]
mod wgpu_backend;

use std::cell::RefCell;
#[cfg(windows)]
use std::ffi::c_void;
use std::rc::Rc;

use gpui::{GpuBackend, GpuPhase, RawGpuAccess, Subscription, Window};
use moon_chart::axes::AxisSnapshot;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::Rect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::session::{CoreId, SessionManager};
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::ID3D11RasterizerState;

use crate::axes::CrossStyle;
use backend::PlatformLayers;
use pane::{Container, ContainerKind, Mode};
use types::{BackgroundParams, BookStyle, ChartViewGpu, GridParams, cover_uv};

const CHART_PHOTO_BACKGROUND_ENABLED: bool = false;

/// sRGB [u8;3] → [f32;4] (alpha 1) для cbuffer-цветов (шейдер переводит в linear).
fn rgb4(c: [u8; 3]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        1.0,
    ]
}

fn union_range(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> Option<(f32, f32)> {
    match (a, b) {
        (Some((alo, ahi)), Some((blo, bhi))) => Some((alo.min(blo), ahi.max(bhi))),
        (Some(r), None) | (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

/// GPU-состояние одной панели для own-pass callback — отделено от логики `Container`,
/// синхронизируется по индексу + идентичности (core, market) в `prepare`.
struct PaneRender {
    core: Option<CoreId>,
    market: String,
    view: ChartViewGpu,
    layers: PlatformLayers,
    background_params: BackgroundParams,
    grid_params: GridParams,
    orderbook_view: ChartViewGpu,
    book_style: BookStyle,
    /// Сколько тиков уже залито в кольцо combo + значение `dropped` (для append/reset).
    last_len: usize,
    last_dropped: u64,
    last_price_lines_rev: u64,
    /// Последнее виденное поколение device combo: сменилось (device-lost) → перезалить историю.
    last_device_gen: u64,
    /// Последняя сборка стакана: ревизия данных + видимое ценовое окно.
    last_book_rev: u64,
    last_book_lo: f32,
    last_book_hi: f32,
    /// Последняя ревизия ордеров, по которой залит userdata-буфер.
    last_orders_rev: u64,
    /// Видима в этом кадре (рисуем) — ставится в `prepare`.
    active: bool,
}

impl PaneRender {
    fn new() -> Self {
        Self {
            core: None,
            market: String::new(),
            view: ChartViewGpu::default(),
            layers: PlatformLayers::new(),
            background_params: BackgroundParams::default(),
            grid_params: GridParams::default(),
            orderbook_view: ChartViewGpu::default(),
            book_style: BookStyle::default(),
            last_len: 0,
            last_dropped: u64::MAX,
            last_price_lines_rev: u64::MAX,
            last_device_gen: 0,
            last_book_rev: u64::MAX,
            last_book_lo: f32::NAN,
            last_book_hi: f32::NAN,
            last_orders_rev: u64::MAX,
            active: false,
        }
    }
}

/// Состояние рендера всех панелей — шарится с own-pass callback'ом (`Rc<RefCell>`,
/// единственный поток UI: `prepare` и callback кадра не пересекаются по времени).
struct RenderState {
    panes: Vec<PaneRender>,
    /// Scissor-растеризатор own-pass (lazy, пересоздаётся на смене device): клипует слои к
    /// зоне панели, чтобы стакан/ордера (позиционируются по ЦЕНЕ) не лезли за плот на тулбар/шкалы.
    #[cfg(windows)]
    scissor_rs: Option<ID3D11RasterizerState>,
    #[cfg(windows)]
    scissor_dev: *mut c_void,
}

pub struct ChartEngine {
    pub container: Container,
    state: Rc<RefCell<RenderState>>,
    epoch: f64,
    theme: ChartTheme,
    orders: OrdersStyle,
    scale: Option<f32>,
    follow: bool,
    present_rate_hz: f32,
    /// Размер слота чарта (девайс-px) — меряется canvas-оверлеем окна.
    w: u32,
    h: u32,
    /// Левый-верхний угол слота чарта В ОКНЕ (девайс-px). own-pass рисует в backbuffer ОКНА,
    /// поэтому координаты слоёв = origin слота + локальные, а cv_resolution = размер backbuffer.
    origin: (f32, f32),
    pass_registration_attempted: bool,
    pass_subscription: Option<Subscription>,
}

impl ChartEngine {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        Self::new_kind(epoch, theme, ContainerKind::Main)
    }

    pub fn new_kind(epoch: f64, theme: ChartTheme, kind: ContainerKind) -> Self {
        Self {
            container: Container::new(kind),
            state: Rc::new(RefCell::new(RenderState {
                panes: Vec::new(),
                #[cfg(windows)]
                scissor_rs: None,
                #[cfg(windows)]
                scissor_dev: std::ptr::null_mut(),
            })),
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            present_rate_hz: 60.0,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
            pass_registration_attempted: false,
            pass_subscription: None,
        }
    }

    /// Регистрирует own-pass ОДИН раз: callback рисует все активные панели
    /// (combo + слои) их own-pass в backbuffer GPUI ПОД сценой. Зовётся из `Render` (есть окно).
    pub fn register_pass(&mut self, window: &mut Window) {
        if self.pass_registration_attempted {
            return;
        }
        let state = self.state.clone();
        // UnderScene — правильный финальный слой: GPUI-хром, попапы, меню и тултипы должны
        // быть поверх графика. Chart host/content в MoonPalette держатся на NoFill, обычные
        // панели — Opaque, поэтому фоновые quads не перекрывают plot area.
        let pass = window.add_gpu_pass(
            GpuPhase::UnderScene,
            Box::new(move |gpu: &RawGpuAccess| {
                let mut st = state.borrow_mut();
                match gpu.backend {
                    #[cfg(windows)]
                    GpuBackend::D3D11 => {
                        let Some((device, context, rtv)) = gpu::borrow_d3d(gpu) else {
                            return Ok(());
                        };
                        // Scissor own-pass (lazy + device-lost guard). GPUI рисует сцену с ScissorEnable=false,
                        // а наши слои стакана/ордеров позиционируются по ЦЕНЕ и эмитят уровни ВНЕ видимого окна
                        // (build_instances отдаёт всю книгу) → без обрезки бары уезжают за плот, на тулбар/шкалы.
                        // Ставим свой scissor-стейт, в конце возвращаем стейт GPUI (иначе следующий кадр сцена
                        // GPUI унаследует наш scissor и обрежет UI).
                        if st.scissor_dev != gpu.device {
                            st.scissor_rs = Some(gpu::create_scissor_rasterizer(&device));
                            st.scissor_dev = gpu.device;
                        }
                        let scissor_rs = st.scissor_rs.clone().unwrap();
                        let prev_rs = unsafe { context.RSGetState().ok() };
                        for pr in &mut st.panes {
                            if pr.active {
                                // cv_resolution = размер backbuffer окна (own-pass пишет в него напрямую).
                                let res = [gpu.width as f32, gpu.height as f32];
                                pr.view.resolution = res;
                                pr.grid_params.resolution = res;
                                pr.orderbook_view.resolution = res;
                                // Обрезка к зоне панели = плот (chart_area) + стакан (glass). Жёлоб цены слева
                                // и шкала времени снизу — ВНЕ scissor, подписи GPUI там выживают.
                                let panel_clip = [
                                    pr.view.bounds[0],
                                    pr.view.bounds[1],
                                    pr.orderbook_view.bounds[0] + pr.orderbook_view.bounds[2],
                                    pr.view.bounds[1] + pr.view.bounds[3],
                                ];
                                gpu::set_scissor(
                                    &context,
                                    &scissor_rs,
                                    panel_clip[0],
                                    panel_clip[1],
                                    panel_clip[2],
                                    panel_clip[3],
                                );
                                pr.layers.render_d3d(
                                    &pr.view,
                                    &pr.background_params,
                                    &pr.grid_params,
                                    &pr.orderbook_view,
                                    &pr.book_style,
                                    &device,
                                    &context,
                                    &rtv,
                                    gpu,
                                    panel_clip,
                                );
                            }
                        }
                        // Вернуть растеризатор GPUI (scissor off).
                        unsafe {
                            context.RSSetState(prev_rs.as_ref());
                        }
                        Ok(())
                    }
                    #[cfg(target_os = "linux")]
                    GpuBackend::Wgpu => {
                        for pr in &mut st.panes {
                            if pr.active {
                                let res = [gpu.width as f32, gpu.height as f32];
                                pr.view.resolution = res;
                                pr.grid_params.resolution = res;
                                pr.orderbook_view.resolution = res;
                                pr.layers.render_wgpu(
                                    &pr.view,
                                    &pr.background_params,
                                    &pr.grid_params,
                                    &pr.orderbook_view,
                                    &pr.book_style,
                                    gpu,
                                )?;
                            }
                        }
                        Ok(())
                    }
                    #[cfg(target_os = "macos")]
                    GpuBackend::Metal => {
                        for pr in &mut st.panes {
                            if pr.active {
                                let res = [gpu.width as f32, gpu.height as f32];
                                pr.view.resolution = res;
                                pr.grid_params.resolution = res;
                                pr.orderbook_view.resolution = res;
                                pr.layers.render_metal(
                                    &pr.view,
                                    &pr.background_params,
                                    &pr.grid_params,
                                    &pr.orderbook_view,
                                    &pr.book_style,
                                    gpu,
                                )?;
                            }
                        }
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }),
        );
        match pass {
            Ok(subscription) => {
                self.pass_subscription = Some(subscription);
            }
            Err(err) => {
                log::warn!("chart own-pass registration failed: {err:#}");
            }
        }
        self.pass_registration_attempted = true;
    }

    /// Размер слота чарта (девайс-px). Combo сам пересоздаёт битмап при смене размера.
    pub fn resize(&mut self, w: u32, h: u32) {
        self.w = w.max(1);
        self.h = h.max(1);
    }

    /// Левый-верхний угол слота чарта В ОКНЕ (девайс-px) — для координат own-pass в backbuffer.
    pub fn set_origin(&mut self, x: f32, y: f32) {
        self.origin = (x, y);
    }

    pub fn set_present_rate_hz(&mut self, hz: f32) {
        self.present_rate_hz = hz.max(1.0);
    }

    /// ПОДГОТОВКА кадра (вместо wgpu submit+readback): обновляет вид и данные слоёв каждой
    /// видимой панели. НЕ рисует — рисование в own-pass callback (`register_pass`). Дёшево:
    /// математика вида + конверт новых тиков; тяжёлое (bake/blit) — на GPU в callback.
    pub fn prepare(&mut self, session: &SessionManager, ppp: f32) {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.layout(area);
        let now = now_unix_ms();
        let res = [self.w as f32, self.h as f32];
        let mut st = self.state.borrow_mut();
        st.panes
            .resize_with(self.container.panes.len(), PaneRender::new);
        for pr in &mut st.panes {
            pr.active = false;
        }
        for (idx, rect) in &layout {
            let pane = &mut self.container.panes[*idx];
            let pr = &mut st.panes[*idx];
            // sync идентичности: панель на этом индексе сменила монету → сбросить GPU-состояние.
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
            }
            // Жёлоба шкал (физ. px): слева цена, снизу время. Подписи рисует GPUI (axes::draw);
            // own-pass рисует ВНУТРИ chart_area. Зона стакана справа добавится с OrderBook-слоем.
            let price_axis_w = moon_chart::PRICE_AXIS_W * ppp;
            let time_axis_h = moon_chart::TIME_AXIS_H * ppp;
            let plot_h = (rect.h - time_axis_h).max(1.0);
            // Три зоны над нижней шкалой: жёлоб цены | график | стакан (справа).
            let glass_w = moon_chart::GLASS_ZONE_PX.min(rect.w * 0.5);
            let chart_area = Rect {
                x: rect.x + price_axis_w,
                y: rect.y,
                w: (rect.w - price_axis_w - glass_w).max(1.0),
                h: plot_h,
            };
            let glass_area = Rect {
                x: rect.x + (rect.w - glass_w).max(1.0),
                y: rect.y,
                w: glass_w,
                h: plot_h,
            };
            // Математика вида (общая с эталоном): X-окно → видимый срез тиков → авто-Y по нему.
            pane.view
                .ensure_default_window(chart_area.w, self.present_rate_hz);
            pane.view.follow_edge(now, now);
            let (view_time0, window_ms) = pane.view.visible_x(chart_area.w);
            let data = session.market_view(pane.core, &pane.market);
            let (cstart, ccount) = match data {
                Some(d) => {
                    let margin = pane.view.marker_half_px / pane.view.px_per_ms.max(1e-6);
                    d.ring
                        .visible_range(view_time0 - margin, view_time0 + window_ms + margin)
                }
                None => (0, 0),
            };
            let tick_price = data.and_then(|d| d.ring.price_range_in(cstart, ccount));
            let order_price = session
                .store()
                .core(pane.core)
                .and_then(|core_st| core_st.order_lines.buy_sell_range(&pane.market));
            let visible_price = union_range(tick_price, order_price);
            let last_price = data.and_then(|d| d.last_price);
            pane.view.update_y(now, plot_h, visible_price, last_price);
            // own-pass рисует в backbuffer ОКНА → bounds в координатах окна (origin слота +
            // локальные). resolution тут placeholder — реальный backbuffer ставит callback.
            let area_win = Rect {
                x: self.origin.0 + chart_area.x,
                y: self.origin.1 + chart_area.y,
                w: chart_area.w,
                h: chart_area.h,
            };
            pr.view = view::view_gpu(&pane.view, area_win, res);
            let (bg_uv_off, bg_uv_scale) = cover_uv(chart_area.w, chart_area.h, 1.0);
            let background_opacity = if CHART_PHOTO_BACKGROUND_ENABLED {
                self.theme.background_opacity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            pr.background_params = BackgroundParams {
                dst: pr.view.bounds,
                resolution: res,
                uv_off: bg_uv_off,
                uv_scale: bg_uv_scale,
                opacity: background_opacity,
                _pad: 0.0,
                bg: rgb4(self.theme.bg),
            };
            // Сетка: СТАТИЧНЫЕ вертикали (6 делений, как подписи времени) + горизонтали по цене
            // (шаг = nice_interval, совпадает с подписями цены). resolution ставит callback.
            pr.grid_params = GridParams {
                bounds: pr.view.bounds,
                resolution: res,
                n_vert: 6.0,
                price_to_px: pr.view.price_to_px,
                view_price0: pr.view.view_price0,
                price_interval: moon_chart::axes::nice_interval(
                    pane.view.render_range.max(1e-9),
                    8.0,
                ),
                grid_alpha: self.theme.grid_alpha,
                bg_alpha: if background_opacity > 0.0 { 0.0 } else { 1.0 },
                bg: rgb4(self.theme.bg),
                grid_col: rgb4(self.theme.grid),
            };
            // Стакан: своя зона справа. Та же ценовая шкала (price_to_px/view_price0), но
            // viewport = glass_area. Уровни нормируются по видимому ценовому окну render_center±range/2.
            let glass_win = Rect {
                x: self.origin.0 + glass_area.x,
                y: self.origin.1 + glass_area.y,
                w: glass_area.w,
                h: glass_area.h,
            };
            pr.orderbook_view = view::view_gpu(&pane.view, glass_win, res);
            pr.book_style = BookStyle {
                book_bg: rgb4(self.theme.book_bg),
                bid: rgb4(self.theme.book_bid),
                ask: rgb4(self.theme.book_ask),
            };
            if let Some(d) = data {
                let half = pane.view.render_range.max(1e-9) * 0.5;
                let (lo, hi) = (
                    pane.view.render_center - half,
                    pane.view.render_center + half,
                );
                if pr.last_book_rev != d.book_rev || pr.last_book_lo != lo || pr.last_book_hi != hi
                {
                    let mut levels = Vec::new();
                    d.book.build_instances(lo, hi, &mut levels);
                    pr.layers.set_orderbook(levels);
                    pr.last_book_rev = d.book_rev;
                    pr.last_book_lo = lo;
                    pr.last_book_hi = hi;
                }
            } else if pr.last_book_rev != u64::MAX {
                pr.layers.set_orderbook(Vec::new());
                pr.last_book_rev = u64::MAX;
                pr.last_book_lo = f32::NAN;
                pr.last_book_hi = f32::NAN;
            }
            // Ордера юзера (UserData): геометрия лестниц/линий/маркеров из ретейн-стора ядра.
            // Активные линии тянутся до правого края plot (через стакан) → edge_rel. Координаты
            // логические (time_rel/price), трансформ в шейдере — тот же chart_area (pr.view).
            let edge_rel = view_time0 + (chart_area.w + glass_w) / pane.view.px_per_ms.max(1e-6);
            pr.view.pad = edge_rel;
            if let Some(core_st) = session.store().core(pane.core) {
                if pr.last_orders_rev != core_st.orders_rev {
                    let mut hlines = Vec::new();
                    let mut segs = Vec::new();
                    let mut markers = Vec::new();
                    let mut zones = Vec::new();
                    moon_chart::build_order_geometry(
                        &core_st.order_lines,
                        &pane.market,
                        &self.orders,
                        pane.view.epoch_ms,
                        now,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.0,
                        &mut zones,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    pr.layers.set_userdata(&zones, &hlines, &segs, &markers);
                    pr.last_orders_rev = core_st.orders_rev;
                }
            } else if pr.last_orders_rev != u64::MAX {
                pr.layers.set_userdata(&[], &[], &[], &[]);
                pr.last_orders_rev = u64::MAX;
            }
            // Trades в combo: полный reset при съезде индексов (drain) ИЛИ device-lost (GPUI
            // пересоздал device → кольцо combo пустое, append живого края не восстановит историю);
            // иначе append живого края. device_gen combo инкрементится в его render при смене device.
            if let Some(d) = data {
                if pr.last_price_lines_rev != d.price_lines_rev {
                    pr.layers
                        .set_price_lines(d.last_line.points(), d.mark_line.points());
                    pr.last_price_lines_rev = d.price_lines_rev;
                }
                let cur_len = d.ring.len();
                let cur_dropped = d.ring.dropped();
                let cur_gen = pr.layers.device_gen();
                if pr.last_dropped != cur_dropped || pr.last_device_gen != cur_gen {
                    pr.layers.reset_combo(view::collect_all(&d.ring));
                    pr.layers
                        .set_price_lines(d.last_line.points(), d.mark_line.points());
                    pr.last_len = cur_len;
                    pr.last_dropped = cur_dropped;
                    pr.last_price_lines_rev = d.price_lines_rev;
                    pr.last_device_gen = cur_gen;
                } else if cur_len > pr.last_len {
                    pr.layers
                        .append_combo(&view::collect_range(&d.ring, pr.last_len, cur_len));
                    pr.last_len = cur_len;
                }
            } else if pr.last_price_lines_rev != u64::MAX {
                pr.layers.set_price_lines(&[], &[]);
                pr.last_price_lines_rev = u64::MAX;
            }
            pr.active = true;
        }
    }

    // ── Настройки (порт из старого chart.rs::ChartGpu) ───────────────────────────

    pub fn set_theme(&mut self, theme: ChartTheme) -> bool {
        if self.theme != theme {
            self.theme = theme;
            true
        } else {
            false
        }
    }

    pub fn set_orders(&mut self, orders: OrdersStyle) -> bool {
        if self.orders != orders {
            self.orders = orders;
            for pr in &mut self.state.borrow_mut().panes {
                pr.last_orders_rev = u64::MAX;
            }
            true
        } else {
            false
        }
    }

    /// Масштаб цены (Y) ко ВСЕМ панелям. None=Авто. Запоминается в контейнере.
    pub fn set_scale(&mut self, pct: Option<f32>) -> bool {
        if self.scale == pct {
            return false;
        }
        self.scale = pct;
        self.container.set_scale(pct);
        true
    }

    /// Live-follow ко ВСЕМ панелям: true = к «сейчас» (resume_live), false = заморозить.
    pub fn set_follow(&mut self, follow: bool, now_ms: f64) -> bool {
        let panes_match = self.container.panes.iter().all(|p| p.view.follow == follow);
        if self.follow == follow && panes_match {
            return false;
        }
        self.follow = follow;
        for p in &mut self.container.panes {
            if follow {
                p.view.resume_live(now_ms);
                p.view.reset_default_window_on_next_prepare();
            } else {
                p.view.follow = false;
            }
        }
        true
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    pub fn sync_follow_from_views(&mut self) -> bool {
        let follow = if self.container.panes.is_empty() {
            self.follow
        } else {
            self.container.panes.iter().all(|p| p.view.follow)
        };
        if self.follow == follow {
            false
        } else {
            self.follow = follow;
            true
        }
    }

    /// Открыть монету (фулскрин-панель).
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container.open_manual(core, market, self.epoch);
    }

    /// AddToChart: добавить монету авто-панелью (Tiled) с TTL.
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container
            .push_auto(core, market, now_ms, ttl_ms, self.epoch);
    }

    /// Убрать истёкшие AddToChart-панели. True — если что-то удалили.
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        self.container.prune_ttl(now_ms)
    }

    #[allow(dead_code)]
    pub fn has_ttl_panes(&self) -> bool {
        self.container.has_ttl_panes()
    }

    /// Сигнатура данных (ticks_rev+book_rev+orders_rev по всем панелям) — для гейта пере-рендера.
    pub fn data_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        for p in &self.container.panes {
            if let Some(v) = session.market_view(p.core, &p.market) {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(v.ticks_rev)
                    .wrapping_add(v.price_lines_rev)
                    .wrapping_add(v.book_rev);
            }
            if let Some(core_st) = session.store().core(p.core) {
                sig = sig.wrapping_mul(31).wrapping_add(core_st.orders_rev);
            }
        }
        sig
    }

    /// Рынок активной (фулскрин/первой) панели — для подписи вкладки.
    pub fn active_market(&self) -> Option<String> {
        let idx = match self.container.mode {
            Mode::Fullscreen(i) => i,
            Mode::Tiled => 0,
        };
        self.container.panes.get(idx).map(|p| p.market.clone())
    }

    pub fn pane_count(&self) -> usize {
        self.container.panes.len()
    }

    /// Снимки осей ПО ВИДИМЫМ ПАНЕЛЯМ: (индекс, прямоугольник девайс-px, снимок). Звать ПОСЛЕ prepare.
    pub fn axis_panes(&self, tz_offset_sec: i64) -> Vec<(usize, Rect, AxisSnapshot)> {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        self.container
            .layout(area)
            .into_iter()
            .filter_map(|(idx, rect)| {
                let v = &self.container.panes.get(idx)?.view;
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

    /// Стиль перекрестия из темы (крест рисует GPUI-оверлей, не own-pass).
    pub fn crosshair_style(&self) -> CrossStyle {
        CrossStyle {
            color: self.theme.cross,
            alpha: self.theme.cross_alpha,
            thickness: self.theme.cross_thickness,
            halo_radius: self.theme.halo_radius,
            halo_intensity: self.theme.halo_intensity,
        }
    }
}
