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
        super::tabs::fill_rest(ui); // не дать пустому логу «съёжить» док
        return;
    }

    // Точная высота строки = высота моноширинного шрифта (одна строка = один
    // галлей). Иначе фактическая высота не совпадёт с row_h и stick_to_bottom
    // начинает «дребезжать» (туда-сюда) при автоскролле вниз.
    let font = egui::FontId::monospace(FONT_PX);
    let row_h = ui.fonts(|f| f.row_height(&font));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show_rows(ui, row_h, lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0; // без зазора между строками
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for line in &lines[range] {
                row(ui, line, &font);
            }
        });
}

fn row(ui: &mut egui::Ui, line: &LogLine, font: &egui::FontId) {
    use egui::text::{LayoutJob, TextFormat};
    // Время — только HH:MM:SS.mmm (дата в логе одного дня избыточна).
    let time = line.ts.rsplit(' ').next().unwrap_or(line.ts.as_str());
    let (tag, col) = level_tag(line.level);
    let flat = line.msg.replace('\n', " ⏎ ");

    // Вся строка — ОДИН галлей (LayoutJob), без переноса: сегменты разного цвета,
    // но высота ровно одна строка → show_rows/stick_to_bottom не дрожат.
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let fmt = |color| TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    job.append(&format!("{time} "), 0.0, fmt(theme::MUTED));
    job.append(&format!("{tag} "), 0.0, fmt(col));
    job.append(&format!("{}  ", line.target), 0.0, fmt(theme::MUTED));
    job.append(&flat, 0.0, fmt(theme::TEXT_2));

    let resp = ui.label(job);
    if line.msg.len() > 1 {
        resp.on_hover_text(&line.msg);
    }
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
