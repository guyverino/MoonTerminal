//! Вкладка «Lines»: стиль линий ордеров (orders.toml). Правки идут в draft-конфиг
//! и применяются ЖИВО (App кормит draft-стиль в чарт каждый тик). «Сохранить»
//! пишет переносимый orders.toml. Подписи — английские (трейдинг-термины).

use super::SettingsTab;
use crate::config::{AppConfig, LineStyle};
use crate::icons::IconSet;

#[derive(Default)]
pub struct LinesTab;

/// Блок одного вида линии: цвет, толщина, маркеры начала/конца, узелки, пунктир.
/// `markers` = поддерживает ли вид маркеры/узлы (для liq — нет, только цвет/толщина).
fn line_block(ui: &mut egui::Ui, label: &str, s: &mut LineStyle, markers: bool) {
    egui::CollapsingHeader::new(label)
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut s.color);
                ui.add_space(8.0);
                ui.add(egui::Slider::new(&mut s.thickness, 0.5..=6.0).text("thickness"));
            });
            ui.checkbox(&mut s.dashed, "dashed");
            if markers {
                ui.separator();
                ui.checkbox(&mut s.start_marker, "start cross");
                ui.checkbox(&mut s.end_marker, "end cross");
                ui.add(egui::Slider::new(&mut s.marker_size, 2.0..=24.0).text("cross size"));
                ui.add(
                    egui::Slider::new(&mut s.marker_thickness, 0.5..=5.0).text("cross thickness"),
                );
                ui.checkbox(&mut s.knots, "knots");
                ui.add(egui::Slider::new(&mut s.knot_size, 1.0..=10.0).text("knot size"));
            }
        });
}

impl SettingsTab for LinesTab {
    fn title(&self) -> String {
        "Lines".to_string()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &mut AppConfig,
        _icons: &mut IconSet,
        _status: &super::CoreStatuses,
        _actions: &mut super::SettingsActions,
    ) {
        let o = &mut cfg.orders;

        ui.label(egui::RichText::new("Order lines").strong());
        ui.add_space(4.0);
        line_block(ui, "Buy", &mut o.buy, true);
        line_block(ui, "Sell", &mut o.sell, true);
        line_block(ui, "Stop", &mut o.stop, true);
        line_block(ui, "Trailing", &mut o.trailing, true);
        line_block(ui, "Take Profit", &mut o.take_profit, true);
        line_block(ui, "VStop", &mut o.vstop, true);
        line_block(ui, "Pending cond", &mut o.pending_cond, true);
        line_block(ui, "Liquidation", &mut o.liq, false);

        ui.separator();
        egui::CollapsingHeader::new("Path (trail / змейка)")
            .default_open(false)
            .show(ui, |ui| {
                ui.checkbox(&mut o.path.show, "show path");
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgb(&mut o.path.color);
                    ui.add_space(8.0);
                    ui.add(egui::Slider::new(&mut o.path.thickness, 0.5..=6.0).text("thickness"));
                });
                ui.checkbox(&mut o.path.dashed, "dashed");
            });

        ui.separator();
        ui.label(egui::RichText::new("Global").strong());
        ui.add_space(4.0);
        ui.add(egui::Slider::new(&mut o.active_alpha, 0.05..=1.0).text("active alpha"));
        // На сколько «невидим» становится отменённый/закрытый ордер (0 = почти прозрачный).
        ui.add(egui::Slider::new(&mut o.closed_alpha, 0.0..=1.0).text("cancelled/closed visibility"));
        ui.checkbox(&mut o.pending_dashed, "pending order: dashed entry");
        ui.add(
            egui::Slider::new(&mut o.max_closed_orders, 0..=5000).text("max closed orders drawn"),
        );

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("Stop/Trailing/Liq lines appear only after the entry is filled.")
                .weak(),
        );
    }
}
