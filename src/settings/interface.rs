//! Вкладка «Интерфейс»: тема оформления, разбитая по окнам/панелям
//! (график, перекрестие, стакан, панели, закрытый график). Правки идут в
//! draft-конфиг и применяются ЖИВО (App кормит draft-тему в чарт каждый тик).
//! «Сохранить» пишет переносимый theme.toml.

use super::SettingsTab;
use crate::config::AppConfig;
use crate::icons::IconSet;

#[derive(Default)]
pub struct InterfaceTab;

fn color_row(ui: &mut egui::Ui, label: String, c: &mut [u8; 3]) {
    ui.horizontal(|ui| {
        ui.color_edit_button_srgb(c);
        ui.add_space(8.0);
        ui.label(label);
    });
}

fn slider_row(ui: &mut egui::Ui, label: String, v: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.add(egui::Slider::new(v, range).text(label));
}

fn section(ui: &mut egui::Ui, title: String) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title).strong());
    ui.add_space(4.0);
}

impl SettingsTab for InterfaceTab {
    fn title(&self) -> String {
        t!("tab.interface").to_string()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &mut AppConfig,
        _icons: &mut IconSet,
        _status: &super::CoreStatuses,
        _actions: &mut super::SettingsActions,
    ) {
        let t = &mut cfg.theme;

        section(ui, t!("iface.sec_chart").to_string());
        color_row(ui, t!("iface.bg").to_string(), &mut t.bg);
        color_row(ui, t!("iface.grid").to_string(), &mut t.grid);
        slider_row(ui, t!("iface.grid_alpha").to_string(), &mut t.grid_alpha, 0.0..=1.0);

        ui.separator();
        section(ui, t!("iface.sec_cross").to_string());
        color_row(ui, t!("iface.cross").to_string(), &mut t.cross);
        slider_row(ui, t!("iface.cross_alpha").to_string(), &mut t.cross_alpha, 0.0..=1.0);
        slider_row(
            ui,
            t!("iface.cross_thickness").to_string(),
            &mut t.cross_thickness,
            0.5..=4.0,
        );
        slider_row(ui, t!("iface.halo_radius").to_string(), &mut t.halo_radius, 0.0..=120.0);
        slider_row(
            ui,
            t!("iface.halo_intensity").to_string(),
            &mut t.halo_intensity,
            0.0..=0.6,
        );

        ui.separator();
        section(ui, t!("iface.sec_book").to_string());
        color_row(ui, t!("iface.book_bg").to_string(), &mut t.book_bg);
        color_row(ui, t!("iface.book_bid").to_string(), &mut t.book_bid);
        color_row(ui, t!("iface.book_ask").to_string(), &mut t.book_ask);

        ui.separator();
        section(ui, t!("iface.sec_panels").to_string());
        color_row(ui, t!("iface.panel_bg").to_string(), &mut t.panel_bg);

        ui.separator();
        section(ui, t!("iface.sec_closed").to_string());
        color_row(ui, t!("iface.closed_bg").to_string(), &mut t.closed_bg);

        ui.add_space(10.0);
        ui.label(egui::RichText::new(t!("iface.hint")).weak());
    }
}
