//! Состояние настроек: вкладки + редактируемая копия AppConfig (draft).
//! Рисуется в отдельном нативном окне (см. window/settings_window.rs).

pub mod connections;
pub mod general;
pub mod interface;

use std::collections::HashMap;

use crate::config::AppConfig;
use crate::feed::ConnStatus;
use crate::icons::IconSet;
use crate::session::CoreId;

/// Снимок статусов подключения ядер (CoreId → статус) для бейджей вкладки
/// «Подключения». Заполняется из живой сессии перед каждым кадром.
pub type CoreStatuses = HashMap<CoreId, ConnStatus>;

/// Действия, запрошенные из вкладок в этом кадре и применяемые приложением
/// (вне save). Сейчас — ручной реконнект ядер по кнопке у кружка статуса.
#[derive(Default)]
pub struct SettingsActions {
    /// Ядра (CoreId), для которых нажата кнопка «переподключить».
    pub reconnect: Vec<CoreId>,
    /// Группы, окно которых попросили показать (кнопка «глаз»): если окно закрыто —
    /// создать (с сохранённой раскладкой), иначе сфокусировать.
    pub show_group: Vec<String>,
}

pub trait SettingsTab {
    /// Заголовок вкладки (локализованный) — String, т.к. t! отдаёт владеющую строку.
    fn title(&self) -> String;
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &mut AppConfig,
        icons: &mut IconSet,
        status: &CoreStatuses,
        actions: &mut SettingsActions,
    );
}

pub struct SettingsState {
    active: usize,
    draft: AppConfig,
    tabs: Vec<Box<dyn SettingsTab>>,
    status: Option<(String, egui::Color32)>,
    /// Действия текущего кадра (реконнект и пр.) — забираются приложением.
    actions: SettingsActions,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            active: 0,
            draft: AppConfig::default(),
            tabs: vec![
                Box::new(connections::ConnectionsTab::default()),
                Box::new(general::GeneralTab::default()),
                Box::new(interface::InterfaceTab),
            ],
            status: None,
            actions: SettingsActions::default(),
        }
    }

    /// Забрать накопленные за кадр действия (реконнект и пр.) для применения.
    pub fn take_actions(&mut self) -> SettingsActions {
        std::mem::take(&mut self.actions)
    }

    /// Начать редактирование копии текущего конфига.
    pub fn begin(&mut self, current: &AppConfig) {
        self.draft = current.clone();
        self.status = None;
    }

    /// Тема из редактируемой копии — для живого превью чарта, пока окно открыто.
    pub fn draft_theme(&self) -> &crate::config::ChartTheme {
        &self.draft.theme
    }

    /// Тело окна настроек: таб-бар + активная вкладка (в ScrollArea).
    pub fn body(&mut self, ui: &mut egui::Ui, icons: &mut IconSet, status: &CoreStatuses) {
        ui.horizontal(|ui| {
            for i in 0..self.tabs.len() {
                let title = self.tabs[i].title();
                if ui.selectable_label(self.active == i, title).clicked() {
                    self.active = i;
                }
            }
        });
        ui.separator();

        let active = self.active.min(self.tabs.len().saturating_sub(1));
        // Вкладка сама управляет своим скроллом/раскладкой.
        self.tabs[active].ui(ui, &mut self.draft, icons, status, &mut self.actions);
    }

    /// Кнопка сохранения. Валидирует (уникальность имени/host:port) и пишет в файлы.
    /// Возвращает Some(config) только при успешном сохранении.
    pub fn footer(&mut self, ui: &mut egui::Ui) -> Option<AppConfig> {
        let mut saved = None;
        ui.horizontal(|ui| {
            if ui.button(t!("settings.save").to_string()).clicked() {
                match self.draft.save() {
                    Ok(()) => {
                        self.status =
                            Some((t!("settings.saved").to_string(), crate::shell::theme::GREEN));
                        saved = Some(self.draft.clone());
                    }
                    Err(e) => {
                        self.status = Some((e.to_string(), crate::shell::theme::RED));
                    }
                }
            }
            if let Some((s, c)) = &self.status {
                ui.label(egui::RichText::new(s).color(*c));
            }
        });
        saved
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}
