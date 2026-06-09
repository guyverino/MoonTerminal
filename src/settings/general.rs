//! Вкладка «Общие»: язык интерфейса. Выбор пишется в draft-конфиг и применяется
//! после «Сохранить» (тогда app пере-создаёт окна групп с новым языком).

use super::SettingsTab;
use crate::config::{AppConfig, Language};
use crate::icons::IconSet;

#[derive(Default)]
pub struct GeneralTab;

impl SettingsTab for GeneralTab {
    fn title(&self) -> String {
        t!("tab.general").to_string()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &mut AppConfig,
        _icons: &mut IconSet,
        _status: &super::CoreStatuses,
        _actions: &mut super::SettingsActions,
    ) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(t!("general.language")).strong());
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("lang_combo")
                .selected_text(cfg.language.label())
                .show_ui(ui, |ui| {
                    for lang in Language::ALL {
                        ui.selectable_value(&mut cfg.language, lang, lang.label());
                    }
                });
        });
        ui.add_space(4.0);
        ui.label(egui::RichText::new(t!("general.language_hint")).weak());
    }
}
