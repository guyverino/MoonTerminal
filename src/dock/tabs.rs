//! Нижний док с вкладками: полоска вкладок (Ордера/Активы/Лог/Отчёт) + контент
//! активной вкладки. Контент рисуется в общий `ui` через [`content_ui`] — та же
//! функция зовётся и при откреплении вкладки в отдельное окно (App рисует её в
//! свой EguiSurface), поэтому состояние одно и то же, без дубля (см.
//! docs/DOCK_TABS_PLAN.md).
//!
//! Открепление: вкладку можно ПОТЯНУТЬ (drag) — на отпускании она превращается в
//! окно (`TabsOutput.detach`). Пока вкладка откреплена, в доке на её месте —
//! плашка «открыто в окне» с кнопкой «вернуть» (`TabsOutput.repin`); закрытие окна
//! также возвращает вкладку (это решает App). Реально наполнены Orders/Log/Report;
//! Assets — заглушка до подключения данных.

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
    /// Порядок вкладок в полоске (слева направо). Индексы совпадают с [`idx`].
    pub const ALL: [DockTab; 4] = [
        DockTab::Orders,
        DockTab::Assets,
        DockTab::Log,
        DockTab::Report,
    ];

    /// Индекс вкладки (для массива `detached` в [`crate::dock::Dock`]).
    pub fn idx(self) -> usize {
        self as usize
    }

    /// Локализованный заголовок вкладки (и заголовок откреплённого окна).
    pub fn title(self) -> String {
        match self {
            DockTab::Orders => t!("dock.tab.orders").to_string(),
            DockTab::Assets => t!("dock.tab.assets").to_string(),
            DockTab::Log => t!("dock.tab.log").to_string(),
            DockTab::Report => t!("dock.tab.report").to_string(),
        }
    }
}

/// Запросы пользователя по итогам кадра дока — обрабатывает App (создаёт/закрывает
/// окна открепления, переключает флаги).
#[derive(Default)]
pub struct TabsOutput {
    /// Вкладку потянули — открепить в отдельное окно.
    pub detach: Option<DockTab>,
    /// Нажата «вернуть в док» на плашке откреплённой вкладки.
    pub repin: Option<DockTab>,
}

/// Нижний док с вкладками. `active` — текущая вкладка (мутируется кликом).
/// `report` — состояние вкладки «Отчёт». `orders` — открытые ордера группы.
/// `detached` — какие вкладки сейчас откреплены в окна (по [`DockTab::idx`]).
pub fn show(
    ctx: &egui::Context,
    active: &mut DockTab,
    report: &mut ReportView,
    orders: &[(String, OrderRow)],
    detached: &[bool; 4],
) -> TabsOutput {
    let mut out = TabsOutput::default();
    egui::TopBottomPanel::bottom("dock")
        .resizable(true)
        .default_height(190.0)
        .min_height(90.0)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            // Полоска вкладок. Клик — выбор; перетаскивание — открепить в окно.
            ui.horizontal(|ui| {
                for tab in DockTab::ALL {
                    let selected = *active == tab;
                    let is_det = detached[tab.idx()];
                    let label = if is_det {
                        format!("{} ↗", tab.title())
                    } else {
                        tab.title()
                    };
                    let resp = theme::seg_btn_sensed(
                        ui,
                        &label,
                        selected,
                        None,
                        false,
                        28.0,
                        egui::Sense::click_and_drag(),
                    );
                    if resp.clicked() {
                        *active = tab;
                    }
                    // Потянули вкладку → открепить (если ещё не откреплена).
                    if resp.drag_stopped() && !is_det {
                        out.detach = Some(tab);
                    }
                    resp.on_hover_text(if is_det {
                        t!("dock.tab.in_window")
                    } else {
                        t!("dock.tab.drag_hint")
                    });
                    ui.add_space(theme::BTN_GAP);
                }
            });
            ui.add_space(4.0);
            ui.separator();

            // Контент активной вкладки — или плашка, если она откреплена в окно.
            if detached[active.idx()] {
                detached_placeholder(ui, &mut out, *active);
            } else {
                content_ui(ui, *active, report, orders);
            }
        });
    out
}

/// Рендер контента конкретной вкладки в данный `ui`. Зовётся и из дока (inline), и
/// из App для откреплённого окна — единый источник, без дубля состояния.
pub fn content_ui(
    ui: &mut egui::Ui,
    tab: DockTab,
    report: &mut ReportView,
    orders: &[(String, OrderRow)],
) {
    match tab {
        DockTab::Orders => super::orders_panel::ui(ui, orders),
        DockTab::Assets => placeholder(ui, t!("dock.todo.assets").to_string()),
        DockTab::Log => super::log_panel::ui(ui),
        DockTab::Report => report.ui(ui),
    }
}

/// Плашка на месте откреплённой вкладки: «открыто в окне» + кнопка «вернуть».
fn detached_placeholder(ui: &mut egui::Ui, out: &mut TabsOutput, tab: DockTab) {
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(t!("dock.tab.in_window")).weak());
        ui.add_space(8.0);
        if theme::seg_btn(ui, &t!("dock.tab.repin"), false, None, false).clicked() {
            out.repin = Some(tab);
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
