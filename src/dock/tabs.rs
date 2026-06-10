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

    /// Вкладка по индексу (из сохранённой раскладки); вне диапазона → Orders.
    pub fn from_idx(i: usize) -> DockTab {
        DockTab::ALL.get(i).copied().unwrap_or(DockTab::Orders)
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

/// Высота свёрнутого дока — только полоска вкладок (без контента).
const STRIP_H: f32 = 46.0;

/// Нижний док с вкладками. `active` — текущая вкладка (мутируется кликом).
/// `report` — ОБЩИЙ `ReportView`. `orders` — открытые ордера группы. `detached` —
/// какие вкладки откреплены в окна (по [`DockTab::idx`]). `collapsed` — свёрнут ли
/// док (видна только полоска вкладок), переключается кнопкой справа в полоске.
pub fn show(
    ctx: &egui::Context,
    active: &mut DockTab,
    report: &mut ReportView,
    orders: &[(String, OrderRow)],
    detached: &[bool; 4],
    collapsed: &mut bool,
) -> TabsOutput {
    let mut out = TabsOutput::default();
    let is_collapsed = *collapsed;
    // Свёрнутый и развёрнутый — РАЗНЫЕ id панели: так egui хранит высоту ресайза
    // развёрнутого дока отдельно и восстанавливает её после разворота (свёрнутая
    // полоса не затирает запомненную высоту). Дефолт развёрнутого +50% (≈285).
    let id = if is_collapsed { "dock_collapsed" } else { "dock" };
    let mut panel = egui::TopBottomPanel::bottom(id);
    panel = if is_collapsed {
        // Свёрнут: фиксированная высота полоски, без ресайза — «уезжает» вниз.
        panel.resizable(false).exact_height(STRIP_H)
    } else {
        panel.resizable(true).default_height(285.0).min_height(90.0)
    };
    panel.show(ctx, |ui| {
        ui.add_space(4.0);
        // Полоска вкладок (underline-стиль). Клик — выбор; двойной клик —
        // открепить/вернуть; перетаскивание — открепить. Активную подчёркиваем
        // акцентом; общий hairline-бейзлайн отделяет полоску от контента.
        let mut active_x: Option<(f32, f32)> = None;
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for tab in DockTab::ALL {
                let selected = *active == tab;
                let is_det = detached[tab.idx()];
                let label = if is_det {
                    format!("{} ↗", tab.title())
                } else {
                    tab.title()
                };
                let resp = tab_button(ui, &label, selected, is_det);
                if selected {
                    active_x = Some((resp.rect.left(), resp.rect.right()));
                }
                // Двойной клик — открепить/вернуть; одиночный — выбрать.
                if resp.double_clicked() {
                    if is_det {
                        out.repin = Some(tab);
                    } else {
                        out.detach = Some(tab);
                    }
                } else if resp.clicked() {
                    *active = tab;
                }
                // Потянули вкладку → открепить (если ещё не откреплена).
                if resp.drag_stopped() && !is_det {
                    out.detach = Some(tab);
                }
                resp.on_hover_text(if is_det {
                    t!("dock.tab.hint_detached")
                } else {
                    t!("dock.tab.hint_docked")
                });
            }
            // Справа: кнопка свернуть/развернуть (самая правая), левее — подсказка.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let hint = if is_collapsed {
                    t!("dock.expand")
                } else {
                    t!("dock.collapse")
                };
                if collapse_button(ui, is_collapsed).on_hover_text(hint).clicked() {
                    *collapsed = !is_collapsed;
                }
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(t!("dock.tab.dblclick_hint"))
                        .size(theme::LABEL_SIZE)
                        .color(theme::TEXT_3),
                );
            });
        });

        // Свёрнут — рисуем только полоску (контент и бейзлайн не нужны).
        if is_collapsed {
            return;
        }

        // Бейзлайн полоски + акцентное подчёркивание активной вкладки поверх.
        let y = row.response.rect.bottom() + 1.0;
        let x = ui.max_rect().x_range();
        let p = ui.painter();
        p.line_segment(
            [egui::pos2(x.min, y), egui::pos2(x.max, y)],
            egui::Stroke::new(1.0, theme::BORDER),
        );
        if let Some((x0, x1)) = active_x {
            p.line_segment(
                [egui::pos2(x0, y), egui::pos2(x1, y)],
                egui::Stroke::new(2.0, theme::ACCENT),
            );
        }
        ui.add_space(8.0);

        // Контент активной вкладки — или плашка, если она откреплена в окно.
        // egui хранит высоту панели как высоту КОНТЕНТА (panel.rs: PanelState{rect}),
        // поэтому короткий контент «съёживает» док и он прыгает при смене вкладки.
        // Решение: КАЖДАЯ вкладка должна заполнять высоту целиком — лог и таблица
        // отчёта делают это через ScrollArea(auto_shrink=false); ордера и заглушки —
        // через fill_rest ниже (НЕ оба сразу, иначе высота «храповиком» растёт).
        if detached[active.idx()] {
            detached_placeholder(ui, &mut out, *active);
        } else {
            content_ui(ui, *active, report, orders);
        }
    });
    out
}

/// Кнопка свернуть/развернуть док. Шеврон рисуем painter'ом (а не глифом ▾/▴ —
/// в Geist Mono их нет, выходил «тофу»-квадрат). Вниз ∨ — свернуть, вверх ∧ —
/// развернуть. Фон/рамка/ховер — как у seg_btn.
fn collapse_button(ui: &mut egui::Ui, collapsed: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let round = egui::Rounding::same(4.0);
        let (fill, border) = if hovered {
            (theme::LIFT_HOVER, theme::ACCENT.gamma_multiply(0.55))
        } else {
            (theme::LIFT, theme::BORDER)
        };
        let p = ui.painter();
        p.rect_filled(rect, round, fill);
        p.rect_stroke(rect, round, egui::Stroke::new(1.0, border));
        let c = rect.center();
        let (hw, hh) = (4.5, 2.6);
        let stroke = egui::Stroke::new(1.6, if hovered { theme::TEXT } else { theme::TEXT_2 });
        // collapsed → ∧ (развернуть), иначе → ∨ (свернуть).
        let (y_tip, y_arm) = if collapsed {
            (c.y - hh, c.y + hh)
        } else {
            (c.y + hh, c.y - hh)
        };
        p.line_segment([egui::pos2(c.x - hw, y_arm), egui::pos2(c.x, y_tip)], stroke);
        p.line_segment([egui::pos2(c.x, y_tip), egui::pos2(c.x + hw, y_arm)], stroke);
    }
    resp
}

/// Кнопка-вкладка (underline-стиль): top-скруглённый lift-фон у активной/наведённой,
/// текст ярче у активной. Подчёркивание активной рисует [`show`] поверх бейзлайна.
fn tab_button(ui: &mut egui::Ui, label: &str, selected: bool, _detached: bool) -> egui::Response {
    let f = if selected {
        theme::font_bold()
    } else {
        theme::font()
    };
    let h = 28.0;
    let w = theme::text_w(ui, label, &f) + 24.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let round = egui::Rounding {
            nw: 5.0,
            ne: 5.0,
            sw: 0.0,
            se: 0.0,
        };
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, round, theme::LIFT);
        } else if hovered {
            p.rect_filled(rect, round, theme::LIFT.gamma_multiply(0.55));
        }
        let fg = if selected || hovered {
            theme::TEXT
        } else {
            theme::TEXT_2
        };
        p.text(rect.center(), egui::Align2::CENTER_CENTER, label, f, fg);
    }
    resp
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
    fill_rest(ui);
}

/// Заглушка контента вкладки (пока нет данных/модуля).
fn placeholder(ui: &mut egui::Ui, text: String) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(text).weak());
    });
    fill_rest(ui);
}

/// Заполнить остаток высоты дока пустым местом — чтобы короткий контент (заглушка,
/// плашка, пустой лог) не «съёживал» панель (egui хранит высоту = высоту контента).
/// Применять ТОЛЬКО на вкладках БЕЗ заполняющего ScrollArea(auto_shrink=false),
/// иначе два филлера дают рост высоты «храповиком».
pub fn fill_rest(ui: &mut egui::Ui) {
    let rest = ui.available_height();
    if rest > 0.0 {
        ui.allocate_space(egui::vec2(ui.available_width(), rest));
    }
}
