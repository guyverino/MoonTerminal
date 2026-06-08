//! Левая панель работы с ордером: BUY/SELL/Panic/Cancel + вход в детальные настройки.
//! Пока действия только логируются — реальные торговые команды подключим с guard-ами.

use super::controls::OrderControls;
use crate::shell::theme;

#[derive(Debug, Clone, Copy)]
pub enum OrderAction {
    Buy,
    Sell,
    CancelBuy,
    PanicSell,
    OpenSettings,
}

pub fn show(ctx: &egui::Context, controls: &OrderControls) -> Option<OrderAction> {
    let mut action = None;

    egui::SidePanel::left("order")
        .exact_width(150.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(t!("order.title")).weak());
            ui.label(
                egui::RichText::new(t!("order.size", n = controls.active_size())).strong(),
            );
            ui.add_space(8.0);

            let full = egui::vec2(ui.available_width(), 40.0);

            let buy = egui::Button::new(
                egui::RichText::new("BUY").strong().color(egui::Color32::BLACK),
            )
            .fill(theme::GREEN)
            .min_size(full);
            if ui.add(buy).clicked() {
                action = Some(OrderAction::Buy);
            }

            ui.add_space(4.0);
            let sell = egui::Button::new(
                egui::RichText::new("SELL").strong().color(egui::Color32::WHITE),
            )
            .fill(theme::RED)
            .min_size(full);
            if ui.add(sell).clicked() {
                action = Some(OrderAction::Sell);
            }

            ui.add_space(10.0);
            if ui
                .add(egui::Button::new("Cancel Buy").min_size(egui::vec2(ui.available_width(), 30.0)))
                .clicked()
            {
                action = Some(OrderAction::CancelBuy);
            }

            ui.add_space(4.0);
            let panic = egui::Button::new(egui::RichText::new("PANIC SELL").strong())
                .fill(egui::Color32::from_rgb(0x7a, 0x1f, 0x1f))
                .min_size(egui::vec2(ui.available_width(), 34.0));
            if ui.add(panic).clicked() {
                action = Some(OrderAction::PanicSell);
            }

            ui.add_space(12.0);
            if ui
                .add(egui::Button::new(t!("order.settings").to_string()).min_size(egui::vec2(ui.available_width(), 28.0)))
                .clicked()
            {
                action = Some(OrderAction::OpenSettings);
            }
        });

    action
}
