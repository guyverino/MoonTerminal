//! Панель «Отчёт» — порт egui `src/dock/report_view.rs`. Таблица закрытых сделок
//! (ордеров) из локальной SQLite. Фильтры (ядро/монета/сторона/даты) + выбор колонок
//! сверху, ИТОГО за период снизу, generic-таблица по всем колонкам БД с сортировкой
//! по клику на заголовок. Автообновление по счётчику-генерации writer'а (Backend.reports).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dock::{Panel, PanelEvent, PanelState, PanelView, TabPanel},
    h_flex,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    v_flex, Sizable, StyledExt,
};
use rusqlite::types::Value;
use rusqlite::Connection;

use crate::detached::DetachedSpec;
use crate::{hex, Backend};
use moon_core::db::{self, ReportFilter, ReportTable, SideFilter};
use moon_core::palette;

/// Грузим только топ-N по сортировке (строк в БД может быть очень много).
const ROW_LIMIT: usize = 100;

/// Колонки, видимые по умолчанию (имена = колонки БД).
const DEFAULT_VISIBLE: &[&str] = &[
    "buydate", "closedate", "core_name", "coin", "isshort", "quantity", "buyprice",
    "sellprice", "profitbtc", "lev", "strategyid", "sellreason", "comment",
];

pub struct ReportPanel {
    backend: Entity<Backend>,
    group: String,
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    cores: Vec<(u64, String)>,
    table: ReportTable,
    totals: (f64, i64),

    sort_key: String,
    sort_desc: bool,

    sel_core: usize,
    coin: Entity<InputState>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    side: SideFilter,
    needs_query: bool,

    /// Видимость колонок (параллельно db::DISPLAY_COLUMNS).
    visible: Vec<bool>,
    tab: Option<WeakEntity<TabPanel>>,
    focus: FocusHandle,
}

impl ReportPanel {
    pub fn new(backend: Entity<Backend>, group: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let generation = backend.read(cx).reports.as_ref().map(|h| h.generation.clone());
        let conn = db::open_reader();
        let cores = conn.as_ref().map(db::distinct_cores).unwrap_or_default();
        let last_gen = generation.as_ref().map(|g| g.load(Ordering::Relaxed)).unwrap_or(0);
        let visible = db::DISPLAY_COLUMNS.iter().map(|c| DEFAULT_VISIBLE.contains(c)).collect();
        let (sort_key, sort_desc) = conn.as_ref().and_then(db::load_sort).unwrap_or_else(|| ("buydate".to_string(), true));

        let coin = cx.new(|cx| InputState::new(window, cx).placeholder("все"));
        let from = cx.new(|cx| InputState::new(window, cx).placeholder("ГГГГ-ММ-ДД"));
        let to = cx.new(|cx| InputState::new(window, cx).placeholder("ГГГГ-ММ-ДД"));
        for st in [&coin, &from, &to] {
            cx.subscribe(st, |t, _e, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    t.needs_query = true;
                    cx.notify();
                }
            })
            .detach();
        }
        // Перерисовка — ТОЛЬКО когда writer записал новый отчёт (сменился generation);
        // иначе таблицу не перестраиваем каждые 100мс. Правки фильтров нотифаят сами.
        cx.observe(&backend, |this, _b, cx| {
            if let Some(g) = &this.generation {
                let v = g.load(Ordering::Relaxed);
                if v != this.last_gen {
                    this.last_gen = v;
                    this.needs_query = true;
                    cx.notify();
                }
            }
        })
        .detach();

        Self {
            backend,
            group,
            generation,
            last_gen,
            conn,
            cores,
            table: ReportTable { cols: db::DISPLAY_COLUMNS, rows: Vec::new() },
            totals: (0.0, 0),
            sort_key,
            sort_desc,
            sel_core: 0,
            coin,
            from,
            to,
            side: SideFilter::All,
            needs_query: true,
            visible,
            tab: None,
            focus: cx.focus_handle(),
        }
    }

    fn filter(&self, cx: &App) -> ReportFilter {
        ReportFilter {
            core_uid: if self.sel_core == 0 { None } else { self.cores.get(self.sel_core - 1).map(|(uid, _)| *uid) },
            date_from: db::parse_ymd(&self.from.read(cx).value()),
            date_to: db::parse_ymd(&self.to.read(cx).value()).map(|d| d + 86_399),
            coin: self.coin.read(cx).value().to_string(),
            side: self.side,
        }
    }

    fn poll(&mut self) {
        if let Some(g) = &self.generation {
            let v = g.load(Ordering::Relaxed);
            if v != self.last_gen {
                self.last_gen = v;
                self.needs_query = true;
            }
        }
    }

    fn requery(&mut self, cx: &App) {
        if self.conn.is_none() {
            self.conn = db::open_reader();
        }
        let f = self.filter(cx);
        if let Some(conn) = &self.conn {
            self.cores = db::distinct_cores(conn);
            self.table = db::query_reports(conn, &f, &self.sort_key, self.sort_desc, ROW_LIMIT);
            self.totals = db::query_totals(conn, &f);
        }
        self.needs_query = false;
    }

    fn set_core(&mut self, i: usize, cx: &mut Context<Self>) {
        self.sel_core = i;
        self.needs_query = true;
        cx.notify();
    }
    fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {
        self.side = s;
        self.needs_query = true;
        cx.notify();
    }
    fn click_header(&mut self, col: &str, cx: &mut Context<Self>) {
        if self.sort_key == col {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_key = col.to_string();
            self.sort_desc = true;
        }
        self.needs_query = true;
        if let Some(conn) = &self.conn {
            db::save_sort(conn, &self.sort_key, self.sort_desc);
        }
        cx.notify();
    }

    /// Комбобокс выбора ядра (Все + ядра из БД).
    fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = if self.sel_core == 0 {
            "Все".to_string()
        } else {
            self.cores.get(self.sel_core - 1).map(|(_, n)| n.clone()).unwrap_or_else(|| "Все".into())
        };
        let view = cx.entity();
        let cores = self.cores.clone();
        Popover::new("rep-core")
            .trigger(Button::new("rep-core-btn").outline().xsmall().label(format!("{cur} ▾")))
            .content(move |_s, _w, _cx| {
                let view2 = view.clone();
                let mut col = v_flex().id("rep-core-list").gap_0p5().p_1().min_w(px(150.0)).max_h(px(360.0)).overflow_y_scroll();
                col = col.child(combo_item("rc-all", "Все", {
                    let view = view2.clone();
                    move |app| view.update(app, |t, c| t.set_core(0, c))
                }));
                for (i, (_u, name)) in cores.iter().enumerate() {
                    let view = view2.clone();
                    col = col.child(combo_item(format!("rc-{i}"), name.clone(), move |app| {
                        view.update(app, |t, c| t.set_core(i + 1, c))
                    }));
                }
                col
            })
    }

    /// Комбобокс стороны (Все/Лонг/Шорт).
    fn side_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.side {
            SideFilter::All => "Все",
            SideFilter::Long => "Лонг",
            SideFilter::Short => "Шорт",
        };
        let view = cx.entity();
        Popover::new("rep-side")
            .trigger(Button::new("rep-side-btn").outline().xsmall().label(format!("{cur} ▾")))
            .content(move |_s, _w, _cx| {
                let opts = [(SideFilter::All, "Все"), (SideFilter::Long, "Лонг"), (SideFilter::Short, "Шорт")];
                let mut col = v_flex().gap_0p5().p_1().min_w(px(110.0));
                for (s, label) in opts {
                    let view = view.clone();
                    col = col.child(combo_item(format!("rs-{label}"), label, move |app| {
                        view.update(app, |t, c| t.set_side(s, c))
                    }));
                }
                col
            })
    }

    /// Попап выбора видимых колонок (чекбоксы).
    fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let visible = self.visible.clone();
        Popover::new("rep-cols")
            .trigger(Button::new("rep-cols-btn").outline().xsmall().label("Колонки ▾"))
            .content(move |_s, _w, _cx| {
                let view2 = view.clone();
                let mut col = v_flex().id("rep-cols-list").gap_0p5().p_2().min_w(px(200.0)).max_h(px(420.0)).overflow_y_scroll();
                for (i, c) in db::DISPLAY_COLUMNS.iter().enumerate() {
                    let on = visible.get(i).copied().unwrap_or(false);
                    let view = view2.clone();
                    col = col.child(
                        Checkbox::new(SharedString::from(format!("col-{i}")))
                            .label(header_for(c))
                            .checked(on)
                            .on_click(move |v: &bool, _w, app| {
                                let v = *v;
                                view.update(app, |t, c| {
                                    if let Some(slot) = t.visible.get_mut(i) {
                                        *slot = v;
                                    }
                                    c.notify();
                                });
                            }),
                    );
                }
                col
            })
    }
}

impl EventEmitter<PanelEvent> for ReportPanel {}
impl Focusable for ReportPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ReportPanel {
    fn panel_name(&self) -> &'static str {
        "Report"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Отчёт")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Report", &self.group)
    }
    fn on_added_to(&mut self, tab_panel: WeakEntity<TabPanel>, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab = Some(tab_panel);
    }
    fn toolbar_buttons(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let tab = self.tab.clone();
        let me = cx.entity().downgrade();
        Some(vec![Button::new("detach-report")
            .ghost()
            .label("⧉")
            .tooltip("В отдельное окно")
            .on_click(move |_, window, app| {
                if let (Some(tab), Some(me)) = (tab.as_ref().and_then(|t| t.upgrade()), me.upgrade()) {
                    let arc: Arc<dyn PanelView> = Arc::new(me);
                    tab.update(app, |tp, cx| tp.remove_panel(arc, window, cx));
                }
                let spec = DetachedSpec::new(group.clone(), "Report".to_string());
                crate::detached::spawn(app, &backend, &spec);
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            })])
    }
}

impl Render for ReportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll();
        if self.needs_query {
            self.requery(cx);
        }

        let border = rgb(hex(palette::LIFT_HOVER));

        // ── Фильтры ──
        let filters = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("Ядро:"))
            .child(self.core_combo(cx))
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("Монета:"))
            .child(div().w(px(90.0)).child(Input::new(&self.coin).small().cleanable(true)))
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("Сторона:"))
            .child(self.side_combo(cx))
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("С:"))
            .child(div().w(px(110.0)).child(Input::new(&self.from).small()))
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("По:"))
            .child(div().w(px(110.0)).child(Input::new(&self.to).small()))
            .child(self.columns_menu(cx));

        // ── Таблица ──
        let vis: Vec<usize> = (0..self.table.cols.len()).filter(|i| self.visible.get(*i).copied().unwrap_or(false)).collect();
        let table_el: AnyElement = if vis.is_empty() {
            div().p_3().text_color(rgb(hex(palette::TEXT_2))).child("Все колонки скрыты — включите в «Колонки».").into_any_element()
        } else {
            // Заголовки (кликабельные, стрелка у активной колонки).
            let mut header = h_flex().gap_0().items_center().py_1();
            for &i in &vis {
                let col = self.table.cols[i];
                let arrow = if self.sort_key == col {
                    if self.sort_desc { " ▼" } else { " ▲" }
                } else {
                    ""
                };
                let colname = col.to_string();
                header = header.child(
                    div()
                        .id(SharedString::from(format!("rh-{col}")))
                        .w(px(width_for(col)))
                        .flex_none()
                        .px_1()
                        .cursor_pointer()
                        .font_bold()
                        .text_xs()
                        .truncate()
                        .text_color(rgb(hex(palette::TEXT)))
                        .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
                        .child(format!("{}{arrow}", header_for(col)))
                        .on_click(cx.listener(move |t, _, _, cx| t.click_header(&colname, cx))),
                );
            }

            let rows_el: AnyElement = if self.table.rows.is_empty() {
                div().p_3().text_color(rgb(hex(palette::TEXT_2))).child("Нет отчётов под фильтр (или БД пуста).").into_any_element()
            } else {
                let mut col = v_flex().gap_0();
                for (ri, r) in self.table.rows.iter().enumerate() {
                    let mut row = h_flex().gap_0().items_center().py(px(1.0));
                    if ri % 2 == 1 {
                        row = row.bg(rgb(hex(palette::LIFT)));
                    }
                    for &i in &vis {
                        let cname = self.table.cols[i];
                        let val = r.get(i).unwrap_or(&Value::Null);
                        let (text, color) = cell(cname, val);
                        let c = color.unwrap_or(hex(palette::TEXT));
                        row = row.child(
                            div()
                                .w(px(width_for(cname)))
                                .flex_none()
                                .px_1()
                                .text_xs()
                                .truncate()
                                .text_color(rgb(c))
                                .child(text),
                        );
                    }
                    col = col.child(row);
                }
                div().id("rep-rows").flex_1().w_full().overflow_y_scroll().child(col).into_any_element()
            };

            // Горизонтальный скролл оборачивает заголовок + строки.
            div()
                .id("rep-hscroll")
                .flex_1()
                .w_full()
                .overflow_x_scroll()
                .child(v_flex().min_w(px(width_total(&vis))).h_full().child(header).child(div().w_full().h(px(1.0)).bg(border)).child(rows_el))
                .into_any_element()
        };

        // ── ИТОГО ──
        let (sum, count) = self.totals;
        let sum_col = if sum > 0.0 { palette::GREEN } else if sum < 0.0 { palette::RED } else { palette::TEXT_2 };
        let totals = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("Итого за период:"))
            .child(div().font_bold().text_color(rgb(hex(sum_col))).child(format!("{sum:+.6} BTC")))
            .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child(format!("ордеров: {count}")))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .text_xs()
                    .text_color(rgb(hex(palette::TEXT_2)))
                    .child(format!("показано (топ): {}", self.table.rows.len())),
            );

        v_flex()
            .id("report-panel")
            .size_full()
            .track_focus(&self.focus)
            .child(filters)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(table_el)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(totals)
    }
}

fn width_total(vis: &[usize]) -> f32 {
    vis.iter().map(|&i| width_for(db::DISPLAY_COLUMNS[i])).sum()
}

/// Текст + цвет ячейки по имени колонки и значению (порт `cell`).
fn cell(col: &str, v: &Value) -> (String, Option<u32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None),
        "isshort" => match as_i64(v) {
            Some(1) => ("Шорт".into(), Some(hex(palette::RED))),
            Some(0) => ("Лонг".into(), Some(hex(palette::GREEN))),
            _ => (String::new(), Some(hex(palette::TEXT_2))),
        },
        "emulator" => match as_i64(v) {
            Some(1) => ("эму".into(), Some(hex(palette::TEXT_2))),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(hex(palette::GREEN)),
                Some(x) if x < 0.0 => Some(hex(palette::RED)),
                _ => None,
            };
            (n.map(|x| format!("{x:+.6}")).unwrap_or_default(), color)
        }
        _ => (value_to_string(v), None),
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(r) => Some(*r),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => moon_core::util::fmt::compact(*r, 8),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => "<blob>".into(),
    }
}

/// Человекочитаемый заголовок колонки (порт `header_for`).
fn header_for(col: &str) -> &str {
    match col {
        "buydate" => "Открыт (UTC)",
        "closedate" => "Закрыт (UTC)",
        "sellsetdate" => "Sell set",
        "last_update_at" => "Обновлён",
        "core_name" => "Ядро",
        "db_id" => "ID",
        "taskid" => "TaskID",
        "exorderid" => "ExOrderID",
        "coin" => "Монета",
        "isshort" => "Сторона",
        "quantity" => "Кол-во",
        "boughtq" => "Куплено",
        "buyprice" => "Покупка",
        "sellprice" => "Продажа",
        "spentbtc" => "Влож.BTC",
        "gainedbtc" => "Получ.BTC",
        "profitbtc" => "Профит BTC",
        "lev" => "Плечо",
        "strategyid" => "Strat",
        "channelname" => "Канал",
        "signaltype" => "Сигнал",
        "fname" => "Файл",
        "basecurrency" => "BaseCur",
        "emulator" => "Эму",
        "status" => "Статус",
        "sellreason" => "Причина",
        "comment" => "Коммент",
        other => other,
    }
}

fn width_for(col: &str) -> f32 {
    match col {
        "buydate" | "closedate" => 120.0,
        "sellsetdate" | "last_update_at" => 116.0,
        "comment" => 280.0,
        "sellreason" => 170.0,
        "channelname" | "signaltype" | "fname" | "exorderid" => 110.0,
        "core_name" | "coin" => 88.0,
        "profitbtc" | "gainedbtc" | "spentbtc" => 96.0,
        "lev" | "isshort" | "emulator" => 52.0,
        _ => 82.0,
    }
}

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
