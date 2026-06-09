//! Нижний док с вкладками: полоска вкладок (Ордера/Активы/Лог/Отчёт) + контент
//! активной вкладки. Контент каждой вкладки рисуется в общий `ui` контент-области
//! — это та же сигнатура, по которой панель позже сможет «открепиться» в отдельное
//! окно (см. docs/DOCK_TABS_PLAN.md). Пока реальна только вкладка «Ордера»;
//! остальные — заглушки до подключения данных (Активы) / своих модулей (Лог/Отчёт).

use super::ReportView;
use crate::feed::OrderRow;
use crate::shell::theme;

/// Активная вкладка дока. Хранится в [`crate::dock::Dock`].
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum DockTab {
    #[default]
    Orders,
    Assets,
    Log,
    Report,
}

impl DockTab {
    /// Порядок вкладок в полоске (слева направо).
    const ALL: [DockTab; 4] = [
        DockTab::Orders,
        DockTab::Assets,
        DockTab::Log,
        DockTab::Report,
    ];

    /// Локализованный заголовок вкладки.
    fn title(self) -> String {
        match self {
            DockTab::Orders => t!("dock.tab.orders").to_string(),
            DockTab::Assets => t!("dock.tab.assets").to_string(),
            DockTab::Log => t!("dock.tab.log").to_string(),
            DockTab::Report => t!("dock.tab.report").to_string(),
        }
    }
}

/// Нижний док с вкладками. `active` — текущая вкладка (мутируется кликом по
/// полоске). `report` — состояние вкладки «Отчёт». `orders` — открытые ордера
/// группы для вкладки «Ордера».
pub fn show(
    ctx: &egui::Context,
    active: &mut DockTab,
    report: &mut ReportView,
    orders: &[(String, OrderRow)],
) {
    egui::TopBottomPanel::bottom("dock")
        .resizable(true)
        .default_height(190.0)
        .min_height(90.0)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            // Полоска вкладок — единый стиль seg_btn (как кнопки шапки/тулбара).
            ui.horizontal(|ui| {
                for tab in DockTab::ALL {
                    let selected = *active == tab;
                    if theme::seg_btn(ui, &tab.title(), selected, None, false).clicked() {
                        *active = tab;
                    }
                    ui.add_space(theme::BTN_GAP);
                }
            });
            ui.add_space(4.0);
            ui.separator();

            // Контент активной вкладки.
            match *active {
                DockTab::Orders => super::orders_panel::ui(ui, orders),
                DockTab::Assets => placeholder(ui, t!("dock.todo.assets").to_string()),
                DockTab::Log => super::log_panel::ui(ui),
                DockTab::Report => report.ui(ui),
            }
        });
}

/// Заглушка контента вкладки (пока нет данных/модуля).
fn placeholder(ui: &mut egui::Ui, text: String) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(text).weak());
    });
}
