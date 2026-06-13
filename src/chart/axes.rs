//! Шкалы графика: цена слева, время снизу + readout'ы перекрестия. Рисуются
//! ОТДЕЛЬНЫМ лёгким egui-overlay'ем (см. window/host.rs) КАЖДЫМ кадром — как
//! wgpu-перекрестие, а не в кэшируемом хроме: подписи времени привязаны к данным
//! (едут за паном/скроллом), а readout'ы перекрестия едут за курсором.
//!
//! Разметка/поведение — порт из moonweb (`coords.ts` niceInterval/priceDecimals),
//! но шрифт/цвета — проекта (Geist Mono, приглушённый текст; readout — акцент,
//! жирным). Геометрия в ЛОГИЧЕСКИХ точках egui; та же раскладка, что строит
//! `chart::render` в физ. пикселях (константы × ppp) → метки совпадают с тиками.

use egui::{Align, Align2, Color32, FontId, Pos2, Rect, Rounding, Stroke};

use crate::shell::theme;

// Размеры осей — единый источник в moon_chart (их же использует движок).
use moon_chart::{PRICE_AXIS_W, TIME_AXIS_H};

/// Срез состояния вида, нужный шкалам. Снимается ПОСЛЕ `chart::render` (значения
/// текущего кадра).
#[derive(Clone, Copy)]
pub struct AxisSnapshot {
    /// Пикселей на миллисекунду (физ.) — ширина окна времени.
    pub px_per_ms: f32,
    /// Доля окна-«будущего» справа (right_margin_frac).
    pub right_margin_frac: f32,
    /// Цена в центре зоны и видимый диапазон.
    pub render_center: f32,
    pub render_range: f32,
    /// Точка отсчёта времени (unix ms) и время у правого якоря (unix ms).
    pub epoch_ms: f64,
    pub right_time_ms: f64,
    /// Смещение локального времени от UTC, сек (для подписей часов).
    pub tz_offset_sec: i64,
}

/// «Круглый» шаг сетки цены для ~`target_lines` линий (порт niceInterval).
fn nice_interval(range: f32, target_lines: f32) -> f32 {
    let rough = range / target_lines.max(1.0);
    if !(rough > 0.0) {
        return 1.0;
    }
    let mag = 10f32.powf(rough.log10().floor());
    let n = rough / mag;
    let nice = if n < 1.5 {
        1.0
    } else if n < 3.0 {
        2.0
    } else if n < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Знаков после запятой для цены такого порядка (порт priceDecimals).
fn price_decimals(price: f32) -> usize {
    let p = price.abs();
    if p >= 1000.0 {
        1
    } else if p >= 10.0 {
        2
    } else if p >= 1.0 {
        3
    } else {
        4
    }
}

/// «Круглый» шаг времени, сек, чтобы влезло ~`target` подписей.
fn nice_time_step(window_sec: f64, target: f64) -> f64 {
    const STEPS: [f64; 16] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
        14400.0, 21600.0,
    ];
    let want = window_sec / target.max(1.0);
    for s in STEPS {
        if s >= want {
            return s;
        }
    }
    STEPS[STEPS.len() - 1]
}

/// Часы локального времени из unix ms (HH:MM:SS или HH:MM).
fn fmt_clock(unix_ms: f64, offset_sec: i64, with_sec: bool) -> String {
    let total = (unix_ms / 1000.0).floor() as i64 + offset_sec;
    let sod = ((total % 86_400) + 86_400) % 86_400;
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    if with_sec {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}")
    }
}

/// Прямоугольник текста по якорю и выравниванию (ручной, без зависимости от
/// приватных хелперов egui).
fn anchored(anchor: Pos2, align: Align2, size: egui::Vec2) -> Rect {
    let x = match align.x() {
        Align::Min => anchor.x,
        Align::Center => anchor.x - size.x * 0.5,
        Align::Max => anchor.x - size.x,
    };
    let y = match align.y() {
        Align::Min => anchor.y,
        Align::Center => anchor.y - size.y * 0.5,
        Align::Max => anchor.y - size.y,
    };
    Rect::from_min_size(Pos2::new(x, y), size)
}

/// Подпись-«плашка» (readout перекрестия): заливка-фон + текст поверх линеек.
fn chip(p: &egui::Painter, anchor: Pos2, align: Align2, txt: &str, font: FontId, fg: Color32) {
    let galley = p.layout_no_wrap(txt.to_owned(), font, fg);
    let rect = anchored(anchor, align, galley.size());
    let pad = egui::vec2(4.0, 1.0);
    p.rect_filled(rect.expand2(pad), Rounding::same(3.0), theme::LIFT);
    p.rect_stroke(
        rect.expand2(pad),
        Rounding::same(3.0),
        Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.55)),
    );
    p.galley(rect.min, galley, fg);
}

/// Рисует обе шкалы (+ readout'ы перекрестия при наличии курсора).
/// `central` — центральная зона (логич. точки), `ppp` — pixels-per-point,
/// `cursor` — позиция курсора в логич. точках (или None).
pub fn draw(
    painter: &egui::Painter,
    central: egui::Rect,
    ppp: f32,
    snap: &AxisSnapshot,
    cursor: Option<Pos2>,
) {
    if central.width() < PRICE_AXIS_W + 60.0 || central.height() < TIME_AXIS_H + 60.0 {
        return;
    }

    let glass_w = (crate::chart::GLASS_ZONE_PX / ppp.max(1e-3)).min(central.width() * 0.5);
    let plot_left = central.left() + PRICE_AXIS_W;
    let plot_right = central.right() - glass_w;
    let plot_top = central.top();
    let plot_bottom = central.bottom() - TIME_AXIS_H;
    let plot_w = (plot_right - plot_left).max(1.0);
    let plot_h = (plot_bottom - plot_top).max(1.0);

    let font = theme::label_font();
    let ink = theme::TEXT_2;
    let sep = Stroke::new(1.0, Color32::from_white_alpha(16));
    painter.vline(plot_left, plot_top..=plot_bottom, sep);
    painter.hline(plot_left..=central.right(), plot_bottom, sep);

    // ── Шкала цены (слева) ──────────────────────────────────────────────────
    let range = snap.render_range.max(1e-9);
    let center = snap.render_center;
    let y_min = center - range * 0.5;
    let px_per_price = plot_h / range; // логич. px на единицу цены
    let interval = nice_interval(range, 8.0);
    let dec = price_decimals(center);
    let label_x = plot_left - 4.0;
    let top_price = y_min + range;
    let mut p = (y_min / interval).ceil() * interval;
    let mut guard = 0;
    while p <= top_price && guard < 256 {
        let y = plot_bottom - (p - y_min) * px_per_price;
        if y >= plot_top - 1.0 && y <= plot_bottom + 1.0 {
            painter.text(
                Pos2::new(label_x, y),
                Align2::RIGHT_CENTER,
                format!("{p:.dec$}"),
                font.clone(),
                ink,
            );
        }
        p += interval;
        guard += 1;
    }

    // ── Шкала времени (снизу), привязана к ДАННЫМ → едет за паном/скроллом ───
    let window_ms = plot_w as f64 * ppp as f64 / snap.px_per_ms.max(1e-6) as f64;
    let window_sec = window_ms / 1000.0;
    // rel-время у левого края = right_rel − window (см. view::visible_x).
    let right_rel = (snap.right_time_ms - snap.epoch_ms) + window_ms * snap.right_margin_frac as f64;
    let left_unix = snap.epoch_ms + right_rel - window_ms;
    let right_unix = left_unix + window_ms;
    let px_per_ms_log = plot_w as f64 / window_ms.max(1e-6);
    let step_sec = nice_time_step(window_sec, 8.0);
    let step_ms = step_sec * 1000.0;
    let with_sec = step_sec < 60.0;
    let tz_ms = snap.tz_offset_sec as f64 * 1000.0;
    let label_y = central.bottom() - 2.0;
    // Выравниваем на «круглые» границы в ЛОКАЛЬНОМ времени.
    let mut t_local = ((left_unix + tz_ms) / step_ms).ceil() * step_ms;
    let mut guard = 0;
    while guard < 256 {
        let unix = t_local - tz_ms;
        if unix > right_unix {
            break;
        }
        let x = plot_left as f64 + (unix - left_unix) * px_per_ms_log;
        painter.text(
            Pos2::new(x as f32, label_y),
            Align2::CENTER_BOTTOM,
            fmt_clock(unix, snap.tz_offset_sec, with_sec),
            font.clone(),
            ink,
        );
        t_local += step_ms;
        guard += 1;
    }

    // ── Readout'ы перекрестия (едут за курсором) ────────────────────────────
    if let Some(c) = cursor {
        // Время под вертикальной линией — плашкой в жёлобе времени.
        if c.x >= plot_left && c.x <= plot_right {
            let unix = left_unix + (c.x - plot_left) as f64 / px_per_ms_log;
            chip(
                painter,
                Pos2::new(c.x, central.bottom() - 1.0),
                Align2::CENTER_BOTTOM,
                &fmt_clock(unix, snap.tz_offset_sec, true),
                theme::font_bold(),
                theme::ACCENT,
            );
        }
        // Цена под горизонтальной линией — плашкой в жёлобе цены, жирным.
        if c.y >= plot_top && c.y <= plot_bottom {
            let price = y_min + (plot_bottom - c.y) / px_per_price;
            chip(
                painter,
                Pos2::new(plot_left - 3.0, c.y),
                Align2::RIGHT_CENTER,
                &format!("{price:.dec$}"),
                theme::font_bold(),
                theme::ACCENT,
            );
        }
    }
}
