//! Панель параметров выбранной секции (read-only). Имя слева, значение справа.
//! Неактивные поля (depends_on не выполнен) гасим тёмным; галка «только активные»
//! их скрывает. Bool/choice показываем как YES/NO. Длинный текст — в окошке по «…».

use std::collections::HashMap;

use super::{
    common_fields, kinds_differ, multi_rows, selected_row, selected_sections, selected_values,
    StrategiesState,
};
use crate::feed::{SchemaField, SchemaFieldUi, StrategyRow};
use crate::session::CoreStore;
use crate::shell::theme;

/// Длиннее этого (символов) — значение считаем «длинным»: обрезаем и даём «…».
const LONG_VALUE: usize = 28;

pub fn show(ui: &mut egui::Ui, st: &mut StrategiesState, store: &CoreStore) {
    let Some(row) = selected_row(st, store) else {
        ui.add_space(12.0);
        ui.label(egui::RichText::new(t!("strat.no_selection").to_string()).weak());
        return;
    };
    let Some(sections) = selected_sections(st, store) else {
        return;
    };
    let Some(sec) = sections.get(st.selected_section) else {
        return;
    };
    let values = selected_values(st, store);
    // Объединённый показ по всем выбранным (любых видов). `common` — поля, что есть
    // у ВСЕХ выбранных (иначе менять нельзя — скрываем). При разных видах прячем
    // ещё и сам тип (SignalType).
    let rows = multi_rows(st, store);
    let multi = rows.len() > 1;
    let common = common_fields(st, store);
    let differ = kinds_differ(st, store);

    ui.add_space(2.0);
    // Заголовок раздела (strong — как «Разделы») + счётчик (полей / выбрано) справа.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&sec.title).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let count = if multi {
                t!("strat.selected_count", n = rows.len()).to_string()
            } else {
                t!("strat.fields_count", n = sec.fields.len()).to_string()
            };
            ui.label(egui::RichText::new(count).color(theme::MUTED).small());
        });
    });
    ui.checkbox(&mut st.only_active_params, t!("strat.only_active").to_string());
    ui.separator();

    // Порядок ядра: сортируем поля секции по их позиции в сериализации стратегии
    // (row.fields). Поля, не сохранённые ядром, уходят в конец — в порядке схемы.
    let pos: HashMap<String, usize> = row
        .fields
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (n.to_lowercase(), i))
        .collect();
    let mut fields: Vec<&SchemaField> = sec.fields.iter().collect();
    fields.sort_by_key(|f| pos.get(&f.name.to_lowercase()).copied().unwrap_or(usize::MAX));

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in fields {
                let lname = f.name.to_lowercase();
                // Поля, которых нет у кого-то из выбранных, — менять нельзя, скрываем.
                if let Some(c) = &common {
                    if !c.contains(&lname) {
                        continue;
                    }
                }
                // Разные виды → тип (SignalType) менять нельзя — скрываем.
                if differ && lname == "signaltype" {
                    continue;
                }
                let active = st.rules.field_active(&f.name, &values);
                if st.only_active_params && !active {
                    continue;
                }
                // По выбранным: общее значение или None (различаются).
                let merged = merged_value(&rows, f);
                field_row(ui, &mut st.popup, f, merged, active);
            }
        });

    // Окошко просмотра длинного значения (read-only). Редактирование — позже.
    if let Some((name, val)) = st.popup.clone() {
        let mut open = true;
        egui::Window::new(name)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([460.0, 280.0])
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut text = val.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(10)
                            .interactive(false),
                    );
                });
            });
        if !open {
            st.popup = None;
        }
    }
}

/// Строка поля: имя слева, значение справа. `active=false` — приглушаем тёмным.
/// `merged=None` — значения у выбранных различаются (помечаем «≠», без значения).
fn field_row(
    ui: &mut egui::Ui,
    popup: &mut Option<(String, String)>,
    f: &SchemaField,
    merged: Option<String>,
    active: bool,
) {
    let name_col = if active { theme::TEXT_2 } else { theme::TEXT_3 };
    let val_col = if active { theme::TEXT } else { theme::TEXT_3 };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&f.name).color(name_col));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let Some(value) = merged else {
                // Различаются — пустое значение, помеченное «≠».
                ui.label(egui::RichText::new("≠").strong().color(theme::ACCENT))
                    .on_hover_text(t!("strat.differs").to_string());
                return;
            };
            match f.ui {
                SchemaFieldUi::Checkbox => {
                    let on = is_on(&value);
                    let col = if active && on { theme::GREEN } else { theme::TEXT_3 };
                    ui.label(
                        egui::RichText::new(if on { "YES" } else { "NO" })
                            .strong()
                            .color(col),
                    );
                }
                _ => {
                    let long = value.chars().count() > LONG_VALUE;
                    if long {
                        if ui.small_button("…").clicked() {
                            *popup = Some((f.name.clone(), value.clone()));
                        }
                        let short: String = value.chars().take(LONG_VALUE).collect();
                        ui.label(egui::RichText::new(format!("{short}…")).color(val_col))
                            .on_hover_text(&value);
                    } else {
                        ui.label(egui::RichText::new(&value).color(val_col));
                    }
                }
            }
        });
    });
}

/// Общее значение поля по всем строкам или None, если различаются.
fn merged_value(rows: &[&StrategyRow], f: &SchemaField) -> Option<String> {
    let mut it = rows.iter().map(|r| field_value(r, f));
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

/// Значение поля стратегии (по имени) или дефолт схемы.
fn field_value(row: &StrategyRow, f: &SchemaField) -> String {
    row.fields
        .iter()
        .find(|(n, _)| n == &f.name)
        .map(|(_, v)| v.clone())
        .or_else(|| f.default.clone())
        .unwrap_or_default()
}

fn is_on(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
}
