//! Вкладка «Ордера»: таблица открытых ордеров группы.
//! Колонки: Ядро · Сторона · Токен · Size · SL · TS · VStop · Buy · Цена · Fill · Strat.
//! Рисуется в переданный `ui` (контент-область дока с вкладками — см. `dock::tabs`).

use crate::feed::OrderRow;
use crate::shell::theme;

pub fn ui(ui: &mut egui::Ui, rows: &[(String, OrderRow)]) {
    ui.label(egui::RichText::new(t!("orders.title", count = rows.len())).weak());
    ui.add_space(2.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("orders_grid")
            .striped(true)
            .num_columns(11)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                // Тех-сокращения (Size/SL/TS/VStop/Buy/Fill/Strat) не переводим.
                let headers = [
                    t!("orders.col.core").to_string(),
                    t!("orders.col.side").to_string(),
                    t!("orders.col.token").to_string(),
                    "Size".to_string(),
                    "SL".to_string(),
                    "TS".to_string(),
                    "VStop".to_string(),
                    "Buy".to_string(),
                    t!("orders.col.price").to_string(),
                    "Fill".to_string(),
                    "Strat".to_string(),
                ];
                for h in headers {
                    ui.label(egui::RichText::new(h).weak());
                }
                ui.end_row();

                if rows.is_empty() {
                    ui.label(egui::RichText::new(t!("orders.empty")).weak());
                    ui.end_row();
                }
                for (core, r) in rows {
                    ui.label(core);
                    let (side, color) = if r.is_short {
                        ("SHORT", theme::RED)
                    } else {
                        ("LONG", theme::GREEN)
                    };
                    ui.label(egui::RichText::new(side).color(color));
                    ui.label(&r.market);
                    ui.label(format!("{:.4}", r.size));
                    on_off(ui, r.sl_on);
                    on_off(ui, r.ts_on);
                    on_off(ui, r.vstop_on);
                    ui.label(format!("{:.4}", r.buy_price));
                    ui.label(format!("{:.4}", r.price));
                    ui.label(format!("{:.0}%", r.fill_pct));
                    ui.label(&r.strat);
                    ui.end_row();
                }
            });
    });
}

fn on_off(ui: &mut egui::Ui, on: bool) {
    if on {
        ui.label(egui::RichText::new("ON").color(theme::GREEN));
    } else {
        ui.label(egui::RichText::new("OFF").color(theme::MUTED));
    }
}
