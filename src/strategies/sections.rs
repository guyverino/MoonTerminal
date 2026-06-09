//! Панель 2: разделы (секции) схемы выбранной стратегии. Неактивные (выключенные
//! через Ignore*-поля) гасим тёмным шрифтом; галка «только активные» их скрывает.

use super::{section_active, selected_sections, selected_values, StrategiesState};
use crate::session::CoreStore;
use crate::shell::theme;

pub fn show(ui: &mut egui::Ui, st: &mut StrategiesState, store: &CoreStore) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(t!("strat.sections").to_string()).strong());
    ui.checkbox(&mut st.only_active_sections, t!("strat.only_active").to_string());
    ui.separator();

    let Some(sections) = selected_sections(st, store) else {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(t!("strat.no_selection").to_string()).weak());
        return;
    };
    if sections.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(t!("strat.no_schema").to_string()).weak());
        return;
    }
    if st.selected_section >= sections.len() {
        st.selected_section = 0;
    }
    let values = selected_values(st, store);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, sec) in sections.iter().enumerate() {
                let active = section_active(&st.rules, &values, sec);
                if st.only_active_sections && !active {
                    continue;
                }
                // Неактивный раздел — приглушённым «тёмным» шрифтом.
                let mut text = egui::RichText::new(&sec.title);
                if !active {
                    text = text.color(theme::TEXT_3);
                }
                if ui.selectable_label(st.selected_section == i, text).clicked() {
                    st.selected_section = i;
                }
            }
        });
}
