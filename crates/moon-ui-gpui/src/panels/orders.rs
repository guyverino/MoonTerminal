//! Панель «Ордера» — таблица открытых ордеров группы (все ядра), на всю ширину.
//! Порт egui `src/dock/orders_panel.rs`. Колонки: Ядро · Сторона · Токен · Size ·
//! SL · TS · Vstop · Bprice · CurPrice · Fill · Strat.
//!
//! Сторона: BUY (лонг, ждёт) — зелёным, SHORT (шорт, ждёт) — красным, SELL
//! (исполнился — позиция открыта/продаётся) — синим. Эмуляторный — «(E)».
//!
//! Клик по колонке токена открывает чарт монеты на Main НА ЯДРЕ ордера (через
//! `Backend.open_request`). Поля-списки источника/типа + меню сортировки/фильтра.

use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState, PanelView, TabPanel},
    h_flex,
    popover::Popover,
    v_flex, Sizable,
};

use crate::detached::DetachedSpec;
use crate::{hex, Backend};
use moon_core::feed::OrderRow;
use moon_core::palette;
use moon_core::session::CoreId;
use moon_core::symbol;

/// SELL (исполненный лонг) — синий (egui `theme::BLUE`, нет в палитре).
const BLUE: u32 = 0x4a90ff;

/// Одна строка таблицы ордеров с привязкой к ядру-источнику (порт `OrderEntry`).
#[derive(Clone)]
struct OrderEntry {
    core: CoreId,
    core_name: String,
    quote: String,
    row: OrderRow,
}

/// Первичный ключ сортировки (тогл-группа в меню).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PrimarySort {
    SellFirst,
    BuyFirst,
    Creation,
}

/// Источник ордеров: все ядра группы или конкретное ядро.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OrdersSource {
    All,
    Core(CoreId),
}

/// Фильтр по типу ордера: все / реальные / эмуляторные.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OrderKind {
    All,
    Real,
    Emu,
}

/// Состояние вида таблицы (источник + тип + фильтр + сортировка). Своё у панели.
#[derive(Clone, Copy, PartialEq, Eq)]
struct OrdersViewState {
    source: OrdersSource,
    kind: OrderKind,
    only_current_market: bool,
    primary: PrimarySort,
    newest_first: bool,
}

impl Default for OrdersViewState {
    fn default() -> Self {
        Self {
            source: OrdersSource::All,
            kind: OrderKind::All,
            only_current_market: false,
            primary: PrimarySort::Creation,
            newest_first: true,
        }
    }
}

/// Вход (покупка) заполнен.
fn executed(r: &OrderRow) -> bool {
    r.fill_pct >= 99.95
}
/// SELL — исполненный ЛОНГ (куплен и выставлен на продажу). НЕ шорт.
fn is_sell(r: &OrderRow) -> bool {
    !r.is_short && executed(r)
}
/// BUY — лонг, ещё не исполнен (ждёт покупки).
fn is_buy(r: &OrderRow) -> bool {
    !r.is_short && !executed(r)
}

/// Панель «Ордера».
pub struct OrdersPanel {
    backend: Entity<Backend>,
    group: String,
    view: OrdersViewState,
    tab: Option<WeakEntity<TabPanel>>,
    focus: FocusHandle,
}

impl OrdersPanel {
    pub fn new(backend: Entity<Backend>, group: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Перерисовка по дренажу backend (новые ордера/цены).
        cx.observe(&backend, |_this, _b, cx| cx.notify()).detach();
        Self { backend, group, view: OrdersViewState::default(), tab: None, focus: cx.focus_handle() }
    }

    /// Открытые ордера ядер группы (с именем ядра и quote) — порт `collect_orders`.
    fn collect(&self, b: &Backend) -> Vec<OrderEntry> {
        let store = b.session.store();
        let mut rows = Vec::new();
        for s in b.session.sessions().iter().filter(|s| s.group == self.group) {
            let quote = b
                .config
                .servers
                .iter()
                .find(|sv| sv.id == s.id)
                .map(|sv| symbol::resolve_quote(&sv.market))
                .unwrap_or_default();
            if let Some(d) = store.core(s.id) {
                for o in &d.orders {
                    rows.push(OrderEntry { core: s.id, core_name: s.name.clone(), quote: quote.clone(), row: o.clone() });
                }
            }
        }
        rows
    }

    /// (ядро, маркет) монеты, открытой на Main группы — для фильтра «только текущий».
    fn current_market(&self, b: &Backend) -> Option<(CoreId, String)> {
        let focus = b.session.sessions().iter().find(|s| s.group == self.group).map(|s| s.id)?;
        b.desired.iter().find(|(id, _)| *id == focus).map(|(c, m)| (*c, m.clone()))
    }

    /// Имена ядер группы (id, имя) — для поля-списка источника.
    fn group_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        b.session.sessions().iter().filter(|s| s.group == self.group).map(|s| (s.id, s.name.clone())).collect()
    }

    fn set_source(&mut self, s: OrdersSource, cx: &mut Context<Self>) {
        self.view.source = s;
        cx.notify();
    }
    fn set_kind(&mut self, k: OrderKind, cx: &mut Context<Self>) {
        self.view.kind = k;
        cx.notify();
    }

    /// Поле-список источника (Все ядра + ядра группы) — порт egui ComboBox.
    fn source_combo(&self, cores: &[(CoreId, String)], cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.view.source {
            OrdersSource::All => "Все ядра".to_string(),
            OrdersSource::Core(id) => cores.iter().find(|(c, _)| *c == id).map(|(_, n)| n.clone()).unwrap_or_else(|| "Все ядра".into()),
        };
        let view = cx.entity();
        let cores = cores.to_vec();
        Popover::new("orders-source")
            .trigger(Button::new("orders-source-btn").outline().xsmall().label(format!("{cur} ▾")))
            .content(move |_s, _w, _cx| {
                let view = view.clone();
                let mut col = v_flex().gap_0p5().p_1().min_w(px(150.0));
                col = col.child(combo_item("os-all", "Все ядра", {
                    let view = view.clone();
                    move |app| view.update(app, |t, c| t.set_source(OrdersSource::All, c))
                }));
                for (id, name) in &cores {
                    let id = *id;
                    let view = view.clone();
                    col = col.child(combo_item(format!("os-{id}"), name.clone(), move |app| {
                        view.update(app, |t, c| t.set_source(OrdersSource::Core(id), c))
                    }));
                }
                col
            })
    }

    /// Поле-список типа ордеров (Все / Реальные / Эмуляторные).
    fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.view.kind {
            OrderKind::All => "Все",
            OrderKind::Real => "Реальные",
            OrderKind::Emu => "Эмуляторные",
        };
        let view = cx.entity();
        Popover::new("orders-kind")
            .trigger(Button::new("orders-kind-btn").outline().xsmall().label(format!("{cur} ▾")))
            .content(move |_s, _w, _cx| {
                let opts = [(OrderKind::All, "Все"), (OrderKind::Real, "Реальные"), (OrderKind::Emu, "Эмуляторные")];
                let mut col = v_flex().gap_0p5().p_1().min_w(px(130.0));
                for (k, label) in opts {
                    let view = view.clone();
                    col = col.child(combo_item(format!("ok-{label}"), label, move |app| {
                        view.update(app, |t, c| t.set_kind(k, c))
                    }));
                }
                col
            })
    }

    /// Меню сортировки/фильтра (порт ПКМ-меню egui): фильтр текущего маркета + две
    /// тогл-группы сортировки. В GPUI — попап-кнопка (PopupMenu основан на Action).
    fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        Popover::new("orders-sort")
            .trigger(Button::new("orders-sort-btn").ghost().xsmall().label("⚙ ▾").tooltip("Сортировка и фильтр"))
            .content(move |_s, _w, _cx| {
                let v = view.clone();
                let mut col = v_flex().gap_0p5().p_1().min_w(px(200.0));
                col = col.child(menu_check("m-onlycur", "Только ордера текущего маркета", cur.only_current_market, {
                    let v = v.clone();
                    move |app| v.update(app, |t, c| { t.view.only_current_market = true; c.notify(); })
                }));
                col = col.child(menu_check("m-showall", "Показать все", !cur.only_current_market, {
                    let v = v.clone();
                    move |app| v.update(app, |t, c| { t.view.only_current_market = false; c.notify(); })
                }));
                col = col.child(menu_sep());
                for (variant, label, id) in [
                    (PrimarySort::SellFirst, "Sell первые", "m-sell"),
                    (PrimarySort::BuyFirst, "Buy первые", "m-buy"),
                    (PrimarySort::Creation, "По созданию ордера", "m-creation"),
                ] {
                    let v = v.clone();
                    col = col.child(menu_check(id, label, cur.primary == variant, move |app| {
                        v.update(app, |t, c| { t.view.primary = variant; c.notify(); })
                    }));
                }
                col = col.child(menu_sep());
                col = col.child(menu_check("m-new", "Новые первые", cur.newest_first, {
                    let v = v.clone();
                    move |app| v.update(app, |t, c| { t.view.newest_first = true; c.notify(); })
                }));
                col = col.child(menu_check("m-old", "Старые первые", !cur.newest_first, {
                    let v = v.clone();
                    move |app| v.update(app, |t, c| { t.view.newest_first = false; c.notify(); })
                }));
                col
            })
    }
}

/// Ключ группировки по ОТОБРАЖАЕМОЙ стороне (с учётом исполнения → SELL). 0 = выше.
fn primary_key(p: PrimarySort, r: &OrderRow) -> u8 {
    match p {
        PrimarySort::Creation => 0,
        PrimarySort::SellFirst => u8::from(!is_sell(r)),
        PrimarySort::BuyFirst => u8::from(!is_buy(r)),
    }
}

fn sort_entries(entries: &mut [OrderEntry], view: &OrdersViewState) {
    entries.sort_by(|a, b| {
        let ka = primary_key(view.primary, &a.row);
        let kb = primary_key(view.primary, &b.row);
        ka.cmp(&kb).then_with(|| {
            let c = a.row.uid.cmp(&b.row.uid);
            if view.newest_first { c.reverse() } else { c }
        })
    });
}

fn num(v: f64) -> String {
    moon_core::util::fmt::adaptive(v)
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
    fn on_added_to(&mut self, tab_panel: WeakEntity<TabPanel>, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab = Some(tab_panel);
    }
    fn toolbar_buttons(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let tab = self.tab.clone();
        let me = cx.entity().downgrade();
        Some(vec![Button::new("detach-orders")
            .ghost()
            .label("⧉")
            .tooltip("В отдельное окно")
            .on_click(move |_, window, app| {
                if let (Some(tab), Some(me)) = (tab.as_ref().and_then(|t| t.upgrade()), me.upgrade()) {
                    let arc: Arc<dyn PanelView> = Arc::new(me);
                    tab.update(app, |tp, cx| tp.remove_panel(arc, window, cx));
                }
                let spec = DetachedSpec::new(group.clone(), "Orders".to_string());
                crate::detached::spawn(app, &backend, &spec);
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            })])
    }
}

impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let b = self.backend.read(cx);
        let cores = self.group_cores(b);
        let current = self.current_market(b);
        let mut entries = self.collect(b);

        // Фильтрация (источник / тип / только текущий маркет).
        let view = self.view;
        entries.retain(|e| {
            let by_source = match view.source {
                OrdersSource::All => true,
                OrdersSource::Core(id) => e.core == id,
            };
            let by_kind = match view.kind {
                OrderKind::All => true,
                OrderKind::Real => !e.row.emulator,
                OrderKind::Emu => e.row.emulator,
            };
            by_source
                && by_kind
                && (!view.only_current_market
                    || match &current {
                        Some((c, m)) => e.core == *c && &e.row.market == m,
                        None => true,
                    })
        });
        sort_entries(&mut entries, &view);
        let shown = entries.len();

        // ── Панель управления ──
        let mut controls = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.source_combo(&cores, cx))
            .child(self.kind_combo(cx))
            .child(self.sort_menu(cx))
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_3))).child(format!("{shown}")));
        if view.only_current_market {
            controls = controls
                .child(div().text_xs().text_color(rgb(hex(palette::TEXT_3))).child("· Только ордера текущего маркета"));
        }

        // ── Заголовок строк (тело) ──
        let mut list = v_flex().w_full();
        if entries.is_empty() {
            list = list.child(div().p_2().text_color(rgb(hex(palette::TEXT_2))).child("нет открытых ордеров"));
        } else {
            for (i, e) in entries.iter().enumerate() {
                list = list.child(data_row(e, i, cx));
            }
        }

        v_flex()
            .id("orders-panel")
            .size_full()
            .track_focus(&self.focus)
            .text_sm()
            .child(controls)
            .child(div().w_full().h(px(1.0)).bg(rgb(hex(palette::LIFT_HOVER))))
            .child(div().id("orders-scroll").flex_1().w_full().overflow_y_scroll().child(list))
    }
}

/// Ширины колонок (px). Strat растягивается (flex), остальные фиксированы.
struct W;
impl W {
    const CORE: f32 = 80.0;
    const SIDE: f32 = 74.0;
    const TOKEN: f32 = 70.0;
    const SIZE: f32 = 92.0;
    const SL: f32 = 64.0;
    const TS: f32 = 64.0;
    const VSTOP: f32 = 80.0;
    const BUY: f32 = 104.0;
    const PRICE: f32 = 110.0;
    const FILL: f32 = 72.0;
}

/// Одна строка ордера (полоски через чётность). Токен — кликабелен (открыть чарт).
fn data_row(e: &OrderEntry, idx: usize, cx: &Context<OrdersPanel>) -> AnyElement {
    let r = &e.row;
    // Сторона + цвет.
    let (side, col) = if is_sell(r) {
        ("SELL", rgb(BLUE))
    } else if r.is_short {
        ("SHORT", rgb(hex(palette::RED)))
    } else {
        ("BUY", rgb(hex(palette::GREEN)))
    };
    let side = if r.emulator { format!("{side}(E)") } else { side.to_string() };
    let token = symbol::base_symbol(&r.market, &e.quote).to_string();
    let (core, market) = (e.core, r.market.clone());

    let token_cell = div()
        .id(SharedString::from(format!("ord-tok-{}-{}", e.core, r.uid)))
        .w(px(W::TOKEN))
        .flex_none()
        .cursor_pointer()
        .truncate()
        .text_right()
        .text_color(rgb(hex(palette::ACCENT)))
        .child(token)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.backend.update(cx, |b, bcx| {
                b.open_request = Some((core, market.clone()));
                bcx.notify();
            });
        }));

    let bg = if idx % 2 == 1 { Some(rgb(hex(palette::LIFT))) } else { None };
    let mut row = h_flex().w_full().items_center().gap_2().px_2().py(px(1.0));
    if let Some(bg) = bg {
        row = row.bg(bg);
    }
    row.child(cell_text(W::CORE, Align::Start, &e.core_name, rgb(hex(palette::TEXT_2))))
        .child(cell_text(W::SIDE, Align::Start, &side, col))
        .child(token_cell)
        .child(cell_lv(W::SIZE, Align::End, "Sz:", &num(r.size), rgb(hex(palette::TEXT_2))))
        .child(cell_onoff(W::SL, "SL:", r.sl_on))
        .child(cell_onoff(W::TS, "TS:", r.ts_on))
        .child(cell_onoff(W::VSTOP, "Vstop:", r.vstop_on))
        .child(cell_lv(W::BUY, Align::End, "Buy:", &num(r.buy_price), rgb(hex(palette::TEXT_2))))
        .child(cell_lv(W::PRICE, Align::End, "Cur.P:", &num(r.price as f64), rgb(hex(palette::TEXT_2))))
        .child(cell_lv(W::FILL, Align::End, "Fill:", &format!("{:.0}%", r.fill_pct), rgb(hex(palette::TEXT_2))))
        // Strat растягивается на остаток ширины (право, одна строка).
        .child(div().flex_1().min_w(px(80.0)).truncate().text_right().text_color(rgb(hex(palette::TEXT_2))).child(r.strat.clone()))
        .into_any_element()
}

#[derive(Clone, Copy)]
enum Align {
    Start,
    Center,
    End,
}

fn justify(d: Div, al: Align) -> Div {
    match al {
        Align::Start => d.justify_start(),
        Align::Center => d.justify_center(),
        Align::End => d.justify_end(),
    }
}

/// Ячейка с одноцветным текстом фикс. ширины (одна строка, обрезка «…»).
fn cell_text(w: f32, al: Align, text: &str, color: Rgba) -> impl IntoElement {
    let d = match al {
        Align::Start => div().text_left(),
        Align::Center => div().text_center(),
        Align::End => div().text_right(),
    };
    d.w(px(w)).flex_none().truncate().text_color(color).child(text.to_string())
}

/// Ячейка «подпись:значение» (подпись тусклая, значение своим цветом).
fn cell_lv(w: f32, al: Align, label: &str, value: &str, vcolor: Rgba) -> impl IntoElement {
    let inner = h_flex()
        .gap_0()
        .child(div().text_color(rgb(hex(palette::TEXT_3))).child(label.to_string()))
        .child(div().text_color(vcolor).child(value.to_string()));
    justify(div().w(px(w)).flex_none().flex().overflow_hidden(), al).child(inner)
}

/// Ячейка флага по центру: «SL:ON» (зелёным) / «SL:OFF» (тускло).
fn cell_onoff(w: f32, label: &str, on: bool) -> impl IntoElement {
    let (v, c) = if on { ("ON", rgb(hex(palette::GREEN))) } else { ("OFF", rgb(hex(palette::TEXT_2))) };
    cell_lv(w, Align::Center, label, v, c)
}

/// Кликабельный пункт попап-комбобокса.
fn combo_item(id: impl Into<SharedString>, label: impl Into<SharedString>, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    div()
        .id(id.into())
        .w_full()
        .px_2()
        .py_1()
        .cursor_pointer()
        .rounded(px(3.0))
        .text_color(rgb(hex(palette::TEXT)))
        .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
        .child(label.into())
        .on_click(move |_, _w, app| on_click(app))
}

/// Пункт меню с галкой-маркером слева (selectable_label в egui).
fn menu_check(id: impl Into<SharedString>, label: impl Into<SharedString>, checked: bool, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let mark = if checked { "✓ " } else { "   " };
    let col = if checked { palette::ACCENT } else { palette::TEXT };
    div()
        .id(id.into())
        .w_full()
        .px_2()
        .py_1()
        .cursor_pointer()
        .rounded(px(3.0))
        .text_color(rgb(hex(col)))
        .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
        .child(format!("{mark}{}", label.into()))
        .on_click(move |_, _w, app| on_click(app))
}

fn menu_sep() -> impl IntoElement {
    div().my_0p5().w_full().h(px(1.0)).bg(rgb(hex(palette::LIFT_HOVER)))
}

/// Открытые ордера всех ядер группы — для статус-бара Shell (число ордеров).
pub fn count_orders(b: &Backend, group: &str) -> usize {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .map(|c| c.orders.len())
        .sum()
}
