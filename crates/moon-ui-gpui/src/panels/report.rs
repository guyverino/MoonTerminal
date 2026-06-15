//! Панель «Отчёт» — порт egui `src/dock/report_view.rs`. Таблица закрытых сделок
//! (ордеров) из локальной SQLite. Фильтры (ядро/монета/сторона/даты) + выбор колонок
//! сверху, ИТОГО за период снизу, generic-таблица по всем колонкам БД с сортировкой
//! по клику на заголовок. Автообновление по счётчику-генерации writer'а (Backend.reports).

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_palette::{
    DockArea, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDataCell, MoonDataRow,
    MoonDataTable, MoonDataTableColumn, MoonDataTableState, MoonDropdown, MoonInput,
    MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, Panel, PanelEvent,
    PanelState, StyledExt, h_flex, v_flex,
};
use rusqlite::Connection;
use rusqlite::types::Value;

use crate::Backend;
use crate::detached::DetachedSpec;
use moon_core::db::{self, ReportFilter, ReportTable, SideFilter};

/// Data cap для отчёта. UI ниже виртуализирован, так что 100k строк не превращаются
/// в 100k GPUI-элементов; отдельная серверная пагинация здесь пока не нужна.
const MAX_REPORT_ROWS: usize = 100_000;

/// Колонки, видимые по умолчанию (имена = колонки БД).
const DEFAULT_VISIBLE: &[&str] = &[
    "buydate",
    "closedate",
    "core_name",
    "coin",
    "isshort",
    "quantity",
    "buyprice",
    "sellprice",
    "profitbtc",
    "lev",
    "strategyid",
    "sellreason",
    "comment",
];

pub struct ReportPanel {
    backend: Entity<Backend>,
    group: String,
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    cores: Vec<(u64, String)>,
    table: Rc<ReportTable>,
    totals: (f64, i64),

    sort_key: String,
    sort_desc: bool,

    sel_core: usize,
    coin: Entity<MoonInputState>,
    from: Entity<MoonInputState>,
    to: Entity<MoonInputState>,
    side: SideFilter,
    needs_query: bool,

    /// Видимость колонок (параллельно db::DISPLAY_COLUMNS).
    visible: Vec<bool>,
    table_state: Entity<MoonDataTableState>,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl ReportPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|h| h.generation.clone());
        let conn = db::open_reader();
        let cores = conn.as_ref().map(db::distinct_cores).unwrap_or_default();
        let last_gen = generation
            .as_ref()
            .map(|g| g.load(Ordering::Relaxed))
            .unwrap_or(0);
        let visible = db::DISPLAY_COLUMNS
            .iter()
            .map(|c| DEFAULT_VISIBLE.contains(c))
            .collect();
        let (sort_key, sort_desc) = conn
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));
        let table_state = cx.new(|_| MoonDataTableState::new());
        table_state.update(cx, |state, _| {
            state.set_sort(sort_key.clone(), !sort_desc);
        });

        let coin = cx.new(|cx| MoonInputState::new(window, cx).placeholder("все"));
        let from = cx.new(|cx| MoonInputState::new(window, cx).placeholder("ГГГГ-ММ-ДД"));
        let to = cx.new(|cx| MoonInputState::new(window, cx).placeholder("ГГГГ-ММ-ДД"));
        for st in [&coin, &from, &to] {
            cx.subscribe(st, |t, _e, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
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
            table: Rc::new(ReportTable {
                cols: db::DISPLAY_COLUMNS,
                rows: Vec::new(),
            }),
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
            table_state,
            dock: None,
            focus: cx.focus_handle(),
        }
    }

    fn filter(&self, cx: &App) -> ReportFilter {
        ReportFilter {
            core_uid: if self.sel_core == 0 {
                None
            } else {
                self.cores.get(self.sel_core - 1).map(|(uid, _)| *uid)
            },
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
            self.table = Rc::new(db::query_reports(
                conn,
                &f,
                &self.sort_key,
                self.sort_desc,
                MAX_REPORT_ROWS,
            ));
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
    fn set_report_sort(&mut self, col: &str, sort_desc: bool, cx: &mut Context<Self>) {
        self.sort_key = col.to_string();
        self.sort_desc = sort_desc;
        self.table_state.update(cx, |state, _| {
            state.set_sort(col.to_string(), !sort_desc);
        });
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
            self.cores
                .get(self.sel_core - 1)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "Все".into())
        };
        let view = cx.entity();
        let cores = self.cores.clone();
        let mut items = vec![
            MoonMenuItem::with_key("rc-all", "Все")
                .selected(self.sel_core == 0)
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |t, c| t.set_core(0, c));
                    }
                }),
        ];
        for (i, (_u, name)) in cores.into_iter().enumerate() {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("rc-{i}"), name)
                    .selected(self.sel_core == i + 1)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_core(i + 1, c));
                    }),
            );
        }
        MoonDropdown::new("rep-core")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(130.0)
            .menu_width(180.0)
            .menu_max_height(360.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Комбобокс стороны (Все/Лонг/Шорт).
    fn side_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.side {
            SideFilter::All => "Все",
            SideFilter::Long => "Лонг",
            SideFilter::Short => "Шорт",
        };
        let view = cx.entity();
        let opts = [
            (SideFilter::All, "Все"),
            (SideFilter::Long, "Лонг"),
            (SideFilter::Short, "Шорт"),
        ];
        MoonDropdown::new("rep-side")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(86.0)
            .menu_width(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(opts.into_iter().map(move |(side, label)| {
                let view = view.clone();
                MoonMenuItem::with_key(format!("rs-{label}"), label)
                    .selected(side == self.side)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_side(side, c));
                    })
            }))
    }

    /// Попап выбора видимых колонок (чекбоксы).
    fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let visible = self.visible.clone();
        let items = db::DISPLAY_COLUMNS.iter().enumerate().map(move |(i, c)| {
            let on = visible.get(i).copied().unwrap_or(false);
            let view = view.clone();
            MoonMenuItem::with_key(format!("col-{i}"), header_for(c))
                .checked(on)
                .selected(on)
                .on_click(move |_, _, app| {
                    view.update(app, |t, c| {
                        if let Some(slot) = t.visible.get_mut(i) {
                            *slot = !*slot;
                        }
                        c.notify();
                    });
                })
        });
        MoonDropdown::new("rep-cols")
            .label("Колонки ▾")
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(110.0)
            .menu_width(230.0)
            .menu_max_height(420.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .items(items)
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
            MoonButton::new("detach-report")
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .on_click(move |_, window, app| {
                    if let Some(dock) = dock.as_ref().and_then(|d| d.upgrade()) {
                        dock.update(app, |area, cx| {
                            area.remove_panel_by_name("Report", window, cx);
                        });
                    }
                    let spec = DetachedSpec::new(group.clone(), "Report".to_string());
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

impl Render for ReportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll();
        if self.needs_query {
            self.requery(cx);
        }

        let p = MoonPalette::active(cx);
        let border = rgb(p.border);
        self.table_state.update(cx, |state, _| {
            state.set_sort(self.sort_key.clone(), !self.sort_desc);
        });

        // ── Фильтры ──
        let filters = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().text_xs().text_color(rgb(p.text_soft)).child("Ядро:"))
            .child(self.core_combo(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_soft))
                    .child("Монета:"),
            )
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("rep-coin")
                        .state(&self.coin)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_soft))
                    .child("Сторона:"),
            )
            .child(self.side_combo(cx))
            .child(div().text_xs().text_color(rgb(p.text_soft)).child("С:"))
            .child(
                div()
                    .w(px(110.0))
                    .child(MoonInput::new("rep-from").state(&self.from).small()),
            )
            .child(div().text_xs().text_color(rgb(p.text_soft)).child("По:"))
            .child(
                div()
                    .w(px(110.0))
                    .child(MoonInput::new("rep-to").state(&self.to).small()),
            )
            .child(self.columns_menu(cx));

        // ── Таблица ──
        let vis: Vec<usize> = (0..self.table.cols.len())
            .filter(|i| self.visible.get(*i).copied().unwrap_or(false))
            .collect();
        let table_el: AnyElement = if vis.is_empty() {
            div()
                .p_3()
                .text_color(rgb(p.text_soft))
                .child("Все колонки скрыты — включите в «Колонки».")
                .into_any_element()
        } else {
            let table = self.table.clone();
            let visible = Rc::new(vis.clone());
            let row_count = table.rows.len();
            let view = cx.entity();
            let table_state = self.table_state.clone();
            let columns = report_columns(&vis);
            div()
                .id("rep-table-host")
                .relative()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(
                    MoonDataTable::new("report-table", row_count, move |ri, _window, _app| {
                        report_data_row(ri, &table, &visible, p)
                    })
                    .state(&table_state)
                    .columns(columns)
                    .header_height(24.0)
                    .row_height(24.0)
                    .on_sort(move |key, ascending, _window, app| {
                        let key = key.to_string();
                        view.update(app, |t, cx| t.set_report_sort(&key, !ascending, cx));
                    }),
                )
                .when(row_count == 0, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left(px(10.0))
                            .top(px(25.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(rgb(p.text_soft))
                            .child("Нет отчётов под фильтр (или БД пуста)."),
                    )
                })
                .into_any_element()
        };

        // ── ИТОГО ──
        let (sum, count) = self.totals;
        let sum_col = if sum > 0.0 {
            p.green
        } else if sum < 0.0 {
            p.red
        } else {
            p.text_soft
        };
        let totals = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_soft))
                    .child("Итого за период:"),
            )
            .child(
                div()
                    .font_bold()
                    .text_color(rgb(sum_col))
                    .child(format!("{sum:+.6} BTC")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_soft))
                    .child(format!("ордеров: {count}")),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .text_xs()
                    .text_color(rgb(p.text_soft))
                    .child(format!("показано (топ): {}", self.table.rows.len())),
            );

        v_flex()
            .id("report-panel")
            .size_full()
            .track_focus(&self.focus)
            .bg(rgb(p.table_body))
            .child(filters)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(table_el)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(totals)
    }
}

fn report_columns(vis: &[usize]) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = db::DISPLAY_COLUMNS[i];
            let column =
                MoonDataTableColumn::new(col, header_for(col), width_for(col)).sortable(true);
            if is_numeric_report_column(col) {
                column.right()
            } else {
                column
            }
        })
        .collect()
}

fn report_data_row(ri: usize, table: &ReportTable, vis: &[usize], p: MoonPalette) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = table.rows.get(ri) {
        for &i in vis {
            let cname = table.cols[i];
            let val = r.get(i).unwrap_or(&Value::Null);
            cells.push(report_data_cell(cname, val, p));
        }
    }
    MoonDataRow::new(cells)
}

fn report_data_cell(col: &str, val: &Value, p: MoonPalette) -> MoonDataCell {
    let (text, color) = cell(col, val, p);
    let cell = MoonDataCell::text(text).font_size(10.0).line_height(13.0);
    if let Some(color) = color {
        cell.text_color(color)
    } else {
        cell
    }
}

fn is_numeric_report_column(col: &str) -> bool {
    matches!(
        col,
        "quantity"
            | "boughtq"
            | "buyprice"
            | "sellprice"
            | "spentbtc"
            | "gainedbtc"
            | "profitbtc"
            | "lev"
            | "db_id"
            | "taskid"
    )
}

/// Текст + цвет ячейки по имени колонки и значению (порт `cell`).
fn cell(col: &str, v: &Value, p: MoonPalette) -> (String, Option<u32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => {
            (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None)
        }
        "isshort" => match as_i64(v) {
            Some(1) => ("Шорт".into(), Some(p.red)),
            Some(0) => ("Лонг".into(), Some(p.green)),
            _ => (String::new(), Some(p.text_soft)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => ("эму".into(), Some(p.text_soft)),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(p.green),
                Some(x) if x < 0.0 => Some(p.red),
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
