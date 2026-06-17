//! Панель «Ордера» — таблица открытых ордеров группы (все ядра), на всю ширину.
//! Визуальная геометрия выведена из HTML-эталона MoonBot Terminal Design:
//! MARKET · SIDE · STATUS · QTY · PRICE · FILLED · PNL · TP · SL · AGE · STRATEGY.
//!
//! Сторона: BUY (лонг, ждёт) — зелёным, SHORT (шорт, ждёт) — красным, SELL
//! (исполнился — позиция открыта/продаётся) — синим. Эмуляторный — «(E)».
//!
//! Клик по колонке токена открывает чарт монеты на Main НА ЯДРЕ ордера (через
//! `Backend.open_request`). Поля-списки источника/типа + меню сортировки/фильтра.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_palette::{
    DockArea, MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize,
    MoonButtonVariant, MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDropdown,
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonText, MoonTone, Panel, PanelEvent, PanelState,
    h_flex, v_flex,
};

use crate::Backend;
use crate::design;
use crate::detached::DetachedSpec;
use moon_core::feed::OrderRow;
use moon_core::session::CoreId;
use moon_core::symbol;

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
    /// Сигнатура ордеров прошлого кадра (сумма orders_rev ядер группы) — чтобы НЕ
    /// перестраивать таблицу каждые 100мс на холостом ходу (иначе вместе с readback
    /// чарта это перегружает UI-поток → рывки графика, когда вкладка «Ордера» активна).
    last_sig: u64,
    /// Секундное ведро для ~1 Гц обновления свежести цен/P&L (они живут от рынка, а
    /// orders_sig на тик цены не реагирует). Перерисовка: эпоха/статус ордера ИЛИ раз в сек.
    last_sec: u64,
    /// Время последней перерисовки (unix мс) — пол 250мс: ордерные ивенты летят часто,
    /// глаз всё равно не успеет, поэтому таблицу обновляем НЕ ЧАЩЕ 4 Гц. Исключение-гейт.
    last_notify_ms: f64,
    /// Армирован ли 1-Гц тик колонки «Age» (возраст ордера живёт по часам, не по данным).
    age_timer_armed: bool,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl OrdersPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Перерисовка по дренажу backend — ТОЛЬКО когда реально изменились ордера.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::ORDERS_OBS_FIRE);
            let now = moon_chart::paint::now_unix_ms();
            let sig = orders_sig(backend.read(cx), &this.group);
            // ~1 Гц тик: цены/P&L в таблице живут от рынка, orders_sig их не ловит.
            let sec = (now as u64) / 1000;
            // Перерисовка: смена эпохи/статуса ордера ИЛИ раз в сек (цены), НО НЕ ЧАЩЕ
            // 250мс — коалесцируем частые ордерные ивенты (глаз их всё равно не различит).
            let changed = sig != this.last_sig || sec != this.last_sec;
            if changed && now - this.last_notify_ms >= 250.0 {
                this.last_sig = sig;
                this.last_sec = sec;
                this.last_notify_ms = now;
                crate::diag::bump(&crate::diag::ORDERS_OBS_NOTIFY);
                cx.notify();
            }
            // Появились ордера → завести Age-тик (идемпотентно; гаснет сам, когда их нет).
            this.arm_age_timer(cx);
        })
        .detach();
        let mut this = Self {
            backend,
            group,
            view: OrdersViewState::default(),
            last_sig: 0,
            last_sec: 0,
            last_notify_ms: 0.0,
            age_timer_armed: false,
            dock: None,
            focus: cx.focus_handle(),
        };
        this.arm_age_timer(cx);
        this
    }

    /// 1-Гц тик для колонки «Age» (возраст ордера): живёт по СВОИМ часам, а не по приходу
    /// данных. На молчащем рынке backend-пульса нет → без этого Age замирал бы. Тикает лишь
    /// пока в группе есть ордера; гаснет, когда их нет (не плодим таймер вхолостую).
    fn arm_age_timer(&mut self, cx: &mut Context<Self>) {
        if self.age_timer_armed || count_orders(self.backend.read(cx), &self.group) == 0 {
            return;
        }
        self.age_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(std::time::Duration::from_millis(1000)).await;
            let alive = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.age_timer_armed = false;
                    if count_orders(this.backend.read(cx), &this.group) > 0 {
                        cx.notify();
                        this.arm_age_timer(cx);
                    }
                })
                .is_ok()
            });
            if !alive {
                return;
            }
        })
        .detach();
    }

    /// Открытые ордера ядер группы (с именем ядра и quote) — порт `collect_orders`.
    fn collect(&self, b: &Backend) -> Vec<OrderEntry> {
        let store = b.session.store();
        let mut rows = Vec::new();
        for s in b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
        {
            let quote = b
                .config
                .servers
                .iter()
                .find(|sv| sv.id == s.id)
                .map(|sv| symbol::resolve_quote(&sv.market))
                .unwrap_or_default();
            if let Some(d) = store.core(s.id) {
                for o in &d.orders {
                    rows.push(OrderEntry {
                        core: s.id,
                        core_name: s.name.clone(),
                        quote: quote.clone(),
                        row: o.clone(),
                    });
                }
            }
        }
        rows
    }

    /// (ядро, маркет) монеты, открытой на Main группы — для фильтра «только текущий».
    fn current_market(&self, b: &Backend) -> Option<(CoreId, String)> {
        let focus = b
            .session
            .sessions()
            .iter()
            .find(|s| s.group == self.group)
            .map(|s| s.id)?;
        b.desired
            .iter()
            .find(|(id, _)| *id == focus)
            .map(|(c, m)| (*c, m.clone()))
    }

    /// Имена ядер группы (id, имя) — для поля-списка источника.
    fn group_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    fn set_source(&mut self, s: OrdersSource, cx: &mut Context<Self>) {
        if self.view.source != s {
            self.view.source = s;
            cx.notify();
        }
    }
    fn set_kind(&mut self, k: OrderKind, cx: &mut Context<Self>) {
        if self.view.kind != k {
            self.view.kind = k;
            cx.notify();
        }
    }

    /// Поле-список источника (Все ядра + ядра группы) — порт egui ComboBox.
    fn source_combo(&self, cores: &[(CoreId, String)], cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.view.source {
            OrdersSource::All => "Все ядра".to_string(),
            OrdersSource::Core(id) => cores
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "Все ядра".into()),
        };
        let view = cx.entity();
        let mut menu = MoonDropdown::new("orders-source")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(118.0)
            .menu_width(160.0)
            .menu_size(MoonMenuSize::Compact)
            .item(
                MoonMenuItem::with_key("all", "Все ядра")
                    .checked(matches!(self.view.source, OrdersSource::All))
                    .on_click({
                        let view = view.clone();
                        move |_, _, app| view.update(app, |t, c| t.set_source(OrdersSource::All, c))
                    }),
            );
        for (id, name) in cores {
            let id = *id;
            let selected = matches!(self.view.source, OrdersSource::Core(cur) if cur == id);
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("core-{id}"), name.clone())
                    .checked(selected)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_source(OrdersSource::Core(id), c))
                    }),
            );
        }
        menu
    }

    /// Поле-список типа ордеров (Все / Реальные / Эмуляторные).
    fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.view.kind {
            OrderKind::All => "Все",
            OrderKind::Real => "Реальные",
            OrderKind::Emu => "Эмуляторные",
        };
        let view = cx.entity();
        let mut menu = MoonDropdown::new("orders-kind")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(102.0)
            .menu_width(138.0)
            .menu_size(MoonMenuSize::Compact);
        for (k, label) in [
            (OrderKind::All, "Все"),
            (OrderKind::Real, "Реальные"),
            (OrderKind::Emu, "Эмуляторные"),
        ] {
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("kind-{label}"), label)
                    .checked(self.view.kind == k)
                    .on_click(move |_, _, app| view.update(app, |t, c| t.set_kind(k, c))),
            );
        }
        menu
    }

    /// Меню сортировки/фильтра (порт ПКМ-меню egui): фильтр текущего маркета + две
    /// тогл-группы сортировки. В GPUI — попап-кнопка (PopupMenu основан на Action).
    fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let v = view.clone();
        let mut menu = MoonDropdown::new("orders-sort")
            .label("⚙")
            .trigger_variant(MoonButtonVariant::Ghost)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(34.0)
            .menu_width(220.0)
            .menu_size(MoonMenuSize::Normal)
            .item(
                MoonMenuItem::with_key("m-onlycur", "Только ордера текущего маркета")
                    .checked(cur.only_current_market)
                    .on_click(move |_, _, app| {
                        v.update(app, |t, c| {
                            if !t.view.only_current_market {
                                t.view.only_current_market = true;
                                c.notify();
                            }
                        })
                    }),
            );
        let v = view.clone();
        menu = menu
            .item(
                MoonMenuItem::with_key("m-showall", "Показать все")
                    .checked(!cur.only_current_market)
                    .on_click(move |_, _, app| {
                        v.update(app, |t, c| {
                            if t.view.only_current_market {
                                t.view.only_current_market = false;
                                c.notify();
                            }
                        })
                    }),
            )
            .item(MoonMenuItem::separator());
        for (variant, label, id) in [
            (PrimarySort::SellFirst, "Sell первые", "m-sell"),
            (PrimarySort::BuyFirst, "Buy первые", "m-buy"),
            (PrimarySort::Creation, "По созданию ордера", "m-creation"),
        ] {
            let v = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(id, label)
                    .checked(cur.primary == variant)
                    .on_click(move |_, _, app| {
                        v.update(app, |t, c| {
                            if t.view.primary != variant {
                                t.view.primary = variant;
                                c.notify();
                            }
                        })
                    }),
            );
        }
        let v = view.clone();
        menu = menu.item(MoonMenuItem::separator()).item(
            MoonMenuItem::with_key("m-new", "Новые первые")
                .checked(cur.newest_first)
                .on_click(move |_, _, app| {
                    v.update(app, |t, c| {
                        if !t.view.newest_first {
                            t.view.newest_first = true;
                            c.notify();
                        }
                    })
                }),
        );
        let v = view;
        menu.item(
            MoonMenuItem::with_key("m-old", "Старые первые")
                .checked(!cur.newest_first)
                .on_click(move |_, _, app| {
                    v.update(app, |t, c| {
                        if t.view.newest_first {
                            t.view.newest_first = false;
                            c.notify();
                        }
                    })
                }),
        )
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
    // × не удаляет панель, а возвращает её в нижнюю строку (см. Shell: PanelCloseRequested).
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    // Вынесенная в split одиночная панель показывает заголовок (drag-ручка + ×), иначе у неё
    // нет ни места тянуть, ни кнопки закрыть.
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Ордера")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Orders", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let dock = self.dock.clone();
        Some(vec![
            MoonButton::new("detach-orders")
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .on_click(move |_, window, app| {
                    if let Some(dock) = dock.as_ref().and_then(|d| d.upgrade()) {
                        dock.update(app, |area, cx| {
                            area.remove_panel_by_name("Orders", window, cx);
                        });
                    }
                    let spec = DetachedSpec::new(group.clone(), "Orders".to_string());
                    crate::detached::spawn(app, &backend, &spec);
                    backend.update(app, |b, _| {
                        b.detached.push(spec);
                        b.detached_dirty = true;
                    });
                })
                .render()
                .into_any_element(),
        ])
    }
}

impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ORDERS_RENDER);
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
        let p = MoonPalette::active(cx);

        // ── Панель управления ──
        let mut controls = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.source_combo(&cores, cx))
            .child(self.kind_combo(cx))
            .child(self.sort_menu(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child(format!("{shown}")),
            );
        if view.only_current_market {
            controls = controls.child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child("· Только ордера текущего маркета"),
            );
        }

        // ── Виртуальная таблица в геометрии HTML-эталона ──
        let table = orders_table(entries, cx);

        v_flex()
            .id("orders-panel")
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(px(10.5))
            .bg(rgb(p.table_body))
            .child(controls)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(table)
    }
}

fn orders_table(entries: Vec<OrderEntry>, cx: &Context<OrdersPanel>) -> impl IntoElement {
    let empty = entries.is_empty();
    let rows = Rc::new(entries);
    let row_count = rows.len();
    let view = cx.entity();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    div()
        .id("orders-table-host")
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(rgb(p.table_body))
        .child(
            MoonDataTable::new("orders-table", row_count, move |ix, _window, _app| {
                order_table_row(&table_rows[ix], &view, p)
            })
            .columns(order_columns())
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H),
        )
        .when(empty, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(10.0))
                    .top(px(design::TABLE_HEAD_H))
                    .h(px(design::TABLE_ROW_H))
                    .flex()
                    .items_center()
                    .font_family(design::mono())
                    .text_size(px(10.5))
                    .text_color(rgb(p.text_muted))
                    .child("нет открытых ордеров"),
            )
        })
}

fn order_columns() -> Vec<MoonDataTableColumn> {
    // Widths are logical design pixels: they are minimums when the table is
    // narrow and proportional weights when the table has extra width. Example:
    // 100 grows twice as much as 50, so the table stays full-width without
    // losing the designer's column ratios.
    vec![
        MoonDataTableColumn::new("market", "Market", 100.0),
        MoonDataTableColumn::new("side", "Side", 50.0),
        MoonDataTableColumn::new("status", "Status", 80.0),
        numeric_column("Qty", 60.0),
        numeric_column("Price", 90.0),
        numeric_column("Filled", 60.0),
        numeric_column("PnL", 60.0),
        numeric_column("TP", 80.0),
        numeric_column("SL", 80.0),
        numeric_column("Age", 50.0),
        MoonDataTableColumn::new("strategy", "Strategy", 130.0),
    ]
}

fn numeric_column(title: impl Into<SharedString>, width: f32) -> MoonDataTableColumn {
    let title = title.into();
    MoonDataTableColumn::new(title.to_lowercase(), title, width).right()
}

fn order_table_row(e: &OrderEntry, view: &Entity<OrdersPanel>, p: MoonPalette) -> MoonDataRow {
    let r = &e.row;
    let (side, side_tone) = if is_sell(r) {
        ("SELL", MoonTone::Info)
    } else if r.is_short {
        ("SHORT", MoonTone::Danger)
    } else {
        ("BUY", MoonTone::Accent)
    };
    let side = if r.emulator {
        format!("{side}(E)")
    } else {
        side.to_string()
    };
    let pnl = pnl_value(r);
    let pnl_tone = if pnl >= 0.0 {
        MoonTone::Positive
    } else {
        MoonTone::Danger
    };
    let price = if r.sell_price > 0.0 {
        r.sell_price
    } else {
        r.buy_price
    };
    let strategy = if r.strat.is_empty() {
        e.core_name.clone()
    } else {
        r.strat.clone()
    };

    MoonDataRow::new([
        MoonDataCell::element(market_cell(e, view, p)),
        MoonDataCell::text(side).tone(side_tone).weight(500.0),
        status_cell(r),
        MoonDataCell::text(num(r.size)),
        MoonDataCell::text(num(price)),
        MoonDataCell::text(format!("{:.0}%", r.fill_pct)).tone(MoonTone::Muted),
        MoonDataCell::text(format!("{pnl:+.2}")).tone(pnl_tone),
        MoonDataCell::text(opt_price(r.take_profit)).tone(MoonTone::Info),
        MoonDataCell::text(opt_price(r.stop_loss)).tone(MoonTone::Danger),
        MoonDataCell::text(age(r.create_time_ms)).tone(MoonTone::Muted),
        MoonDataCell::text(strategy)
            .tone(MoonTone::Muted)
            .font_size(10.0)
            .line_height(13.0)
            .text_color(p.text_muted),
    ])
}

fn market_cell(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let base = symbol::base_symbol(&e.row.market, &e.quote);
    let market_label = if e.quote.is_empty() {
        e.row.market.clone()
    } else {
        format!("{base}/{}", e.quote)
    };
    let core = e.core;
    let market = e.row.market.clone();
    let uid = e.row.uid;
    let view = view.clone();

    div()
        .id(SharedString::from(format!("ord-tok-{core}-{uid}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            MoonText::new(market_label)
                .color(p.text)
                .font_size(10.5)
                .line_height(14.0)
                .weight(500.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.open_request = Some((core, market.clone()));
                    b.open_request_rev = b.open_request_rev.wrapping_add(1);
                    bcx.notify();
                });
            });
        })
}

fn status_cell(r: &OrderRow) -> MoonDataCell {
    let (label, tone) = if r.pending {
        ("pending", MoonTone::Notice)
    } else if r.filled || executed(r) {
        ("live", MoonTone::Positive)
    } else if r.emulator {
        ("emu", MoonTone::Muted)
    } else {
        ("open", MoonTone::Muted)
    };
    MoonDataCell::element(
        MoonBadge::new(label)
            .size(MoonBadgeSize::Status)
            .variant(MoonBadgeVariant::Outline)
            .tone(tone)
            .bg_alpha(0.08)
            .border_alpha(0.27)
            .weight(400.0)
            .render(),
    )
}

fn pnl_value(r: &OrderRow) -> f64 {
    if r.buy_price <= 0.0 || r.price <= 0.0 || r.size <= 0.0 {
        return 0.0;
    }
    let cur = r.price as f64;
    let delta = if r.is_short {
        r.buy_price - cur
    } else {
        cur - r.buy_price
    };
    delta * r.size
}

fn opt_price(v: Option<f64>) -> String {
    v.filter(|x| *x > 0.0)
        .map(num)
        .unwrap_or_else(|| "—".into())
}

fn age(create_time_ms: f64) -> String {
    if create_time_ms <= 0.0 {
        return "—".into();
    }
    let secs = ((moon_chart::paint::now_unix_ms() - create_time_ms) * 0.001).max(0.0);
    if secs < 60.0 {
        format!("{secs:.0}s")
    } else if secs < 3600.0 {
        format!("{:.0}m", secs / 60.0)
    } else {
        format!("{:.0}h", secs / 3600.0)
    }
}

/// Сигнатура ордеров группы (сумма orders_rev ядер) — растёт при любом изменении
/// ордеров. Не сменилась → таблицу можно не перестраивать (экономим UI-поток).
fn orders_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.orders_rev))
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
