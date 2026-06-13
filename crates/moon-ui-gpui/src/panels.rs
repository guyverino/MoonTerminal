//! Панели нижнего дока (порт egui `src/dock/tabs.rs`): Ордера/Активы/Лог/Отчёт как
//! `gpui_component::dock::Panel` — получают вкладки, сплиты, отцепление в окно и
//! персист раскладки от `DockArea` (см. [[prefer-gpui-components]]). Сейчас наполнены
//! Ордера (виртуализированная Table); Активы/Лог/Отчёт — заглушки до подключения данных.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    table::{Table, TableState},
    v_flex,
};

use crate::chart::ChartGpu;
use crate::{axes, collect_orders, hex, input, Backend, OrdersDelegate};
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::ChartTheme;
use moon_core::palette;
use moon_core::session::CoreId;

/// Панель «Ордера» — виртуализированная таблица ордеров группы.
pub struct OrdersPanel {
    orders: Entity<TableState<OrdersDelegate>>,
    /// Группа окна — нужна для персиста раскладки (dump → docks.json).
    group: String,
    focus: FocusHandle,
}

impl OrdersPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let orders = cx.new(|cx| TableState::new(OrdersDelegate::new(), window, cx));
        // Дренаж backend → пересобрать строки таблицы.
        let g = group.clone();
        let orders_h = orders.clone();
        cx.observe(&backend, move |_this, backend, cx| {
            let rows = collect_orders(backend.read(cx), &g);
            orders_h.update(cx, |st, cx| {
                st.delegate_mut().rows = rows;
                cx.notify();
            });
        })
        .detach();
        Self { orders, group, focus: cx.focus_handle() }
    }
}

impl EventEmitter<PanelEvent> for OrdersPanel {}
impl Focusable for OrdersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrdersPanel {
    fn panel_name(&self) -> &'static str {
        "Orders"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Ордера")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Orders", &self.group)
    }
}
impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("orders-panel")
            .size_full()
            .track_focus(&self.focus)
            .child(Table::new(&self.orders))
    }
}

/// Кнопка ленты детектов (порт `src/dock/detects.rs::RibbonItem`).
struct DetectItem {
    core: CoreId,
    core_name: String,
    market: String,
    color: [u8; 3],
    born_ms: f64,
    ttl_ms: f64,
}

/// Лента детектов — откпрепляемая панель (порт egui `DetectRibbon`). Втягивает
/// детекты ядер группы с `SoundAlert=Yes` и `AddToChart==0` (AddToChart-детекты —
/// в чарт-вкладки), держит `KeepAlert` секунд, новые сверху. Клик → открыть монету
/// на Main (через `Backend.open_request`, который читает Shell).
pub struct DetectsPanel {
    backend: Entity<Backend>,
    group: String,
    items: VecDeque<DetectItem>,
    last_seq: HashMap<CoreId, u64>,
    focus: FocusHandle,
}

const MAX_DETECT_BTNS: usize = 48;

impl DetectsPanel {
    pub fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        cx.observe(&backend, |this, backend, cx| {
            this.ingest(backend.read(cx));
            this.prune(now_unix_ms());
            cx.notify();
        })
        .detach();
        Self { backend, group, items: VecDeque::new(), last_seq: HashMap::new(), focus: cx.focus_handle() }
    }

    /// Втянуть свежие детекты ядер группы (seq > курсора, sound_alert, не AddToChart).
    fn ingest(&mut self, b: &Backend) {
        let cores: Vec<(CoreId, String, [u8; 3])> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| {
                let color = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == s.id)
                    .map(|sv| sv.color)
                    .unwrap_or(palette::ACCENT);
                (s.id, s.name.clone(), color)
            })
            .collect();
        for (id, name, color) in cores {
            let Some(d) = b.session.store().core(id) else { continue };
            let last = self.last_seq.get(&id).copied().unwrap_or(0);
            let mut fresh: Vec<&moon_core::feed::DetectRow> = Vec::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                fresh.push(det);
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(id, fresh[0].seq);
            for det in fresh.iter().rev() {
                if !det.sound_alert || det.add_to_chart > 0 {
                    continue;
                }
                let ttl = (det.keep_alert_secs.max(1) as f64) * 1000.0;
                if let Some(it) = self.items.iter_mut().find(|it| it.core == id && it.market == det.market) {
                    it.born_ms = det.time_ms;
                    it.ttl_ms = ttl;
                    it.color = color;
                } else {
                    self.items.push_back(DetectItem {
                        core: id,
                        core_name: name.clone(),
                        market: det.market.clone(),
                        color,
                        born_ms: det.time_ms,
                        ttl_ms: ttl,
                    });
                }
            }
        }
        while self.items.len() > MAX_DETECT_BTNS {
            self.items.pop_front();
        }
    }

    fn prune(&mut self, now_ms: f64) {
        self.items.retain(|it| now_ms - it.born_ms < it.ttl_ms);
    }

    /// Открыть монету на Main: запрос в Backend (Shell откроет чарт) + убрать кнопку.
    fn open(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.items.retain(|it| !(it.core == core && it.market == market));
        self.backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market.clone()));
            bcx.notify();
        });
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for DetectsPanel {}
impl Focusable for DetectsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for DetectsPanel {
    fn panel_name(&self) -> &'static str {
        "Detects"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Детекты")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Detects", &self.group)
    }
}
impl Render for DetectsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_unix_ms();
        let mut col = v_flex().id("detects").size_full().gap_1().p_2().track_focus(&self.focus);
        // Новые сверху.
        for (i, it) in self.items.iter().enumerate().rev() {
            let secs = ((it.ttl_ms - (now - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            let glow = rgb(hex(it.color));
            let (core, market) = (it.core, it.market.clone());
            col = col.child(
                div()
                    .id(SharedString::from(format!("det-{i}")))
                    .w_full()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(glow)
                    .bg(rgb(hex(palette::LIFT)))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(div().child(it.market.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(hex(palette::TEXT_2)))
                                    .child(format!("{secs}s · {}", it.core_name)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open(core, market.clone(), cx);
                    })),
            );
        }
        col
    }
}

/// Заглушка-панель (Активы/Лог/Отчёт) до подключения данных.
pub struct StubPanel {
    name: &'static str,
    title: SharedString,
    focus: FocusHandle,
}

impl StubPanel {
    pub fn new(name: &'static str, title: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self { name, title: title.into(), focus: cx.focus_handle() }
    }
}

impl EventEmitter<PanelEvent> for StubPanel {}
impl Focusable for StubPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for StubPanel {
    fn panel_name(&self) -> &'static str {
        self.name
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
}
impl Render for StubPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.name)
            .size_full()
            .p_4()
            .track_focus(&self.focus)
            .text_color(rgb(hex(palette::TEXT_2)))
            .child(format!("{} — скоро", self.title))
    }
}

/// Панель чарта (center DockArea): движок offscreen+readback + ввод + оверлей осей.
/// Перенос всей чарт-логики из Shell. Как Dock-панель — отцепляется в окно (ChartGpu
/// рендерит offscreen, не привязан к ОС-окну). Монета — из focus и `Backend.open_request`.
pub struct ChartPanel {
    backend: Entity<Backend>,
    chart: ChartGpu,
    chart_img: Option<Arc<RenderImage>>,
    chart_dirty: bool,
    chart_dev: (u32, u32),
    chart_bounds: Option<Bounds<Pixels>>,
    input: input::ChartInput,
    market: Option<String>,
    /// Номер AddToChart-вкладки (None = Main).
    num: Option<u32>,
    focus: FocusHandle,
}

impl ChartPanel {
    pub fn new(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartGpu::new(epoch, theme);
        let mut market = None;
        if let Some((core, m)) = focus_open {
            chart.open(core, &m);
            market = Some(m.clone());
            backend.update(cx, |b, _| {
                if !b.desired.iter().any(|(c, mm)| *c == core && mm == &m) {
                    b.desired.push((core, m));
                }
            });
        }
        // open_request (дабл-клик→Main) обрабатывает ChartTabs (ему нужно ещё
        // переключить активную вкладку). Здесь — только prune + перерисовка.
        cx.observe(&backend, |this, _backend, cx| {
            this.chart.prune_ttl(now_unix_ms());
            this.chart_dirty = true;
            cx.notify();
        })
        .detach();
        Self {
            backend,
            chart,
            chart_img: None,
            chart_dirty: true,
            chart_dev: (1024, 576),
            chart_bounds: None,
            input: input::ChartInput::default(),
            market,
            num: None,
            focus: cx.focus_handle(),
        }
    }

    /// Открыть монету на этой (Main) панели фуллскрином + подписать рынок.
    pub fn open_market(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.chart.open(core, &market);
        self.market = Some(market.clone());
        self.backend.update(cx, |b, _| {
            if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                b.desired.push((core, market));
            }
        });
        self.chart_dirty = true;
    }

    /// AddToChart-вкладка №`num` (без focus-монеты; наполняется детектами через add_coin).
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chart = ChartGpu::new_kind(epoch, theme, ContainerKind::Chart { num, core: None });
        // Дренаж → prune истёкших панелей + пере-рендер. open_request НЕ берём (Main-only).
        cx.observe(&backend, |this, _backend, cx| {
            this.chart.prune_ttl(now_unix_ms());
            this.chart_dirty = true;
            cx.notify();
        })
        .detach();
        Self {
            backend,
            chart,
            chart_img: None,
            chart_dirty: true,
            chart_dev: (1024, 576),
            chart_bounds: None,
            input: input::ChartInput::default(),
            market: None,
            num: Some(num),
            focus: cx.focus_handle(),
        }
    }

    /// Число открытых панелей чарта (для бейджа-счётчика на вкладке, как в egui).
    pub fn pane_count(&self) -> usize {
        self.chart.container.panes.len()
    }

    /// AddToChart: добавить монету авто-панелью (Tiled-мультичарт) с TTL.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64) {
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.chart_dirty = true;
    }

    fn chart_local(&self, pos: Point<Pixels>, sf: f32) -> Option<((f32, f32), bool)> {
        let b = self.chart_bounds?;
        let lx = f32::from(pos.x) - f32::from(b.origin.x);
        let ly = f32::from(pos.y) - f32::from(b.origin.y);
        let w = f32::from(b.size.width);
        let h = f32::from(b.size.height);
        let within = lx >= 0.0 && lx <= w && ly >= 0.0 && ly <= h;
        Some(((lx * sf, ly * sf), within))
    }
}

impl EventEmitter<PanelEvent> for ChartPanel {}
impl Focusable for ChartPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartPanel {
    fn panel_name(&self) -> &'static str {
        "Chart"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.title_text())
    }
}
impl ChartPanel {
    /// Подпись вкладки: «Чарт N» для AddToChart, иначе рынок открытой монеты
    /// (из контейнера — авторитетно), затем self.market, затем «Main».
    pub fn title_text(&self) -> String {
        if let Some(n) = self.num {
            return format!("Чарт {n}");
        }
        self.chart
            .active_market()
            .filter(|m| !m.is_empty())
            .or_else(|| self.market.clone())
            .unwrap_or_else(|| "Main".into())
    }
}
impl Render for ChartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ppp = window.scale_factor();
        self.chart.resize(self.chart_dev.0, self.chart_dev.1);

        let (theme, orders_style, scale, follow) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            (eff.theme.clone(), eff.orders.clone(), b.price_scale, b.follow)
        };
        if self.chart.set_theme(theme) {
            self.chart_dirty = true;
        }
        if self.chart.set_orders(orders_style) {
            self.chart_dirty = true;
        }
        // Масштаб цены и Live/Пауза — из ScalePanel тулбара (живут в Backend).
        if self.chart.set_scale(scale) {
            self.chart_dirty = true;
        }
        if self.chart.set_follow(follow, now_unix_ms()) {
            self.chart_dirty = true;
        }

        if self.chart_dirty || self.chart_img.is_none() {
            if let Some(old) = self.chart_img.take() {
                cx.drop_image(old, Some(window));
            }
            let (img_arc, layout) = {
                let b = self.backend.read(cx);
                self.chart.render(&b.session, ppp)
            };
            self.input.pane_rects = layout;
            self.chart_img = Some(img_arc);
            self.chart_dirty = false;
        }
        let chart_img = self.chart_img.clone().expect("chart image");
        let axis_panes = self.chart.axis_panes(axes::local_offset_sec());
        let cross = self.chart.crosshair_style();
        let cursor_dev = self.input.cursor;
        let hovered = self.input.hovered_pane;

        div()
            .id("chart-slot")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .relative()
            .track_focus(&self.focus)
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| {
                if cx.has_active_drag() { return; } // идёт drag Dock-панели — не мешаем drop
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                let dy = match e.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
                };
                this.input.hovered_pane = this.input.pane_at(pos.0, pos.1);
                let fb = this.chart_dev.0 as f32;
                if this.input.wheel(dy, e.modifiers.shift, within, &mut this.chart.container, fb) {
                    this.chart_dirty = true;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                if cx.has_active_drag() { return; }
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                this.input.last_ptr = pos;
                // На AddToChart-вкладках дабл-клик по ЧАРТУ (не стакану) → открыть ту
                // монету/ядро на Main (fullscreen). Main себе это не делает.
                let allow_to_main = this.num.is_some();
                this.input.mouse_button(input::Btn::Left, true, within, allow_to_main, &mut this.chart.container);
                if let Some((core, market)) = this.input.pending_to_main.take() {
                    this.backend.update(cx, |b, bcx| {
                        b.open_request = Some((core, market));
                        bcx.notify();
                    });
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _e: &MouseUpEvent, _window, cx| {
                this.input.mouse_button(input::Btn::Left, false, false, false, &mut this.chart.container);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                this.input.last_ptr = pos;
                if this.input.mouse_button(input::Btn::Right, true, within, false, &mut this.chart.container) {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, _e: &MouseUpEvent, _window, cx| {
                this.input.mouse_button(input::Btn::Right, false, false, false, &mut this.chart.container);
                this.chart_dirty = true;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if cx.has_active_drag() { return; } // идёт drag Dock-панели — не перехватываем
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                this.input.sync_pressed(
                    e.pressed_button == Some(MouseButton::Left),
                    e.pressed_button == Some(MouseButton::Right),
                );
                this.input.cursor = if within { Some(pos) } else { None };
                this.input.hovered_pane = if within { this.input.pane_at(pos.0, pos.1) } else { None };
                if this.input.pointer_drag(pos.0, pos.1, &mut this.chart.container) {
                    this.chart_dirty = true;
                }
                cx.notify();
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.input.cursor = None;
                    this.input.hovered_pane = None;
                    cx.notify();
                }
            }))
            .child(img(chart_img).absolute().size_full())
            .child({
                let entity = cx.entity();
                let measured = self.chart_dev;
                let prev_bounds = self.chart_bounds;
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let sf = window.scale_factor();
                        let dev = (
                            (f32::from(bounds.size.width) * sf).round().max(1.0) as u32,
                            (f32::from(bounds.size.height) * sf).round().max(1.0) as u32,
                        );
                        if dev != measured || Some(bounds) != prev_bounds {
                            entity.update(cx, |this, cx| {
                                this.chart_bounds = Some(bounds);
                                this.chart_dev = dev;
                                this.chart_dirty = true;
                                cx.notify();
                            });
                        }
                        // Оси/перекрестие — ПО КАЖДОЙ панели (Tiled-мультичарт): свой
                        // прямоугольник (девайс-px → лог.px окна) и снимок. Курсор —
                        // только для панели под мышью.
                        for (idx, rect, snap) in &axis_panes {
                            let sub = Bounds::new(
                                point(
                                    bounds.origin.x + px(rect.x / sf),
                                    bounds.origin.y + px(rect.y / sf),
                                ),
                                gpui::size(px(rect.w / sf), px(rect.h / sf)),
                            );
                            let cursor = if hovered == Some(*idx) {
                                cursor_dev.map(|(x, y)| {
                                    point(bounds.origin.x + px(x / sf), bounds.origin.y + px(y / sf))
                                })
                            } else {
                                None
                            };
                            axes::draw(window, cx, sub, snap, cursor, sf, cross);
                        }
                    },
                )
                .absolute()
                .size_full()
            })
    }
}

/// Панель ордера (right dock): BUY/SELL/Cancel/Panic. Порт egui `dock/order.rs`.
pub struct OrderPanel {
    focus: FocusHandle,
}
impl OrderPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle() }
    }
}
impl EventEmitter<PanelEvent> for OrderPanel {}
impl Focusable for OrderPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrderPanel {
    fn panel_name(&self) -> &'static str {
        "Order"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Ордер")
    }
}
impl Render for OrderPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("order-panel")
            .size_full()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            .child(Button::new("buy").success().label("BUY").on_click(|_, _, _| log::info!("BUY")))
            .child(Button::new("sell").danger().label("SELL").on_click(|_, _, _| log::info!("SELL")))
            .child(Button::new("cancel").warning().label("Cancel Buy").on_click(|_, _, _| log::info!("Cancel")))
            .child(Button::new("panic").danger().label("PANIC SELL").on_click(|_, _, _| log::info!("PANIC")))
    }
}
