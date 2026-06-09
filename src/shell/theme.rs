//! Стили egui-оболочки: палитра, шрифты, `apply` (visuals). Держим отдельно от
//! логики shell. Кастомные виджеты (seg_btn/pill/градиенты) — в
//! [`super::widgets`]; для совместимости ре-экспортируются отсюда.
//!
//! Кнопки повторяют «key-strip» стенда (F1/F2…): тонкая рамка-hairline, радиус 4,
//! lift-фон, приглушённый текст в покое и яркий + акцентная рамка при наведении.
//! Шрифт — Geist Mono (как на стенде), ttf вшиты в бинарь.

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle};

use crate::palette;

pub use super::widgets::{
    lerp_color, pill, rounded_grad, seg_btn, seg_btn_h, seg_btn_tinted, text_w,
};

/// sRGB-байты палитры → egui [`Color32`]. Единый источник цветов — [`crate::palette`].
const fn c(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

// --- Единый шрифт UI (Geist Mono). Ссылаться отсюда, не хардкодить размеры. ---
/// Базовый размер шрифта хрома (точки egui) — кнопки/пиллы/значения.
pub const FONT_SIZE: f32 = 11.5;
/// Размер мелких подписей-ярлыков (SIZE/SELL/МАСШТАБ, обратный отсчёт детекта).
pub const LABEL_SIZE: f32 = 9.5;

/// Базовый шрифт (Geist Mono Regular, [`FONT_SIZE`]).
pub fn font() -> FontId {
    FontId::proportional(FONT_SIZE)
}
/// «Жирный» шрифт (Geist Mono Bold) — для значений в пиллах (стендовый weight 600).
pub fn font_bold() -> FontId {
    FontId::new(FONT_SIZE, FontFamily::Name("bold".into()))
}
/// Шрифт мелких подписей ([`LABEL_SIZE`]).
pub fn label_font() -> FontId {
    FontId::proportional(LABEL_SIZE)
}

// --- Палитра стенда: egui-обёртки над общими байтами crate::palette ---
const BG: Color32 = c(palette::BG); // --bg
const SURFACE_1: Color32 = c(palette::SURFACE_1); // --surface-1
pub const TEXT: Color32 = c(palette::TEXT); // --text
pub const TEXT_2: Color32 = c(palette::TEXT_2); // --text-2 (приглушённый)
pub const TEXT_3: Color32 = c(palette::TEXT_3); // --text-3 (самый тусклый)
const HAIRLINE_STRONG: Color32 = c(palette::HAIRLINE_STRONG); // --hairline-strong

// Кнопки key-strip: lift-фон (белый ~2% поверх тёмного), hairline-рамка,
// при наведении — чуть светлее фон и акцентная рамка вместо box-shadow стенда.
pub const LIFT: Color32 = c(palette::LIFT); // ≈ --lift над --bg
pub const LIFT_HOVER: Color32 = c(palette::LIFT_HOVER); // ≈ --lift-hover
const LIFT_ACTIVE: Color32 = c(palette::LIFT_ACTIVE);
pub const HAIRLINE: Color32 = Color32::from_rgba_premultiplied(13, 13, 13, 13); // ≈ rgba(255,255,255,.05)
/// Рамка кнопок в покое — тонкая, еле заметная (чуть ярче hairline, чтобы не «пропадала»).
pub const BORDER: Color32 = Color32::from_rgba_premultiplied(24, 24, 24, 24); // ≈ rgba(255,255,255,.094)

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    v.dark_mode = true;
    v.panel_fill = BG;
    v.window_fill = SURFACE_1;

    let round = Rounding::same(4.0);

    // Не-интерактивные элементы (label/heading): рамка-hairline, яркий текст.
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE_STRONG);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

    // Кнопка в покое: lift-фон, hairline-рамка, приглушённый текст.
    w.inactive.weak_bg_fill = LIFT;
    w.inactive.bg_fill = LIFT;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT_2);
    w.inactive.rounding = round;
    w.inactive.expansion = 0.0;

    // Наведение: светлее фон, акцентная рамка, яркий текст (аналог hover-glow стенда).
    w.hovered.weak_bg_fill = LIFT_HOVER;
    w.hovered.bg_fill = LIFT_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.55));
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.rounding = round;
    w.hovered.expansion = 0.0;

    // Нажатие/активное: ещё светлее, сплошная акцентная рамка.
    w.active.weak_bg_fill = LIFT_ACTIVE;
    w.active.bg_fill = LIFT_ACTIVE;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.rounding = round;
    w.active.expansion = 0.0;

    // Раскрытое (combo/selectable selected): акцентная рамка, яркий текст.
    w.open.weak_bg_fill = LIFT_HOVER;
    w.open.bg_fill = LIFT_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.55));
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.rounding = round;

    // Выделение selectable_label — акцентом стенда.
    v.selection.bg_fill = ACCENT.gamma_multiply(0.22);
    v.selection.stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.55));

    // Единый размер шрифта для всего хрома (Geist Mono — уже как семейство).
    // Heading чуть крупнее (имя группы), Small — мелкие подписи.
    style.text_styles.insert(TextStyle::Body, font());
    style.text_styles.insert(TextStyle::Button, font());
    style.text_styles.insert(TextStyle::Monospace, font());
    style.text_styles.insert(TextStyle::Small, label_font());
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(16.0));

    ctx.set_style(style);
}

/// Ставит Geist Mono основным шрифтом обоих семейств (как на стенде).
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "GeistMono".to_owned(),
        FontData::from_static(include_bytes!("../../assets/fonts/GeistMono-Regular.ttf")),
    );
    fonts.font_data.insert(
        "GeistMono-Bold".to_owned(),
        FontData::from_static(include_bytes!("../../assets/fonts/GeistMono-Bold.ttf")),
    );
    // Geist Mono — первым в приоритете для обоих семейств; дефолтные egui-шрифты
    // остаются запасными (кириллица/глифы, которых нет в Geist Mono).
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "GeistMono".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "GeistMono".to_owned());
    // Именованное «жирное» семейство для значений в пиллах (стендовый weight 600).
    fonts
        .families
        .insert(FontFamily::Name("bold".into()), vec!["GeistMono-Bold".to_owned()]);

    // CJK-фолбэк: системный шрифт (китайский/японский/корейский в тикерах/именах
    // стратегий). Без него такие глифы рисуются квадратиками-«тофу». Грузим из ОС
    // (не вшиваем — он большой); добавляем ПОСЛЕДНИМ в цепочку обоих семейств.
    if let Some(bytes) = load_cjk_font() {
        fonts
            .font_data
            .insert("CJK".to_owned(), FontData::from_owned(bytes));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("CJK".to_owned());
        }
    }

    ctx.set_fonts(fonts);
}

/// Читает системный CJK-шрифт (первый найденный). `.ttc` грузится как face index 0.
fn load_cjk_font() -> Option<Vec<u8>> {
    // Windows: YaHei/SimSun; macOS: PingFang; Linux: Noto CJK (типовые пути).
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\msjh.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ];
    CANDIDATES
        .iter()
        .find_map(|p| std::fs::read(p).ok())
}

pub const ACCENT: Color32 = c(palette::ACCENT); // --accent
pub const GREEN: Color32 = c(palette::GREEN); // --long
pub const RED: Color32 = c(palette::RED); // --sl
pub const MUTED: Color32 = c(palette::TEXT_2); // --text-2 (== TEXT_2)
pub const TP: Color32 = c(palette::TP); // --tp (take-profit)

/// Зазор между соседними кнопками (точки egui). Ставим явным `add_space`, т.к.
/// кастомные кнопки на `allocate_exact_size` идут впритык.
pub const BTN_GAP: f32 = 5.0;
