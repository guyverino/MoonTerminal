//! Вкладка «Лог»: живой список строк из общего in-memory буфера (`applog`).
//! Источники строк — сырые команды ядра и напечатанные `log::`-записи (через
//! `applog::TeeLogger`). Рисуется в переданный `ui`; новые строки появляются
//! снизу (автоскролл). Многострочные сообщения схлопываются в одну строку (полный
//! текст — по наведению), чтобы виртуализация `show_rows` держала ровную высоту.

use crate::applog::LogLine;
use crate::shell::theme;

/// Сколько последних строк держим в поле зрения (буфер в памяти больше).
const VIEW_LIMIT: usize = 1500;

/// Размер моноширинного шрифта строк лога (фикс — для ровной высоты `show_rows`).
const FONT_PX: f32 = 11.0;

pub fn ui(ui: &mut egui::Ui) {
    let lines = crate::applog::snapshot(VIEW_LIMIT);
    if lines.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(t!("dock.log.empty")).weak());
        });
        return;
    }

    let row_h = FONT_PX + 4.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show_rows(ui, row_h, lines.len(), |ui, range| {
            for line in &lines[range] {
                row(ui, line);
            }
        });
}

fn row(ui: &mut egui::Ui, line: &LogLine) {
    let font = egui::FontId::monospace(FONT_PX);
    // Время — только HH:MM:SS.mmm (дата в логе одного дня избыточна).
    let time = line.ts.rsplit(' ').next().unwrap_or(line.ts.as_str());
    let (tag, col) = level_tag(line.level);
    let flat = line.msg.replace('\n', " ⏎ ");

    ui.horizontal(|ui| {
        // Не переносим строки — длинные обрезаются краем панели (полный текст по
        // наведению ниже), иначе высота строки «поедет» и сломает show_rows.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new(time).font(font.clone()).color(theme::MUTED));
        ui.label(egui::RichText::new(tag).font(font.clone()).color(col));
        ui.label(egui::RichText::new(&line.target).font(font.clone()).color(theme::MUTED));
        let msg = ui.label(egui::RichText::new(flat).font(font).color(theme::TEXT_2));
        if line.msg.len() > 1 {
            msg.on_hover_text(&line.msg);
        }
    });
}

/// Бейдж уровня + цвет.
fn level_tag(level: log::Level) -> (&'static str, egui::Color32) {
    match level {
        log::Level::Error => ("ERR ", theme::RED),
        log::Level::Warn => ("WARN", theme::ACCENT),
        log::Level::Info => ("INFO", theme::TEXT_2),
        log::Level::Debug => ("DBG ", theme::MUTED),
        log::Level::Trace => ("TRC ", theme::MUTED),
    }
}
