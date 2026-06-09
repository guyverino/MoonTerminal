//! `ReportView` — состояние + рендер таблицы отчётов (закрытые ордера из локальной
//! SQLite). Рисуется в любой `ui` через `show_inside`-панели, поэтому одинаково
//! работает и во вкладке «Отчёт» нижнего дока, и в откреплённом окне (App рисует
//! тот же `ReportView` в свой EguiSurface — состояние одно, без дубля).
//!
//! Фильтры (ядро/монета/сторона/даты) + выбор колонок сверху, ИТОГО за период
//! снизу, generic-таблица по всем колонкам БД с сортировкой по клику посередине.
//! Автообновление по счётчику-генерации writer'а (см. [`poll`](ReportView::poll)).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusqlite::types::Value;
use rusqlite::Connection;

use crate::db::{self, ReportFilter, ReportTable, SideFilter};
use crate::shell::theme;

/// Грузим только топ-N по сортировке (строк в БД может быть очень много).
const ROW_LIMIT: usize = 100;

/// Колонки, видимые по умолчанию (имена = колонки БД).
const DEFAULT_VISIBLE: &[&str] = &[
    "buydate", "closedate", "core_name", "coin", "isshort", "quantity", "buyprice",
    "sellprice", "profitbtc", "lev", "strategyid", "sellreason", "comment",
];

pub struct ReportView {
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    cores: Vec<(u64, String)>,
    table: ReportTable,
    totals: (f64, i64),

    sort_key: String,
    sort_desc: bool,

    sel_core: usize,
    coin_buf: String,
    from_buf: String,
    to_buf: String,
    side: SideFilter,
    needs_query: bool,

    /// Видимость колонок (параллельно db::DISPLAY_COLUMNS).
    visible: Vec<bool>,
}

impl ReportView {
    /// `generation` — счётчик writer'а (None = БД недоступна / нет writer'а).
    pub fn new(generation: Option<Arc<AtomicU64>>) -> Self {
        let conn = db::open_reader();
        let cores = conn.as_ref().map(db::distinct_cores).unwrap_or_default();
        let last_gen = generation.as_ref().map(|g| g.load(Ordering::Relaxed)).unwrap_or(0);
        let visible = db::DISPLAY_COLUMNS
            .iter()
            .map(|c| DEFAULT_VISIBLE.contains(c))
            .collect();
        let (sort_key, sort_desc) = conn
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));

        Self {
            generation,
            last_gen,
            conn,
            cores,
            table: ReportTable { cols: db::DISPLAY_COLUMNS, rows: Vec::new() },
            totals: (0.0, 0),
            sort_key,
            sort_desc,
            sel_core: 0,
            coin_buf: String::new(),
            from_buf: String::new(),
            to_buf: String::new(),
            side: SideFilter::All,
            needs_query: true,
            visible,
        }
    }

    /// Текущее значение счётчика-генерации (для дёшевого детекта новых отчётов
    /// host'ом — форсит кадр, когда активна вкладка «Отчёт»).
    pub fn generation(&self) -> u64 {
        self.generation.as_ref().map(|g| g.load(Ordering::Relaxed)).unwrap_or(0)
    }

    /// Сверяет счётчик writer'а: новые/изменённые записи → перезапрос. Возвращает
    /// `true`, если что-то изменилось (вызывающий может пометить кадр грязным).
    pub fn poll(&mut self) -> bool {
        if let Some(g) = &self.generation {
            let v = g.load(Ordering::Relaxed);
            if v != self.last_gen {
                self.last_gen = v;
                self.needs_query = true;
                return true;
            }
        }
        false
    }

    fn filter(&self) -> ReportFilter {
        ReportFilter {
            core_uid: if self.sel_core == 0 {
                None
            } else {
                self.cores.get(self.sel_core - 1).map(|(uid, _)| *uid)
            },
            date_from: db::parse_ymd(&self.from_buf),
            date_to: db::parse_ymd(&self.to_buf).map(|d| d + 86_399),
            coin: self.coin_buf.clone(),
            side: self.side,
        }
    }

    fn requery(&mut self) {
        if self.conn.is_none() {
            self.conn = db::open_reader();
        }
        let f = self.filter();
        if let Some(conn) = &self.conn {
            self.cores = db::distinct_cores(conn);
            self.table = db::query_reports(conn, &f, &self.sort_key, self.sort_desc, ROW_LIMIT);
            self.totals = db::query_totals(conn, &f);
        }
        self.needs_query = false;
    }

    /// Нарисовать отчёт в данный `ui` (вкладка дока или CentralPanel окна).
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll();
        if self.needs_query {
            self.requery();
        }

        let mut changed = false;
        let mut sort_changed = false;

        egui::TopBottomPanel::top("report-filters")
            .exact_height(44.0)
            .show_inside(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Ядро:").weak());
                    let sel_text = if self.sel_core == 0 {
                        "Все".to_string()
                    } else {
                        self.cores.get(self.sel_core - 1).map(|(_, n)| n.clone()).unwrap_or_else(|| "Все".into())
                    };
                    egui::ComboBox::from_id_salt("core_cb").selected_text(sel_text).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut self.sel_core, 0, "Все").changed();
                        for (i, (_u, name)) in self.cores.iter().enumerate() {
                            changed |= ui.selectable_value(&mut self.sel_core, i + 1, name).changed();
                        }
                    });

                    ui.separator();
                    ui.label(egui::RichText::new("Монета:").weak());
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.coin_buf).desired_width(80.0).hint_text("все")).changed();

                    ui.separator();
                    ui.label(egui::RichText::new("Сторона:").weak());
                    let side_text = match self.side {
                        SideFilter::All => "Все",
                        SideFilter::Long => "Лонг",
                        SideFilter::Short => "Шорт",
                    };
                    egui::ComboBox::from_id_salt("side_cb").selected_text(side_text).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut self.side, SideFilter::All, "Все").changed();
                        changed |= ui.selectable_value(&mut self.side, SideFilter::Long, "Лонг").changed();
                        changed |= ui.selectable_value(&mut self.side, SideFilter::Short, "Шорт").changed();
                    });

                    ui.separator();
                    ui.label(egui::RichText::new("С:").weak());
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.from_buf).desired_width(92.0).hint_text("ГГГГ-ММ-ДД")).changed();
                    ui.label(egui::RichText::new("По:").weak());
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.to_buf).desired_width(92.0).hint_text("ГГГГ-ММ-ДД")).changed();

                    ui.separator();
                    ui.menu_button("Колонки ▾", |ui| {
                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                            for (i, col) in db::DISPLAY_COLUMNS.iter().enumerate() {
                                ui.checkbox(&mut self.visible[i], header_for(col));
                            }
                        });
                    });
                });
            });

        egui::TopBottomPanel::bottom("report-totals")
            .exact_height(30.0)
            .show_inside(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (sum, count) = self.totals;
                    ui.label(egui::RichText::new("Итого за период:").weak());
                    let col = if sum > 0.0 { theme::GREEN } else if sum < 0.0 { theme::RED } else { theme::MUTED };
                    ui.label(egui::RichText::new(format!("{sum:+.6} BTC")).color(col).strong());
                    ui.separator();
                    ui.label(egui::RichText::new(format!("ордеров: {count}")).weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(format!("показано (топ): {}", self.table.rows.len())).weak());
                    });
                });
            });

        // Таблица — в оставшейся (центральной) области ui после top/bottom-панелей.
        // Без CentralPanel: он бы конфликтовал по id с центральной панелью чарта.
        reports_table(
            ui,
            &self.table,
            &self.visible,
            &mut self.sort_key,
            &mut self.sort_desc,
            &mut sort_changed,
        );

        if changed || sort_changed {
            self.needs_query = true;
        }
        if sort_changed {
            if let Some(conn) = &self.conn {
                db::save_sort(conn, &self.sort_key, self.sort_desc);
            }
        }
    }
}

/// Generic-таблица по всем DISPLAY_COLUMNS. Клик по заголовку — сортировка.
fn reports_table(
    ui: &mut egui::Ui,
    table: &ReportTable,
    visible: &[bool],
    sort_key: &mut String,
    sort_desc: &mut bool,
    sort_changed: &mut bool,
) {
    let vis: Vec<usize> = (0..table.cols.len()).filter(|i| visible.get(*i).copied().unwrap_or(false)).collect();
    if vis.is_empty() {
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Все колонки скрыты — включите в «Колонки».").weak());
        return;
    }
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;

    egui::ScrollArea::horizontal().show(ui, |ui| {
        // Заголовки — кликабельные, со стрелкой направления у активной колонки.
        ui.horizontal(|ui| {
            for &i in &vis {
                let col = table.cols[i];
                let arrow = if *sort_key == col {
                    if *sort_desc { " ▼" } else { " ▲" }
                } else {
                    ""
                };
                let w = width_for(col);
                let btn = egui::Button::new(egui::RichText::new(format!("{}{arrow}", header_for(col))).strong()).frame(false);
                if ui.add_sized([w, row_h], btn).clicked() {
                    if *sort_key == col {
                        *sort_desc = !*sort_desc;
                    } else {
                        *sort_key = col.to_string();
                        *sort_desc = true;
                    }
                    *sort_changed = true;
                }
            }
        });
        ui.separator();

        if table.rows.is_empty() {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Нет отчётов под фильтр (или БД пуста).").weak());
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, table.rows.len(), |ui, range| {
            for r in &table.rows[range] {
                ui.horizontal(|ui| {
                    for &i in &vis {
                        let col = table.cols[i];
                        let val = r.get(i).unwrap_or(&Value::Null);
                        let (text, color) = cell(col, val);
                        let mut rich = egui::RichText::new(text.clone());
                        if let Some(c) = color {
                            rich = rich.color(c);
                        }
                        let resp = ui.add_sized([width_for(col), row_h], egui::Label::new(rich).truncate());
                        if matches!(col, "comment" | "sellreason" | "channelname" | "signaltype" | "fname")
                            && !text.is_empty()
                        {
                            resp.on_hover_text(text);
                        }
                    }
                });
            }
        });
    });
}

/// Текст + цвет ячейки по имени колонки и значению.
fn cell(col: &str, v: &Value) -> (String, Option<egui::Color32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => {
            (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None)
        }
        "isshort" => match as_i64(v) {
            Some(1) => ("Шорт".into(), Some(theme::RED)),
            Some(0) => ("Лонг".into(), Some(theme::GREEN)),
            _ => (String::new(), Some(theme::MUTED)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => ("эму".into(), Some(theme::MUTED)),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(theme::GREEN),
                Some(x) if x < 0.0 => Some(theme::RED),
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
        Value::Real(r) => {
            let s = format!("{r:.8}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            if s.is_empty() { "0".into() } else { s.to_string() }
        }
        Value::Text(t) => t.clone(),
        Value::Blob(_) => "<blob>".into(),
    }
}

/// Человекочитаемый заголовок колонки (для дельт — имя как есть).
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
