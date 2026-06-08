//! Кнопка закрытия графика — в правом верхнем углу контейнера (график+стакан+
//! ордер), сразу под тулбаром. Рисуется только когда чарт открыт. Клик возвращает
//! true → окно сбрасывает открытый чарт (контейнер становится пустым/серым).

use crate::shell::theme;

/// `top_y` — низ тулбара (верх контейнера) в точках egui. `right_margin` — отступ
/// от правого края окна (ширина правого дока детектов), чтобы кнопка села в
/// правый-верхний угол области графика, а не поверх дока. Возвращает true на клик.
pub fn show(ctx: &egui::Context, top_y: f32, right_margin: f32) -> bool {
    let mut closed = false;
    egui::Area::new(egui::Id::new("chart-close"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-(6.0 + right_margin), top_y + 6.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let btn = egui::Button::new(egui::RichText::new("✕").size(14.0).color(theme::MUTED))
                .fill(egui::Color32::from_black_alpha(96))
                .min_size(egui::vec2(22.0, 22.0));
            if ui.add(btn).on_hover_text(t!("toolbar.close_chart").to_string()).clicked() {
                closed = true;
            }
        });
    closed
}
