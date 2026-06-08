//! Верхний (торговый) тулбар — порт строки `topbar-row--trade` со стенда:
//! полоски `size` (F1..F6) и `sell` (S1..S6). Плюс свои масштаб цены и
//! live-follow. Действия кнопок порта пока заглушки (лог) — прикрутим позже.
//! Пиллы TP/SL/Lev переехали в header (shell), Стратегии/Отчёт/Справка — туда же.

use super::controls::{OrderControls, ScaleAction, SCALES};
use crate::shell::theme;

/// Высота тулбара (точки egui) — нужна для позиционирования close-кнопки.
pub const TOOLBAR_H: f32 = 40.0;

/// Подписи полоски `size` и `sell` (как на стенде).
const SIZE_KEYS: [&str; 6] = ["F1", "F2", "F3", "F4", "F5", "F6"];
const SELL_KEYS: [&str; 6] = ["S1", "S2", "S3", "S4", "S5", "S6"];

/// Результат тулбара за кадр.
pub struct ToolbarOut {
    /// Новый масштаб цены, если нажата кнопка.
    pub scale: Option<ScaleAction>,
    /// Новое состояние live-follow, если переключили.
    pub set_follow: Option<bool>,
}

/// Рисует тулбар. `following` — текущее состояние live-follow (для подсветки).
pub fn show(ctx: &egui::Context, controls: &mut OrderControls, following: bool) -> ToolbarOut {
    let mut set_scale = None;
    let mut set_follow = None;

    egui::TopBottomPanel::top("toolbar")
        .exact_height(TOOLBAR_H)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(4.0);

                // --- Полоска size: F1..F6 (порт со стенда; действия позже) ---
                strip_label(ui, "SIZE");
                key_strip(ui, &SIZE_KEYS, "size");

                divider(ui);

                // --- Полоска sell: S1..S6 ---
                strip_label(ui, "SELL");
                key_strip(ui, &SELL_KEYS, "sell");

                divider(ui);

                // --- Масштаб цены (Y) — наш, кнопки в том же стиле, что и ключи ---
                strip_label(ui, &t!("toolbar.scale").to_uppercase());
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0; // зазор ставим явно ниже
                    for (i, (label, action)) in SCALES.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(theme::BTN_GAP);
                        }
                        // idx 0 = «Авто» (локализуем); проценты — нейтральны.
                        let text = if i == 0 {
                            t!("toolbar.scale_auto").to_string()
                        } else {
                            (*label).to_string()
                        };
                        if theme::seg_btn(ui, &text, controls.scale_idx == i, None, false).clicked() {
                            controls.scale_idx = i;
                            set_scale = Some(*action);
                        }
                    }
                });

                divider(ui);

                // --- Live follow — наш, та же кнопка (активна = бежим за «сейчас») ---
                let live_label = if following {
                    t!("toolbar.live").to_string()
                } else {
                    t!("toolbar.pause").to_string()
                };
                if theme::seg_btn(ui, &live_label, following, None, false)
                    .on_hover_text(t!("toolbar.live_tip").to_string())
                    .clicked()
                {
                    set_follow = Some(!following);
                }
            });
        });

    ToolbarOut {
        scale: set_scale,
        set_follow,
    }
}

/// Полоска кнопок-ключей (F1..F6 / S1..S6): фикс. 34×28, gap 3px, единый стиль
/// `seg_btn`. `kind` — для лога клика (size/sell). Действия позже.
fn key_strip(ui: &mut egui::Ui, keys: &[&str], kind: &str) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0; // зазор ставим явно
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                ui.add_space(theme::BTN_GAP);
            }
            if theme::seg_btn(ui, k, false, Some(34.0), false).clicked() {
                log::info!("[ui] {kind} {k} (todo)");
            }
        }
    });
}

/// Подпись полоски (`SIZE`/`SELL`/`МАСШТАБ`) — мелкая тусклая (стендовый .strip-label).
fn strip_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::TEXT_3).font(theme::label_font()));
}

/// Вертикальный разделитель стенда (.divider): 1px линия высотой 16px.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(9.0, ui.available_height()),
        egui::Sense::hover(),
    );
    let x = rect.center().x.round();
    let cy = rect.center().y;
    ui.painter().line_segment(
        [egui::pos2(x, cy - 8.0), egui::pos2(x, cy + 8.0)],
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(18)),
    );
}
