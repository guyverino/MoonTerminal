//! Own-pass DX11 рендер чарта (замена wgpu-offscreen+readback). Слои по природе данных
//! (см. `TERMINAL_RENDER_ARCHITECTURE.md` §9): Combo (рыночная история) / OrderBook (срез) /
//! UserData (мутирующее юзерское) + хром (Grid/Background) + текст/курсор поверх в GPUI.
//!
//! Доменная специфика чарта живёт ЗДЕСЬ (в терминале); форк gpui отдаёт только generic-хук
//! `RawGpuAccess`. Файл на слой; здесь — оркестратор `ChartEngine`: prepare данных per pane
//! (БЕЗ рисования) + регистрация own-pass, который и рисует в кадре GPUI.

pub mod combo;
pub mod cursor;
pub mod gpu;
pub mod grid;
pub mod orderbook;
pub mod pane;
pub mod userdata;
pub mod view;

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use gpui::{GpuPhase, RawGpuAccess, Window};
use windows::Win32::Graphics::Direct3D11::ID3D11RasterizerState;
use moon_chart::axes::AxisSnapshot;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::Rect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::session::{CoreId, SessionManager};

use crate::axes::CrossStyle;
use combo::ComboLayer;
use cursor::{CursorLayer, CursorParams};
use gpu::ChartViewGpu;
use grid::{GridLayer, GridParams};
use orderbook::{BookStyle, OrderBookLayer};
use pane::{Container, ContainerKind, Mode};
use userdata::UserDataLayer;

/// sRGB [u8;3] → [f32;4] (alpha 1) для cbuffer-цветов (шейдер переводит в linear).
fn rgb4(c: [u8; 3]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

/// GPU-состояние одной панели для own-pass callback — отделено от логики `Container`,
/// синхронизируется по индексу + идентичности (core, market) в `prepare`.
struct PaneRender {
    core: Option<CoreId>,
    market: String,
    view: ChartViewGpu,
    combo: ComboLayer,
    grid: GridLayer,
    grid_params: GridParams,
    orderbook: OrderBookLayer,
    orderbook_view: ChartViewGpu,
    book_style: BookStyle,
    userdata: UserDataLayer,
    /// Сколько тиков уже залито в кольцо combo + значение `dropped` (для append/reset).
    last_len: usize,
    last_dropped: u64,
    /// Последнее виденное поколение device combo: сменилось (device-lost) → перезалить историю.
    last_device_gen: u64,
    /// Видима в этом кадре (рисуем) — ставится в `prepare`.
    active: bool,
}

impl PaneRender {
    fn new() -> Self {
        Self {
            core: None,
            market: String::new(),
            view: ChartViewGpu::default(),
            combo: ComboLayer::new(),
            grid: GridLayer::new(),
            grid_params: GridParams::default(),
            orderbook: OrderBookLayer::new(),
            orderbook_view: ChartViewGpu::default(),
            book_style: BookStyle::default(),
            userdata: UserDataLayer::new(),
            last_len: 0,
            last_dropped: u64::MAX,
            last_device_gen: 0,
            active: false,
        }
    }
}

/// Состояние рендера всех панелей — шарится с own-pass callback'ом (`Rc<RefCell>`,
/// единственный поток UI: `prepare` и callback кадра не пересекаются по времени).
struct RenderState {
    panes: Vec<PaneRender>,
    /// Крестик-курсор — один на чарт (рисуется на панели под мышью). См. `cursor`.
    cursor: CursorLayer,
    cursor_params: Option<CursorParams>,
    /// Scissor-растеризатор own-pass (lazy, пересоздаётся на смене device): клипует слои к
    /// зоне панели, чтобы стакан/ордера (позиционируются по ЦЕНЕ) не лезли за плот на тулбар/шкалы.
    scissor_rs: Option<ID3D11RasterizerState>,
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
    /// Размер слота чарта (девайс-px) — меряется canvas-оверлеем окна.
    w: u32,
    h: u32,
    /// Левый-верхний угол слота чарта В ОКНЕ (девайс-px). own-pass рисует в backbuffer ОКНА,
    /// поэтому координаты слоёв = origin слота + локальные, а cv_resolution = размер backbuffer.
    origin: (f32, f32),
    /// Курсор (px окна) + панель под мышью — для own-pass крестика в backbuffer окна.
    cursor_win: Option<(f32, f32)>,
    hovered: Option<usize>,
    registered: bool,
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
                cursor: CursorLayer::new(),
                cursor_params: None,
                scissor_rs: None,
                scissor_dev: std::ptr::null_mut(),
            })),
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
            cursor_win: None,
            hovered: None,
            registered: false,
        }
    }

    /// Курсор (px окна) + индекс панели под мышью (для own-pass крестика). None = вне чарта.
    pub fn set_cursor(&mut self, cursor_win: Option<(f32, f32)>, hovered: Option<usize>) {
        self.cursor_win = cursor_win;
        self.hovered = hovered;
    }

    /// Регистрирует own-pass ОДИН раз: callback рисует все активные панели
    /// (combo + слои) их own-pass в backbuffer GPUI ПОД сценой. Зовётся из `Render` (есть окно).
    pub fn register_pass(&mut self, window: &mut Window) {
        if self.registered {
            return;
        }
        let state = self.state.clone();
        // UnderScene — правильный финальный слой: GPUI-хром, попапы, меню и тултипы должны
        // быть поверх графика. Chart host/content в MoonPalette держатся на NoFill, обычные
        // панели — Opaque, поэтому фоновые quads не перекрывают plot area.
        window.add_gpu_pass(
            GpuPhase::UnderScene,
            Box::new(move |gpu: &RawGpuAccess| {
                let mut st = state.borrow_mut();
                let Some((device, context, rtv)) = gpu::borrow_d3d(gpu) else {
                    return;
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
                        gpu::set_scissor(
                            &context,
                            &scissor_rs,
                            pr.view.bounds[0],
                            pr.view.bounds[1],
                            pr.orderbook_view.bounds[0] + pr.orderbook_view.bounds[2],
                            pr.view.bounds[1] + pr.view.bounds[3],
                        );
                        // Z ВНУТРИ own-pass: сетка ПОД данными → кресты поверх; стакан — своя зона справа.
                        pr.grid.render(&pr.grid_params, &device, &context, &rtv, gpu);
                        pr.combo.render(&pr.view, &device, &context, &rtv, gpu);
                        pr.orderbook
                            .render(&pr.orderbook_view, &pr.book_style, &device, &context, &rtv, gpu);
                        // Ордера юзера — ПОВЕРХ данных, тем же chart_area-трансформом (линии тянутся
                        // в зону стакана; scissor зоны их там и удержит).
                        pr.userdata.render(&pr.view, &device, &context, &rtv, gpu);
                    }
                }
                // Крестик-курсор — последним, поверх всего (на панели под мышью), в своей зоне.
                if let Some(mut cp) = st.cursor_params {
                    cp.resolution = [gpu.width as f32, gpu.height as f32];
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        cp.bounds[0],
                        cp.bounds[1],
                        cp.bounds[0] + cp.bounds[2],
                        cp.bounds[1] + cp.bounds[3],
                    );
                    st.cursor.render(&cp, &device, &context, &rtv, gpu);
                }
                // Вернуть растеризатор GPUI (scissor off).
                unsafe {
                    context.RSSetState(prev_rs.as_ref());
                }
            }),
        );
        self.registered = true;
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

    /// ПОДГОТОВКА кадра (вместо wgpu submit+readback): обновляет вид и данные слоёв каждой
    /// видимой панели. НЕ рисует — рисование в own-pass callback (`register_pass`). Дёшево:
    /// математика вида + конверт новых тиков; тяжёлое (bake/blit) — на GPU в callback.
    pub fn prepare(&mut self, session: &SessionManager, ppp: f32) {
        let area = Rect { x: 0.0, y: 0.0, w: self.w as f32, h: self.h as f32 };
        let layout = self.container.layout(area);
        let now = now_unix_ms();
        let res = [self.w as f32, self.h as f32];
        let mut st = self.state.borrow_mut();
        st.panes.resize_with(self.container.panes.len(), PaneRender::new);
        for pr in &mut st.panes {
            pr.active = false;
        }
        let mut cur_params: Option<CursorParams> = None;
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
            // Крестик own-pass на панели под мышью: область = плот+стакан (px окна).
            if self.hovered == Some(*idx) {
                if let Some((cxp, cyp)) = self.cursor_win {
                    let cc = self.theme.cross;
                    cur_params = Some(CursorParams {
                        bounds: [
                            self.origin.0 + chart_area.x,
                            self.origin.1 + chart_area.y,
                            chart_area.w + glass_w,
                            chart_area.h,
                        ],
                        resolution: res,
                        cursor: [cxp, cyp],
                        color: [
                            cc[0] as f32 / 255.0,
                            cc[1] as f32 / 255.0,
                            cc[2] as f32 / 255.0,
                            self.theme.cross_alpha,
                        ],
                        thickness: self.theme.cross_thickness,
                        pad: [0.0; 3],
                    });
                }
            }
            // Математика вида (общая с эталоном): X-окно → видимый срез тиков → авто-Y по нему.
            pane.view.ensure_default_window(chart_area.w);
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
            let visible_price = data.and_then(|d| d.ring.price_range_in(cstart, ccount));
            let last_price = data.and_then(|d| d.last_price);
            pane.view.update_y(now, rect.h, visible_price, last_price);
            // own-pass рисует в backbuffer ОКНА → bounds в координатах окна (origin слота +
            // локальные). resolution тут placeholder — реальный backbuffer ставит callback.
            let area_win = Rect {
                x: self.origin.0 + chart_area.x,
                y: self.origin.1 + chart_area.y,
                w: chart_area.w,
                h: chart_area.h,
            };
            pr.view = view::view_gpu(&pane.view, area_win, res);
            // Сетка: СТАТИЧНЫЕ вертикали (6 делений, как подписи времени) + горизонтали по цене
            // (шаг = nice_interval, совпадает с подписями цены). resolution ставит callback.
            pr.grid_params = GridParams {
                bounds: pr.view.bounds,
                resolution: res,
                n_vert: 6.0,
                price_to_px: pr.view.price_to_px,
                view_price0: pr.view.view_price0,
                price_interval: moon_chart::axes::nice_interval(pane.view.render_range.max(1e-9), 8.0),
                grid_alpha: self.theme.grid_alpha,
                _pad: 0.0,
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
            let mut levels = Vec::new();
            if let Some(d) = data {
                let half = pane.view.render_range.max(1e-9) * 0.5;
                let (lo, hi) = (pane.view.render_center - half, pane.view.render_center + half);
                d.book.build_instances(lo, hi, &mut levels);
            }
            pr.orderbook.set(levels);
            // Ордера юзера (UserData): геометрия лестниц/линий/маркеров из ретейн-стора ядра.
            // Активные линии тянутся до правого края plot (через стакан) → edge_rel. Координаты
            // логические (time_rel/price), трансформ в шейдере — тот же chart_area (pr.view).
            let edge_rel = view_time0 + (chart_area.w + glass_w) / pane.view.px_per_ms.max(1e-6);
            let mut hlines = Vec::new();
            let mut segs = Vec::new();
            let mut markers = Vec::new();
            if let Some(core_st) = session.store().core(pane.core) {
                moon_chart::build_order_geometry(
                    &core_st.order_lines,
                    &pane.market,
                    &self.orders,
                    pane.view.epoch_ms,
                    now,
                    view_time0,
                    view_time0 + window_ms,
                    edge_rel,
                    &mut hlines,
                    &mut segs,
                    &mut markers,
                );
            }
            pr.userdata.set(&hlines, &segs, &markers);
            // Trades в combo: полный reset при съезде индексов (drain) ИЛИ device-lost (GPUI
            // пересоздал device → кольцо combo пустое, append живого края не восстановит историю);
            // иначе append живого края. device_gen combo инкрементится в его render при смене device.
            if let Some(d) = data {
                let cur_len = d.ring.len();
                let cur_dropped = d.ring.dropped();
                let cur_gen = pr.combo.device_gen();
                if pr.last_dropped != cur_dropped || pr.last_device_gen != cur_gen {
                    pr.combo.reset(view::collect_all(&d.ring));
                    pr.last_len = cur_len;
                    pr.last_dropped = cur_dropped;
                    pr.last_device_gen = cur_gen;
                } else if cur_len > pr.last_len {
                    pr.combo
                        .append(&view::collect_range(&d.ring, pr.last_len, cur_len));
                    pr.last_len = cur_len;
                }
            }
            pr.active = true;
        }
        st.cursor_params = cur_params;
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
        if self.follow == follow {
            return false;
        }
        self.follow = follow;
        for p in &mut self.container.panes {
            if follow {
                p.view.resume_live(now_ms);
            } else {
                p.view.follow = false;
            }
        }
        true
    }

    /// Открыть монету (фулскрин-панель).
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container.open_manual(core, market, self.epoch);
    }

    /// AddToChart: добавить монету авто-панелью (Tiled) с TTL.
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container.push_auto(core, market, now_ms, ttl_ms, self.epoch);
    }

    /// Убрать истёкшие AddToChart-панели. True — если что-то удалили.
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        self.container.prune_ttl(now_ms)
    }

    #[allow(dead_code)]
    pub fn has_ttl_panes(&self) -> bool {
        self.container.has_ttl_panes()
    }

    /// Сигнатура рыночных данных (ticks_rev+book_rev по всем панелям) — для гейта пере-рендера.
    pub fn data_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        for p in &self.container.panes {
            if let Some(v) = session.market_view(p.core, &p.market) {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(v.ticks_rev)
                    .wrapping_add(v.book_rev);
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
        let area = Rect { x: 0.0, y: 0.0, w: self.w as f32, h: self.h as f32 };
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
