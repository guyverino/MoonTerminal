//! Native `gpu_canvas` рендер чарта (замена wgpu-offscreen+readback). Слои по природе данных
//! (см. `docs/RENDER_PLAN.md`): Combo (рыночная история) / OrderBook (срез) /
//! UserData (мутирующее юзерское) + хром (Grid/Background) + native cursor; текст осей — в GPUI.
//!
//! Доменная специфика чарта живёт ЗДЕСЬ (в терминале); форк gpui отдаёт только generic-хук
//! `RawGpuAccess`. Файл на слой; здесь — оркестратор `ChartEngine`: prepare данных per pane
//! (БЕЗ рисования) + `gpu_canvas` element, который и рисует в кадре GPUI.

mod backend;
#[cfg(windows)]
pub mod background;
#[cfg(windows)]
mod base;
#[cfg(windows)]
pub mod combo;
#[cfg(windows)]
pub mod cursor;
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
use std::rc::{Rc, Weak};

use gpui::{
    GpuBackend, GpuCanvasDriver, GpuCanvasHandle, GpuFrameDecision, GpuFrameInfo, RawGpuAccess,
};
use moon_chart::axes::AxisSnapshot;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::Rect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::session::{CoreId, SessionManager};
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11RasterizerState, ID3D11RenderTargetView,
};

use crate::axes::CrossStyle;
use backend::PlatformLayers;
use pane::{Container, ContainerKind, Mode};
use types::{BackgroundParams, BookStyle, ChartViewGpu, CursorParams, GridParams, cover_uv};

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

#[derive(Clone, Copy, PartialEq)]
struct CursorState {
    pane: usize,
    local: [f32; 2],
}

/// GPU-состояние одной панели для `gpu_canvas` callbacks — отделено от логики `Container`,
/// синхронизируется по индексу + идентичности (core, market) в `prepare`.
struct PaneRender {
    core: Option<CoreId>,
    market: String,
    view: ChartViewGpu,
    layers: PlatformLayers,
    background_params: BackgroundParams,
    grid_params: GridParams,
    cursor_params: CursorParams,
    orderbook_view: ChartViewGpu,
    book_style: BookStyle,
    /// Абсолютный индекс тиков (`total_pushed`), до которого combo уже залит. Сдвиг
    /// головы кольца (drop) НЕ инвалидирует его — хвост `[last_total, total)` валиден
    /// всегда. `u64::MAX` = ещё не заливали / смена монеты → форсит полный reset.
    last_total: u64,
    last_price_lines_rev: u64,
    /// Последнее виденное поколение device combo: сменилось (device-lost) → перезалить историю.
    last_device_gen: u64,
    /// Последняя сборка стакана: ревизия данных + видимое ценовое окно.
    last_book_rev: u64,
    last_book_lo: f32,
    last_book_hi: f32,
    /// Последняя ревизия ордеров, по которой залит userdata-буфер.
    last_orders_rev: u64,
    /// Камера X для own-pass: эпоха времени, поле справа (доля «будущего»), флаг follow и
    /// последняя КВАНТОВАННАЯ пиксель-позиция правого края. Callback двигает камеру по этим
    /// полям на каждый present (vblank, целопиксельно) — живой скролл без отдельного таймера.
    epoch_ms: f64,
    right_margin_frac: f32,
    follow: bool,
    last_edge_px: i64,
    /// Кэш дорогого авто-Y скана (min/max видимых тиков) + пиксель-позиция камеры, при которой
    /// он валиден. Пересканируем лишь на пиксель-кроссе (рубильник, см. prepare).
    scan_cam_px: i64,
    cached_tick_price: Option<(f32, f32)>,
    /// Видима в этом кадре (рисуем) — ставится в `prepare`.
    active: bool,
    /// CPU/base inputs changed and D3D prepare must upload/bake resident resources before draw.
    /// Cursor-only presents leave this false.
    gpu_prepare_dirty: bool,
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
            cursor_params: CursorParams::default(),
            orderbook_view: ChartViewGpu::default(),
            book_style: BookStyle::default(),
            last_total: u64::MAX,
            last_price_lines_rev: u64::MAX,
            last_device_gen: 0,
            last_book_rev: u64::MAX,
            last_book_lo: f32::NAN,
            last_book_hi: f32::NAN,
            last_orders_rev: u64::MAX,
            epoch_ms: 0.0,
            right_margin_frac: 0.10,
            follow: false,
            last_edge_px: i64::MIN,
            scan_cam_px: i64::MIN,
            cached_tick_price: None,
            active: false,
            gpu_prepare_dirty: true,
        }
    }

    /// Пиксельный рубильник камеры (follow по X). Двигаем правый край по `now_ms` ТОЛЬКО
    /// когда «сейчас» уехало на ≥1 ЦЕЛЫЙ пиксель (MoonBot `round(Now/FdtScale)`): между
    /// пикселями кадр попиксельно идентичен → present переказывает его без работы. Целый
    /// шаг убирает субпиксельное дрожание; вызов на каждый present даёт гладкость на vblank.
    /// True — камера реально сдвинулась (для счётчика «рабочих» кадров).
    fn advance_camera(&mut self, now_ms: f64) -> bool {
        if !self.follow || !(self.view.time_to_px > 0.0) {
            return false;
        }
        let ppm = self.view.time_to_px;
        let target_px = ((now_ms - self.epoch_ms) * ppm as f64).round() as i64;
        if target_px == self.last_edge_px {
            return false;
        }
        self.last_edge_px = target_px;
        let inv_ppm = 1.0 / ppm.max(1e-6);
        let area_w = self.view.bounds[2];
        let glass_w = self.orderbook_view.bounds[2];
        let window_ms = area_w * inv_ppm;
        let right_rel = target_px as f32 * inv_ppm;
        self.view.view_time0 = right_rel + window_ms * self.right_margin_frac - window_ms;
        self.view.pad = self.view.view_time0 + (area_w + glass_w) * inv_ppm;
        self.gpu_prepare_dirty = true;
        true
    }
}

/// Состояние рендера всех панелей — шарится с `gpu_canvas` callbacks (`Rc<RefCell>`,
/// единственный поток UI: `prepare` и callbacks кадра не пересекаются по времени).
struct RenderState {
    panes: Vec<PaneRender>,
    /// CPU-side dirty flag для `GpuCanvasDriver::frame`: `prepare()` обновил resident state,
    /// значит следующий platform tick должен презентить кадр даже без GPUI dirty.
    needs_present: bool,
    /// Scene pixels changed since the optional DX11 cursor-restore cache was built.
    /// Live-scroll draws directly and invalidates that cache; cursor-only frames may rebuild it once.
    base_dirty: bool,
    last_present_ms: f64,
    target_present_interval_ms: f64,
    last_gpu_prepare_generation: u64,
    /// Левый верхний угол chart slot в backbuffer. Cursor приходит из UI в локальных
    /// device-px слота, а own-pass рисует в координатах окна.
    slot_origin: [f32; 2],
    cursor: Option<CursorState>,
    cursor_color: [f32; 4],
    cursor_thickness: f32,
    /// Scissor-растеризатор own-pass (lazy, пересоздаётся на смене device): клипует слои к
    /// зоне панели, чтобы стакан/ордера (позиционируются по ЦЕНЕ) не лезли за плот на тулбар/шкалы.
    #[cfg(windows)]
    scissor_rs: Option<ID3D11RasterizerState>,
    #[cfg(windows)]
    scissor_dev: *mut c_void,
    /// Полно-оконная тёмная база: рисуется ПЕРВЫМ слоем own-pass на ВЕСЬ backbuffer
    /// (без scissor), чтобы закрыть белый незакрашенный фон GPUI/SwapChain на первом кадре.
    /// Брендовый empty-state логотип рисуется SVG-слоем GPUI, не растровым native splash.
    #[cfg(windows)]
    window_bg: background::BackgroundLayer,
    /// Цвет тёмной базы (= `rgb4(theme.bg)`), обновляется в `prepare`. Заливает ВСЁ окно.
    #[cfg(windows)]
    window_bg_color: [f32; 4],
    #[cfg(windows)]
    base_cache: base::BaseCache,
}

#[derive(Clone)]
pub struct ChartDataHandle {
    inner: Weak<RefCell<ChartDataState>>,
}

impl PartialEq for ChartDataHandle {
    fn eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }
}

impl ChartDataHandle {
    pub fn is_alive(&self) -> bool {
        self.inner.strong_count() > 0
    }

    pub fn sync_retained_state_if_visible(&self, session: &SessionManager, force: bool) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        inner
            .borrow_mut()
            .sync_retained_state_if_visible(session, force)
    }
}

struct ChartDataState {
    container: Rc<RefCell<Container>>,
    render: Rc<RefCell<RenderState>>,
    theme: ChartTheme,
    orders: OrdersStyle,
    follow: bool,
    present_rate_hz: f32,
    w: u32,
    h: u32,
    origin: (f32, f32),
    scene_visible: bool,
    last_ppp: f32,
    last_prepared_data_sig: u64,
    last_prepared_dev: (u32, u32),
    view_dirty: bool,
}

impl ChartDataState {
    fn new(
        container: Rc<RefCell<Container>>,
        render: Rc<RefCell<RenderState>>,
        theme: ChartTheme,
    ) -> Self {
        Self {
            container,
            render,
            theme,
            orders: OrdersStyle::default(),
            follow: true,
            present_rate_hz: 60.0,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
            scene_visible: false,
            last_ppp: 1.0,
            last_prepared_data_sig: u64::MAX,
            last_prepared_dev: (0, 0),
            view_dirty: true,
        }
    }

    fn data_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        for p in &self.container.borrow().panes {
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

    fn sync_retained_state_if_visible(&mut self, session: &SessionManager, force: bool) -> bool {
        if !self.scene_visible {
            return false;
        }
        let sig = self.data_signature(session);
        if !force && !self.view_dirty && sig == self.last_prepared_data_sig {
            return false;
        }
        crate::diag::bump(&crate::diag::CHART_PREPARE);
        self.sync_from_session(session);
        self.last_prepared_data_sig = sig;
        self.last_prepared_dev = (self.w, self.h);
        self.view_dirty = false;
        true
    }

    fn mark_view_dirty(&mut self) {
        self.view_dirty = true;
    }

    fn sync_from_session(&mut self, session: &SessionManager) {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.borrow().layout(area);
        let now = now_unix_ms();
        let res = [self.w as f32, self.h as f32];
        let mut st = self.render.borrow_mut();
        let mut container = self.container.borrow_mut();
        let mut pixels_changed = false;
        #[cfg(windows)]
        {
            let next_bg_color = rgb4(self.theme.bg);
            if st.window_bg_color != next_bg_color {
                st.window_bg_color = next_bg_color;
                pixels_changed = true;
            }
        }
        let was_active: Vec<bool> = st.panes.iter().map(|pane| pane.active).collect();
        if st.panes.len() != container.panes.len() {
            pixels_changed = true;
        }
        st.panes.resize_with(container.panes.len(), PaneRender::new);
        for pr in &mut st.panes {
            pr.active = false;
        }
        for (idx, rect) in &layout {
            let pane = &mut container.panes[*idx];
            let pr = &mut st.panes[*idx];
            if !was_active.get(*idx).copied().unwrap_or(false) {
                pixels_changed = true;
                pr.gpu_prepare_dirty = true;
            }
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
                pixels_changed = true;
            }
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_book_rev = u64::MAX;
                pr.last_orders_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let price_axis_w = moon_chart::PRICE_AXIS_W * self.last_ppp;
            let time_axis_h = moon_chart::TIME_AXIS_H * self.last_ppp;
            let plot_h = (rect.h - time_axis_h).max(1.0);
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
            pane.view
                .ensure_default_window(chart_area.w, self.present_rate_hz);
            pane.view.follow_edge(now, now);
            let (view_time0, window_ms) = pane.view.visible_x(chart_area.w);
            let data = session.market_view(pane.core, &pane.market);
            let cam_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            if device_lost || cam_px != pr.scan_cam_px {
                pr.cached_tick_price = match data {
                    Some(d) => {
                        let margin = pane.view.marker_half_px / pane.view.px_per_ms.max(1e-6);
                        let (cstart, ccount) = d
                            .ring
                            .visible_range(view_time0 - margin, view_time0 + window_ms + margin);
                        d.ring.price_range_in(cstart, ccount)
                    }
                    None => None,
                };
                pr.scan_cam_px = cam_px;
            }
            let tick_price = pr.cached_tick_price;
            let order_price = session
                .store()
                .core(pane.core)
                .and_then(|core_st| core_st.order_lines.buy_sell_range(&pane.market));
            let last_price = data.and_then(|d| d.last_price);
            let visible_price = union_range(
                union_range(tick_price, order_price),
                last_price.map(|p| (p, p)),
            );
            pane.view.update_y(now, plot_h, visible_price, last_price);
            let area_win = Rect {
                x: self.origin.0 + chart_area.x,
                y: self.origin.1 + chart_area.y,
                w: chart_area.w,
                h: chart_area.h,
            };
            let next_view = view::view_gpu(&pane.view, area_win, res);
            if pr.view != next_view {
                pr.view = next_view;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.epoch_ms = pane.view.epoch_ms;
            pr.right_margin_frac = pane.view.right_margin_frac;
            pr.follow = pane.view.follow;
            pr.last_edge_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let (bg_uv_off, bg_uv_scale) = cover_uv(chart_area.w, chart_area.h, 1.0);
            let background_opacity = if CHART_PHOTO_BACKGROUND_ENABLED {
                self.theme.background_opacity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let next_background_params = BackgroundParams {
                dst: pr.view.bounds,
                resolution: res,
                uv_off: bg_uv_off,
                uv_scale: bg_uv_scale,
                opacity: background_opacity,
                _pad: 0.0,
                bg: rgb4(self.theme.bg),
            };
            if pr.background_params != next_background_params {
                pr.background_params = next_background_params;
                pixels_changed = true;
            }
            let next_grid_params = GridParams {
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
            if pr.grid_params != next_grid_params {
                pr.grid_params = next_grid_params;
                pixels_changed = true;
            }
            let glass_win = Rect {
                x: self.origin.0 + glass_area.x,
                y: self.origin.1 + glass_area.y,
                w: glass_area.w,
                h: glass_area.h,
            };
            let next_orderbook_view = view::view_gpu(&pane.view, glass_win, res);
            if pr.orderbook_view != next_orderbook_view {
                pr.orderbook_view = next_orderbook_view;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let next_book_style = BookStyle {
                book_bg: rgb4(self.theme.book_bg),
                bid: rgb4(self.theme.book_bid),
                ask: rgb4(self.theme.book_ask),
            };
            if pr.book_style != next_book_style {
                pr.book_style = next_book_style;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
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
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else if pr.last_book_rev != u64::MAX {
                pr.layers.set_orderbook(Vec::new());
                pr.last_book_rev = u64::MAX;
                pr.last_book_lo = f32::NAN;
                pr.last_book_hi = f32::NAN;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let edge_rel = view_time0 + (chart_area.w + glass_w) / pane.view.px_per_ms.max(1e-6);
            if pr.view.pad != edge_rel {
                pr.view.pad = edge_rel;
                pixels_changed = true;
            }
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
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else if pr.last_orders_rev != u64::MAX {
                pr.layers.set_userdata(&[], &[], &[], &[]);
                pr.last_orders_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            if let Some(d) = data {
                if pr.last_price_lines_rev != d.price_lines_rev {
                    pr.layers
                        .set_price_lines(d.last_line.points(), d.mark_line.points());
                    pr.last_price_lines_rev = d.price_lines_rev;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                let total = d.ring.total_pushed();
                let avail_from = d.ring.dropped();
                if device_lost || pr.last_total > total || pr.last_total < avail_from {
                    pr.layers.reset_combo(view::collect_all(&d.ring));
                    pr.layers
                        .set_price_lines(d.last_line.points(), d.mark_line.points());
                    pr.last_price_lines_rev = d.price_lines_rev;
                    pr.last_total = total;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                } else if total > pr.last_total {
                    pr.layers
                        .append_combo(&view::collect_since(&d.ring, pr.last_total));
                    pr.last_total = total;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else if pr.last_price_lines_rev != u64::MAX {
                pr.layers.set_price_lines(&[], &[]);
                pr.last_price_lines_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.last_device_gen = device_gen;
            pr.active = true;
        }
        for (idx, was_active) in was_active.into_iter().enumerate() {
            if was_active && !st.panes.get(idx).is_some_and(|pr| pr.active) {
                pixels_changed = true;
            }
        }
        let prev_cursor_params: Vec<CursorParams> =
            st.panes.iter().map(|pr| pr.cursor_params).collect();
        st.sync_cursor_params();
        let cursor_changed = st.cursor.is_some()
            && st
                .panes
                .iter()
                .zip(prev_cursor_params.iter())
                .any(|(pr, prev)| pr.cursor_params != *prev);
        if pixels_changed {
            st.base_dirty = true;
        }
        if pixels_changed || cursor_changed {
            st.needs_present = true;
        }
    }
}

#[derive(Clone)]
struct ChartCanvasDriver {
    state: Rc<RefCell<RenderState>>,
}

impl GpuCanvasDriver for ChartCanvasDriver {
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        self.state.borrow_mut().frame(info)
    }

    fn prepare_gpu(&mut self, ctx: &mut gpui::GpuCanvasPrepareContext<'_>) -> anyhow::Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.state.borrow_mut().prepare_gpu(&ctx.gpu)
        }));
        match result {
            Ok(result) => result,
            Err(e) => {
                let msg = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                    .unwrap_or("<non-string panic>");
                log::error!("chart gpu_canvas prepare PANIC (кадр пропущен): {msg}");
                moon_core::detect_diag::line(&format!("[gpu_canvas] prepare PANIC: {msg}"));
                Ok(())
            }
        }
    }

    fn draw(&mut self, ctx: &mut gpui::GpuCanvasDrawContext<'_>) -> anyhow::Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.state.borrow_mut().draw_gpu(&ctx.gpu)
        }));
        match result {
            Ok(result) => result,
            Err(e) => {
                let msg = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                    .unwrap_or("<non-string panic>");
                log::error!("chart gpu_canvas PANIC (кадр пропущен): {msg}");
                moon_core::detect_diag::line(&format!("[gpu_canvas] PANIC: {msg}"));
                Ok(())
            }
        }
    }
}

impl RenderState {
    fn set_target_present_rate_hz(&mut self, hz: f32) {
        let hz = hz.clamp(1.0, 240.0);
        self.target_present_interval_ms = 1000.0 / hz as f64;
    }

    fn set_slot_origin(&mut self, x: f32, y: f32) {
        let next = [x, y];
        if self.slot_origin != next {
            self.slot_origin = next;
            self.base_dirty = true;
            self.needs_present = true;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    fn set_cursor_style(&mut self, color: [f32; 4], thickness: f32) {
        let thickness = thickness.max(1.0);
        if self.cursor_color != color || self.cursor_thickness != thickness {
            self.cursor_color = color;
            self.cursor_thickness = thickness;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    fn set_cursor(&mut self, cursor: Option<CursorState>) -> bool {
        if self.cursor == cursor {
            return false;
        }
        self.cursor = cursor;
        self.sync_cursor_params();
        self.needs_present = true;
        true
    }

    fn sync_cursor_params(&mut self) {
        for (idx, pr) in self.panes.iter_mut().enumerate() {
            let right = (pr.orderbook_view.bounds[0] + pr.orderbook_view.bounds[2])
                .max(pr.view.bounds[0] + pr.view.bounds[2]);
            let bounds = [
                pr.view.bounds[0],
                pr.view.bounds[1],
                (right - pr.view.bounds[0]).max(1.0),
                pr.view.bounds[3].max(1.0),
            ];
            let mut params = CursorParams {
                bounds,
                resolution: pr.view.resolution,
                color: self.cursor_color,
                thickness: self.cursor_thickness.max(1.0),
                ..CursorParams::default()
            };
            if pr.active {
                if let Some(cursor) = self.cursor.filter(|c| c.pane == idx) {
                    params.cursor = [
                        self.slot_origin[0] + cursor.local[0],
                        self.slot_origin[1] + cursor.local[1],
                    ];
                    params.enabled = 1.0;
                }
            }
            #[cfg(not(windows))]
            let changed = pr.cursor_params != params;
            pr.cursor_params = params;
            #[cfg(not(windows))]
            if changed {
                pr.gpu_prepare_dirty = true;
            }
        }
    }

    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        crate::diag::bump(&crate::diag::CHART_FRAME);
        if !info.presentable || info.bounds.is_empty() {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_NOT_PRESENTABLE);
            return GpuFrameDecision::Skip;
        }

        let now_ms = now_unix_ms();
        let mut wants_present = std::mem::take(&mut self.needs_present);
        let cap_due = self.last_present_ms <= 0.0
            || now_ms - self.last_present_ms >= self.target_present_interval_ms;
        for pr in &mut self.panes {
            if pr.active && (wants_present || cap_due) && pr.advance_camera(now_ms) {
                crate::diag::bump(&crate::diag::CHART_CAM_STEP);
                self.base_dirty = true;
                wants_present = true;
            }
        }

        if wants_present {
            self.last_present_ms = now_ms;
            crate::diag::bump(&crate::diag::CHART_FRAME_REQUEST);
            GpuFrameDecision::RequestPresent
        } else {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_IDLE);
            GpuFrameDecision::Skip
        }
    }

    fn prepare_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        let generation = gpu.device_generation();
        if self.last_gpu_prepare_generation != generation {
            self.last_gpu_prepare_generation = generation;
            self.base_dirty = true;
            for pr in &mut self.panes {
                pr.gpu_prepare_dirty = true;
            }
        }

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let Some((device, context, _rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 prepare received empty D3D11 raw gpu handles");
                };
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if !pr.active || !pr.gpu_prepare_dirty {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_d3d(
                        &view,
                        &orderbook_view,
                        &pr.book_style,
                        &device,
                        &context,
                        gpu,
                    );
                    pr.gpu_prepare_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                for pr in &mut self.panes {
                    if !pr.active || !pr.gpu_prepare_dirty {
                        continue;
                    }
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_wgpu(
                        &pr.view,
                        &pr.background_params,
                        &pr.grid_params,
                        &pr.cursor_params,
                        &pr.orderbook_view,
                        &pr.book_style,
                        gpu,
                    )?;
                    pr.gpu_prepare_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                for pr in &mut self.panes {
                    if !pr.active || !pr.gpu_prepare_dirty {
                        continue;
                    }
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_metal(
                        &pr.view,
                        &pr.background_params,
                        &pr.grid_params,
                        &pr.cursor_params,
                        &pr.orderbook_view,
                        &pr.book_style,
                        gpu,
                    )?;
                    pr.gpu_prepare_dirty = false;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    #[cfg(windows)]
    fn render_window_background_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        let base = BackgroundParams {
            dst: [0.0, 0.0, res[0], res[1]],
            resolution: res,
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            opacity: 0.0,
            _pad: 0.0,
            bg: self.window_bg_color,
        };
        self.window_bg.render(&base, device, context, rtv, gpu);
    }

    #[cfg(windows)]
    fn render_chart_base_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        scissor_rs: &ID3D11RasterizerState,
    ) {
        for pr in &mut self.panes {
            if !pr.active {
                continue;
            }
            let mut view = pr.view;
            let mut background_params = pr.background_params;
            let mut grid_params = pr.grid_params;
            let mut orderbook_view = pr.orderbook_view;
            view.resolution = res;
            background_params.resolution = res;
            grid_params.resolution = res;
            orderbook_view.resolution = res;
            let panel_clip = [
                view.bounds[0],
                view.bounds[1],
                orderbook_view.bounds[0] + orderbook_view.bounds[2],
                view.bounds[1] + view.bounds[3],
            ];
            gpu::set_scissor(
                context,
                scissor_rs,
                panel_clip[0],
                panel_clip[1],
                panel_clip[2],
                panel_clip[3],
            );
            pr.layers.render_base_d3d(
                &view,
                &background_params,
                &grid_params,
                &orderbook_view,
                &pr.book_style,
                device,
                context,
                rtv,
                gpu,
                panel_clip,
            );
        }
    }

    fn draw_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        crate::diag::bump(&crate::diag::CHART_PRESENT);

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let RawGpuAccess::D3d11(d3d) = gpu else {
                    anyhow::bail!("chart dx11 draw received non-D3D11 raw gpu access");
                };
                let Some((device, context, rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 draw received empty D3D11 raw gpu handles");
                };

                let d3d_device_ptr = d3d.device.as_ptr();
                if self.scissor_dev != d3d_device_ptr {
                    self.scissor_rs = Some(gpu::create_scissor_rasterizer(&device));
                    self.scissor_dev = d3d_device_ptr;
                }
                let res = [width as f32, height as f32];
                let scissor_rs = self.scissor_rs.clone().unwrap();
                let prev_rs = unsafe { context.RSGetState().ok() };

                if self.base_dirty {
                    self.render_window_background_d3d(res, &device, &context, &rtv, gpu);
                    self.render_chart_base_d3d(res, &device, &context, &rtv, gpu, &scissor_rs);
                    self.base_cache.invalidate();
                    self.base_dirty = false;
                } else {
                    if self.base_cache.needs_rebuild(gpu) {
                        let base_rtv = self.base_cache.begin_rebuild(&device, &context, gpu)?;
                        self.render_window_background_d3d(res, &device, &context, &base_rtv, gpu);
                        self.render_chart_base_d3d(
                            res,
                            &device,
                            &context,
                            &base_rtv,
                            gpu,
                            &scissor_rs,
                        );
                    }
                    self.base_cache.blit_to(&context, &rtv, gpu);
                }

                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let mut cursor_params = pr.cursor_params;
                    let mut view = pr.view;
                    let mut orderbook_view = pr.orderbook_view;
                    cursor_params.resolution = res;
                    view.resolution = res;
                    orderbook_view.resolution = res;
                    let panel_clip = [
                        view.bounds[0],
                        view.bounds[1],
                        orderbook_view.bounds[0] + orderbook_view.bounds[2],
                        view.bounds[1] + view.bounds[3],
                    ];
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        panel_clip[0],
                        panel_clip[1],
                        panel_clip[2],
                        panel_clip[3],
                    );
                    pr.layers
                        .render_cursor_d3d(&cursor_params, &device, &context, &rtv, gpu);
                }
                unsafe {
                    context.RSSetState(prev_rs.as_ref());
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        pr.layers.render_wgpu(
                            &view,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &orderbook_view,
                            gpu,
                        )?;
                    }
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        pr.layers.render_metal(
                            &view,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &orderbook_view,
                            gpu,
                        )?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

pub struct ChartEngine {
    container: Rc<RefCell<Container>>,
    state: Rc<RefCell<RenderState>>,
    data: Rc<RefCell<ChartDataState>>,
    canvas: GpuCanvasHandle,
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
}

impl ChartEngine {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        Self::new_kind(epoch, theme, ContainerKind::Main)
    }

    pub fn new_kind(epoch: f64, theme: ChartTheme, kind: ContainerKind) -> Self {
        let container = Rc::new(RefCell::new(Container::new(kind)));
        let state = Rc::new(RefCell::new(RenderState {
            panes: Vec::new(),
            needs_present: true,
            base_dirty: true,
            last_present_ms: 0.0,
            target_present_interval_ms: 1000.0 / 60.0,
            last_gpu_prepare_generation: 0,
            slot_origin: [0.0, 0.0],
            cursor: None,
            cursor_color: {
                let mut c = rgb4(theme.cross);
                c[3] = theme.cross_alpha;
                c
            },
            cursor_thickness: theme.cross_thickness.max(1.0),
            #[cfg(windows)]
            scissor_rs: None,
            #[cfg(windows)]
            scissor_dev: std::ptr::null_mut(),
            #[cfg(windows)]
            window_bg: background::BackgroundLayer::new(background::SPLASH_PNG),
            #[cfg(windows)]
            window_bg_color: rgb4(theme.bg),
            #[cfg(windows)]
            base_cache: base::BaseCache::new(),
        }));
        let canvas = GpuCanvasHandle::new(ChartCanvasDriver {
            state: state.clone(),
        });
        let data = Rc::new(RefCell::new(ChartDataState::new(
            container.clone(),
            state.clone(),
            theme.clone(),
        )));
        Self {
            container,
            state,
            data,
            canvas,
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            present_rate_hz: 60.0,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
        }
    }

    pub fn data_handle(&self) -> ChartDataHandle {
        ChartDataHandle {
            inner: Rc::downgrade(&self.data),
        }
    }

    /// Обычный GPUI element, который владеет bounds/clip/lifetime через дерево.
    /// В отличие от старого window-global pass, он сам исчезает при скрытии вкладки
    /// и переезжает при detach вместе с `ChartPanel`.
    pub fn canvas(&self) -> gpui::GpuCanvas {
        gpui::gpu_canvas(self.canvas.clone())
    }

    /// Размер слота чарта (девайс-px). Combo сам пересоздаёт битмап при смене размера.
    pub fn resize(&mut self, w: u32, h: u32) {
        let next_w = w.max(1);
        let next_h = h.max(1);
        if self.w == next_w && self.h == next_h {
            return;
        }
        self.w = next_w;
        self.h = next_h;
        let mut data = self.data.borrow_mut();
        data.w = self.w;
        data.h = self.h;
        data.mark_view_dirty();
    }

    /// Левый-верхний угол слота чарта В ОКНЕ (девайс-px) — для координат own-pass в backbuffer.
    pub fn set_origin(&mut self, x: f32, y: f32) {
        if self.origin == (x, y) {
            return;
        }
        self.origin = (x, y);
        {
            let mut data = self.data.borrow_mut();
            data.origin = self.origin;
            data.mark_view_dirty();
        }
        self.state.borrow_mut().set_slot_origin(x, y);
    }

    pub fn set_present_rate_hz(&mut self, hz: f32) {
        self.present_rate_hz = hz.max(1.0);
        self.data.borrow_mut().present_rate_hz = self.present_rate_hz;
        self.state
            .borrow_mut()
            .set_target_present_rate_hz(self.present_rate_hz);
    }

    pub fn set_cursor(&mut self, cursor: Option<(usize, f32, f32)>) -> bool {
        self.state
            .borrow_mut()
            .set_cursor(cursor.map(|(pane, x, y)| CursorState {
                pane,
                local: [x, y],
            }))
    }

    /// ПОДГОТОВКА кадра (вместо wgpu submit+readback): обновляет вид и данные слоёв каждой
    /// видимой панели. НЕ рисует — рисование делает `gpu_canvas.draw()`. Дёшево:
    /// математика вида + конверт новых тиков; тяжёлое (bake/blit) — на GPU в callback.
    /// Sync app/session data into retained chart state. This is the data-ingest
    /// side of the bridge; `gpu_canvas.frame()` later consumes the retained dirty
    /// flags and decides whether the current platform tick should present.
    pub fn sync_retained_state_if_visible(
        &mut self,
        session: &SessionManager,
        force: bool,
    ) -> bool {
        self.data
            .borrow_mut()
            .sync_retained_state_if_visible(session, force)
    }

    pub fn data_signature(&self, session: &SessionManager) -> u64 {
        self.data.borrow().data_signature(session)
    }

    pub fn set_scene_visible(&mut self, visible: bool) {
        self.data.borrow_mut().scene_visible = visible;
    }

    pub fn set_last_ppp(&mut self, ppp: f32) {
        self.data.borrow_mut().last_ppp = ppp.max(0.1);
    }

    // ── Настройки (порт из старого chart.rs::ChartGpu) ───────────────────────────

    pub fn set_theme(&mut self, theme: ChartTheme) -> bool {
        if self.theme != theme {
            let mut cursor_color = rgb4(theme.cross);
            cursor_color[3] = theme.cross_alpha;
            self.state
                .borrow_mut()
                .set_cursor_style(cursor_color, theme.cross_thickness);
            self.theme = theme;
            let mut data = self.data.borrow_mut();
            data.theme = self.theme.clone();
            data.mark_view_dirty();
            true
        } else {
            false
        }
    }

    pub fn set_orders(&mut self, orders: OrdersStyle) -> bool {
        if self.orders != orders {
            self.orders = orders;
            let mut data = self.data.borrow_mut();
            data.orders = self.orders.clone();
            data.mark_view_dirty();
            drop(data);
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
        self.container.borrow_mut().set_scale(pct);
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    /// Глобальный live-follow из тулбара (Live/Пауза) ко ВСЕМ панелям. Реагирует ТОЛЬКО
    /// на смену самого глобального флага (явный клик). НЕ на производное состояние от пана
    /// одной панели: иначе пан одной монеты в Tiled гасил бы live у соседних, которых не
    /// трогали (их view.follow перетирался). Пан/rejoin отдельной панели живут в её
    /// view.follow; сюда уже сведённое значение прилетает через sync_follow_from_views, и
    /// если глобальный флаг не изменился — выходим, панели не трогаем.
    pub fn set_follow(&mut self, follow: bool, now_ms: f64) -> bool {
        if self.follow == follow {
            return false;
        }
        self.follow = follow;
        self.data.borrow_mut().follow = follow;
        for p in &mut self.container.borrow_mut().panes {
            if follow {
                // Возобновляем live только у панелей, которые НЕ следовали (явный Live из
                // тулбара): уже живые панели не трогаем — их окно/зум не сбрасываем.
                if !p.view.follow {
                    p.view.resume_live(now_ms);
                    p.view.reset_default_window_on_next_prepare();
                }
            } else {
                p.view.follow = false;
            }
        }
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    pub fn sync_follow_from_views(&mut self) -> bool {
        let container = self.container.borrow();
        let follow = if container.panes.is_empty() {
            self.follow
        } else {
            container.panes.iter().all(|p| p.view.follow)
        };
        drop(container);
        if self.follow == follow {
            false
        } else {
            self.follow = follow;
            self.data.borrow_mut().follow = follow;
            true
        }
    }

    /// Открыть монету (фулскрин-панель).
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container
            .borrow_mut()
            .open_manual(core, market, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// AddToChart: добавить монету авто-панелью (Tiled) с TTL.
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container
            .borrow_mut()
            .push_auto(core, market, now_ms, ttl_ms, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Убрать истёкшие AddToChart-панели. True — если что-то удалили.
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        let changed = self.container.borrow_mut().prune_ttl(now_ms);
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    #[allow(dead_code)]
    pub fn has_ttl_panes(&self) -> bool {
        self.container.borrow().has_ttl_panes()
    }

    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.container.borrow().next_ttl_deadline_ms()
    }

    pub fn with_container_mut<R>(&mut self, f: impl FnOnce(&mut Container) -> R) -> R {
        let out = f(&mut self.container.borrow_mut());
        self.data.borrow_mut().mark_view_dirty();
        out
    }

    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        let removed = self.container.borrow_mut().remove_pane(idx);
        if removed.is_some() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.container.borrow().uses_market(core, market)
    }

    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        let removed = self.container.borrow_mut().clear_panes();
        if !removed.is_empty() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    /// Рынок активной (фулскрин/первой) панели — для подписи вкладки.
    pub fn active_market(&self) -> Option<String> {
        let container = self.container.borrow();
        let idx = match container.mode {
            Mode::Fullscreen(i) => i,
            Mode::Tiled => 0,
        };
        container.panes.get(idx).map(|p| p.market.clone())
    }

    pub fn pane_count(&self) -> usize {
        self.container.borrow().panes.len()
    }

    /// Снимки осей ПО ВИДИМЫМ ПАНЕЛЯМ: (индекс, прямоугольник девайс-px, снимок). Звать ПОСЛЕ prepare.
    pub fn axis_panes(&self, tz_offset_sec: i64) -> Vec<(usize, Rect, AxisSnapshot)> {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let container = self.container.borrow();
        container
            .layout(area)
            .into_iter()
            .filter_map(|(idx, rect)| {
                let v = &container.panes.get(idx)?.view;
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
