//! Подписи осей чарта в GPUI: цена слева, время снизу + readout'ы перекрестия.
//! Порт `src/chart/axes.rs` egui-версии 1:1 — та же геометрия и тик-математика
//! (`moon_chart::axes`), но рисуем текстом GPUI поверх offscreen-картинки движка
//! (canvas-оверлей, см. main.rs), а не egui-painter'ом. Координаты — логические
//! пиксели в системе окна (origin слота = `bounds.origin`).

use gpui::{
    fill, point, px, App, Bounds, Hsla, Pixels, Point, SharedString, TextRun, Window,
};

use moon_chart::axes::{fmt_clock, nice_interval, nice_time_step, price_decimals, AxisSnapshot};
use moon_chart::{GLASS_ZONE_PX, PRICE_AXIS_W, TIME_AXIS_H};
use moon_core::palette;

/// Размер шрифта подписей (логич. px) — как `theme::FONT_SIZE` egui-версии.
const FONT_SIZE: f32 = 11.5;

/// Стиль перекрестия из темы чарта (`ChartTheme`). Рисуем крест GPUI-оверлеем
/// 1:1 с движковым cursor-слоем (см. moon-chart/shaders/cursor.wgsl).
#[derive(Clone, Copy)]
pub struct CrossStyle {
    pub color: [u8; 3],
    pub alpha: f32,
    /// Полутолщина линии (px) — как style.params.x движка.
    pub thickness: f32,
    /// Поля гало креста из темы: пока крест рисуем без гало (оверлеем), но держим
    /// в стиле для 1:1 с движком и будущего гало-оверлея.
    #[allow(dead_code)]
    pub halo_radius: f32,
    #[allow(dead_code)]
    pub halo_intensity: f32,
}

/// [u8;3] (sRGB) → Hsla без альфы.
fn rgb3(c: [u8; 3]) -> Hsla {
    gpui::rgb((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).into()
}

/// Тот же цвет с заданной альфой.
fn rgba3(c: [u8; 3], a: f32) -> Hsla {
    let mut h = rgb3(c);
    h.a = a;
    h
}

/// Высота строки для центровки текста по вертикали (origin.y = центр − height/2).
fn line_h() -> Pixels {
    px(FONT_SIZE + 4.0)
}

/// Одна подпись: шейпит строку и красит. `anchor` — точка привязки (лог. px окна),
/// `(ax, ay)` — выравнивание: ax 0=left/0.5=center/1=right, ay аналогично по Y.
#[allow(clippy::too_many_arguments)]
fn label(
    window: &mut Window,
    cx: &mut App,
    text: &str,
    anchor: Point<Pixels>,
    ax: f32,
    ay: f32,
    color: Hsla,
    font: &gpui::Font,
) {
    let ts = window.text_system().clone();
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped: SharedString = text.to_string().into();
    let line = ts.shape_line(shaped, px(FONT_SIZE), &[run], None);
    let w = line.width;
    let lh = line_h();
    let origin = point(anchor.x - w * ax, anchor.y - lh * ay);
    let _ = line.paint(origin, lh, window, cx);
}

/// Плашка readout перекрестия: заливка + рамка + текст поверх (как egui `chip`).
#[allow(clippy::too_many_arguments)]
fn chip(
    window: &mut Window,
    cx: &mut App,
    text: &str,
    anchor: Point<Pixels>,
    ax: f32,
    ay: f32,
    font: &gpui::Font,
) {
    let ts = window.text_system().clone();
    let fg = rgb3(palette::ACCENT);
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: fg,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped: SharedString = text.to_string().into();
    let line = ts.shape_line(shaped, px(FONT_SIZE), &[run], None);
    let w = line.width;
    let lh = line_h();
    let origin = point(anchor.x - w * ax, anchor.y - lh * ay);
    // Фон-плашка с паддингом 4×1 (как egui chip).
    let pad = point(px(4.0), px(1.0));
    let bg = Bounds::new(
        point(origin.x - pad.x, origin.y - pad.y),
        gpui::size(w + pad.x * 2.0, lh + pad.y * 2.0),
    );
    window.paint_quad(fill(bg, rgb3(palette::LIFT)));
    window.paint_quad(gpui::outline(bg, rgba3(palette::ACCENT, 0.55), gpui::BorderStyle::Solid));
    let _ = line.paint(origin, lh, window, cx);
}

/// Рисует обе шкалы (+ readout'ы перекрестия при наличии курсора). `bounds` —
/// слот чарта (лог. px окна), `ppp` — scale_factor, `cursor` — позиция курсора
/// в лог. px окна (или None). Порт `axes::draw` egui-версии.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    snap: &AxisSnapshot,
    cursor: Option<Point<Pixels>>,
    ppp: f32,
    cross: CrossStyle,
) {
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let right = left + width;
    let bottom = top + height;

    if width < PRICE_AXIS_W + 60.0 || height < TIME_AXIS_H + 60.0 {
        return;
    }

    let glass_w = (GLASS_ZONE_PX / ppp.max(1e-3)).min(width * 0.5);
    let plot_left = left + PRICE_AXIS_W;
    let plot_right = right - glass_w;
    let plot_top = top;
    let plot_bottom = bottom - TIME_AXIS_H;
    let plot_w = (plot_right - plot_left).max(1.0);
    let plot_h = (plot_bottom - plot_top).max(1.0);

    let font = window.text_style().font();
    let ink = rgb3(palette::TEXT_2);

    // Сепараторы (белый ~6% альфа): вертикаль у plot_left, горизонталь у plot_bottom.
    let sep = rgba3([255, 255, 255], 16.0 / 255.0);
    window.paint_quad(fill(
        Bounds::new(point(px(plot_left), px(plot_top)), gpui::size(px(1.0), px(plot_h))),
        sep,
    ));
    window.paint_quad(fill(
        Bounds::new(point(px(plot_left), px(plot_bottom)), gpui::size(px(right - plot_left), px(1.0))),
        sep,
    ));

    // ── Шкала цены (слева) ──────────────────────────────────────────────────
    let range = snap.render_range.max(1e-9);
    let center = snap.render_center;
    let y_min = center - range * 0.5;
    let px_per_price = plot_h / range;
    let interval = nice_interval(range, 8.0);
    let dec = price_decimals(center);
    let label_x = plot_left - 4.0;
    let top_price = y_min + range;
    let mut p = (y_min / interval).ceil() * interval;
    let mut guard = 0;
    while p <= top_price && guard < 256 {
        let y = plot_bottom - (p - y_min) * px_per_price;
        if y >= plot_top - 1.0 && y <= plot_bottom + 1.0 {
            label(
                window,
                cx,
                &format!("{p:.dec$}"),
                point(px(label_x), px(y)),
                1.0,
                0.5,
                ink,
                &font,
            );
        }
        p += interval;
        guard += 1;
    }

    // ── Шкала времени (снизу), привязана к ДАННЫМ → едет за паном/скроллом ───
    let window_ms = plot_w as f64 * ppp as f64 / snap.px_per_ms.max(1e-6) as f64;
    let window_sec = window_ms / 1000.0;
    let right_rel = (snap.right_time_ms - snap.epoch_ms) + window_ms * snap.right_margin_frac as f64;
    let left_unix = snap.epoch_ms + right_rel - window_ms;
    let right_unix = left_unix + window_ms;
    let px_per_ms_log = plot_w as f64 / window_ms.max(1e-6);
    let step_sec = nice_time_step(window_sec, 8.0);
    let step_ms = step_sec * 1000.0;
    let with_sec = step_sec < 60.0;
    let tz_ms = snap.tz_offset_sec as f64 * 1000.0;
    let label_y = bottom - 2.0;
    let mut t_local = ((left_unix + tz_ms) / step_ms).ceil() * step_ms;
    let mut guard = 0;
    while guard < 256 {
        let unix = t_local - tz_ms;
        if unix > right_unix {
            break;
        }
        let x = plot_left as f64 + (unix - left_unix) * px_per_ms_log;
        label(
            window,
            cx,
            &fmt_clock(unix, snap.tz_offset_sec, with_sec),
            point(px(x as f32), px(label_y)),
            0.5,
            1.0,
            ink,
            &font,
        );
        t_local += step_ms;
        guard += 1;
    }

    // ── Перекрестие + readout'ы (едут за курсором) ──────────────────────────
    if let Some(c) = cursor {
        let cx_px = f32::from(c.x);
        let cy_px = f32::from(c.y);

        // Крест: вертикаль на всю высоту plot-зоны, горизонталь на всю ширину
        // (чарт + стакан, без жёлобов) — как cursor-слой движка. Цвет/толщина из
        // темы. Ореол движка (мягкое свечение) пока не воспроизводим (нужен
        // радиальный градиент; крест-линии — главный элемент).
        let line = rgba3(cross.color, cross.alpha);
        let lw = cross.thickness.max(1.0);
        if cx_px >= plot_left && cx_px <= right {
            window.paint_quad(fill(
                Bounds::new(
                    point(px(cx_px - lw * 0.5), px(plot_top)),
                    gpui::size(px(lw), px(plot_bottom - plot_top)),
                ),
                line,
            ));
        }
        if cy_px >= plot_top && cy_px <= plot_bottom {
            window.paint_quad(fill(
                Bounds::new(
                    point(px(plot_left), px(cy_px - lw * 0.5)),
                    gpui::size(px(right - plot_left), px(lw)),
                ),
                line,
            ));
        }

        // Время под вертикальной линией — плашкой в жёлобе времени.
        if cx_px >= plot_left && cx_px <= plot_right {
            let unix = left_unix + (cx_px - plot_left) as f64 / px_per_ms_log;
            chip(
                window,
                cx,
                &fmt_clock(unix, snap.tz_offset_sec, true),
                point(px(cx_px), px(bottom - 1.0)),
                0.5,
                1.0,
                &font,
            );
        }
        // Цена под горизонтальной линией — плашкой в жёлобе цены.
        if cy_px >= plot_top && cy_px <= plot_bottom {
            let price = y_min + (plot_bottom - cy_px) / px_per_price;
            chip(
                window,
                cx,
                &format!("{price:.dec$}"),
                point(px(plot_left - 3.0), px(cy_px)),
                1.0,
                0.5,
                &font,
            );
        }
    }
}

/// Смещение локального времени от UTC, сек (подписи часов). На не-Windows — UTC.
/// Зеркало `src/chart/overlay.rs::local_offset_sec`.
#[cfg(windows)]
pub fn local_offset_sec() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let lsec = l.wHour as i64 * 3600 + l.wMinute as i64 * 60 + l.wSecond as i64;
    let usec = u.wHour as i64 * 3600 + u.wMinute as i64 * 60 + u.wSecond as i64;
    let mut d = lsec - usec;
    if d > 43_200 {
        d -= 86_400;
    } else if d < -43_200 {
        d += 86_400;
    }
    d
}
#[cfg(not(windows))]
pub fn local_offset_sec() -> i64 {
    0
}
