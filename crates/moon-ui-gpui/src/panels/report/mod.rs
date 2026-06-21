//! Панель «Отчёт» — порт egui `src/dock/report_view.rs`. Таблица закрытых сделок
//! (ордеров) из локальной SQLite. Фильтры (ядро/монета/сторона/даты) + выбор колонок
//! сверху, ИТОГО за период снизу, generic-таблица по всем колонкам БД с сортировкой
//! по клику на заголовок. Автообновление по счётчику-генерации writer'а (Backend.reports).
//!
//! По функционалу разнесено: состояние/запросы/жизненный цикл — здесь, поля-списки и
//! меню колонок — [`controls`], форматирование колонок/ячеек/заголовков — [`columns`].

mod columns;
mod controls;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonDataCell, MoonDataRow, MoonDataTable,
    MoonDataTableColumn, MoonDataTableState, MoonDropdown, MoonInput, MoonInputEvent,
    MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonText, MoonTone, Panel, PanelEvent,
    PanelState, StyledExt, h_flex, v_flex,
};
use rusqlite::Connection;
use rusqlite::types::Value;

use crate::{Backend, design};
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
    "sellreason",
    "comment",
];

pub struct ReportPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    pub(super) cores: Vec<(u64, String)>,
    pub(super) table: Rc<ReportTable>,
    totals: (f64, i64),

    sort_key: String,
    sort_desc: bool,

    pub(super) sel_core: usize,
    coin: Entity<MoonInputState>,
    from: Entity<MoonInputState>,
    to: Entity<MoonInputState>,
    pub(super) side: SideFilter,
    needs_query: bool,

    /// Видимость колонок (параллельно db::DISPLAY_COLUMNS).
    pub(super) visible: Vec<bool>,
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
        // Видимость колонок: восстанавливаем сохранённый набор (app_meta), иначе дефолт.
        let visible: Vec<bool> = conn
            .as_ref()
            .and_then(db::load_visible)
            .map(|saved| {
                db::DISPLAY_COLUMNS
                    .iter()
                    .map(|c| saved.iter().any(|s| s == c))
                    .collect()
            })
            .unwrap_or_else(|| {
                db::DISPLAY_COLUMNS
                    .iter()
                    .map(|c| DEFAULT_VISIBLE.contains(c))
                    .collect()
            });
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

    pub(super) fn set_core(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.sel_core != i {
            self.sel_core = i;
            self.needs_query = true;
            cx.notify();
        }
    }
    pub(super) fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {
        if self.side != s {
            self.side = s;
            self.needs_query = true;
            cx.notify();
        }
    }
    /// Переключить видимость колонки и СОХРАНИТЬ набор (app_meta) — переживает рестарт.
    pub(super) fn toggle_column(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(slot) = self.visible.get_mut(i) {
            *slot = !*slot;
        }
        if let Some(conn) = &self.conn {
            let cols: Vec<&str> = db::DISPLAY_COLUMNS
                .iter()
                .enumerate()
                .filter(|(j, _)| self.visible.get(*j).copied().unwrap_or(false))
                .map(|(_, c)| *c)
                .collect();
            db::save_visible(conn, &cols);
        }
        cx.notify();
    }
    fn set_report_sort(&mut self, col: &str, sort_desc: bool, cx: &mut Context<Self>) {
        if self.sort_key == col && self.sort_desc == sort_desc {
            return;
        }
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
}

impl EventEmitter<PanelEvent> for ReportPanel {}
impl Focusable for ReportPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ReportPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
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
        Some(vec![crate::panels::detach_button(
            "Report",
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
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
            let cols = columns::report_columns(&vis);
            div()
                .id("rep-table-host")
                .relative()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(
                    MoonDataTable::new("report-table", row_count, move |ri, _window, _app| {
                        columns::report_data_row(ri, &table, &visible, p)
                    })
                    .state(&table_state)
                    .columns(cols)
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
                            .h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
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
