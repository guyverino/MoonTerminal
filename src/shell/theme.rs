//! Стили egui-оболочки. Держим отдельно от логики shell.
//!
//! Кнопки повторяют «key-strip» стенда (F1/F2…): тонкая рамка-hairline, радиус 4,
//! lift-фон, приглушённый текст в покое и яркий + акцентная рамка при наведении.
//! Шрифт — Geist Mono (как на стенде), ttf вшиты в бинарь.

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle};

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

// --- Палитра стенда (см. stand-tauri/src/styles.css :root) ---
const BG: Color32 = Color32::from_rgb(0x13, 0x14, 0x16); // --bg
const SURFACE_1: Color32 = Color32::from_rgb(0x1a, 0x1c, 0x1f); // --surface-1
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe4, 0xdc); // --text
pub const TEXT_2: Color32 = Color32::from_rgb(0x97, 0x92, 0x8a); // --text-2 (приглушённый)
pub const TEXT_3: Color32 = Color32::from_rgb(0x5e, 0x5a, 0x53); // --text-3 (самый тусклый)
const HAIRLINE_STRONG: Color32 = Color32::from_rgb(0x3a, 0x3e, 0x45); // --hairline-strong

// Кнопки key-strip: lift-фон (белый ~2% поверх тёмного), hairline-рамка,
// при наведении — чуть светлее фон и акцентная рамка вместо box-shadow стенда.
pub const LIFT: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x22); // ≈ --lift над --bg
pub const LIFT_HOVER: Color32 = Color32::from_rgb(0x26, 0x28, 0x2d); // ≈ --lift-hover
const LIFT_ACTIVE: Color32 = Color32::from_rgb(0x2c, 0x2f, 0x35);
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

pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xb3, 0x47); // --accent
pub const GREEN: Color32 = Color32::from_rgb(0x2f, 0xa8, 0x5c); // --long
pub const RED: Color32 = Color32::from_rgb(0xff, 0x4a, 0x4a); // --sl
pub const MUTED: Color32 = Color32::from_rgb(0x97, 0x92, 0x8a); // --text-2
pub const TP: Color32 = Color32::from_rgb(0x7f, 0xc9, 0xff); // --tp (take-profit)

/// Зазор между соседними кнопками (точки egui). Ставим явным `add_space`, т.к.
/// кастомные кнопки на `allocate_exact_size` идут впритык.
pub const BTN_GAP: f32 = 5.0;

/// Ширина строки текста заданным шрифтом (для авто-ширины виджетов).
pub fn text_w(ui: &egui::Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts(|f| {
        f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    })
}

/// Единая кнопка-сегмент (ключи size/sell, масштаб, live, кнопки шапки): lift-фон
/// + тонкая еле видимая рамка, радиус 4, высота 28, текст по центру шрифтом темы.
/// `active` — подсветка акцентом (выбранный масштаб / включённый live). `fixed_w`
/// — фикс. ширина (ключи 34px), иначе по тексту + паддинг. На ховере — светлее
/// фон + акцентная рамка.
/// `rest_gradient` — рисовать ли лёгкий градиент-блик в ПОКОЕ (не на ховере).
/// Для обычных кнопок тулбара/шапки = false (плоские в покое, глоу лишь на
/// ховере); для «объёмных» кнопок можно передать true. Глоу на ховере — всегда.
pub fn seg_btn(
    ui: &mut egui::Ui,
    text: &str,
    active: bool,
    fixed_w: Option<f32>,
    rest_gradient: bool,
) -> egui::Response {
    seg_btn_h(ui, text, active, fixed_w, rest_gradient, 28.0)
}

/// Как [`seg_btn`], но с произвольной высотой `height`. Нужно для мест, где высота
/// строки задана извне (таблица настроек H=22) и трогать её нельзя, а вид кнопки
/// должен совпадать с основным шаблоном.
pub fn seg_btn_h(
    ui: &mut egui::Ui,
    text: &str,
    active: bool,
    fixed_w: Option<f32>,
    rest_gradient: bool,
    height: f32,
) -> egui::Response {
    use egui::{vec2, Align2, Rounding, Sense, Stroke};

    let f = font();
    let w = fixed_w.unwrap_or_else(|| text_w(ui, text, &f) + 16.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let round = Rounding::same(4.0);
        let (fill, border, fg) = if active {
            (ACCENT.gamma_multiply(0.16), ACCENT.gamma_multiply(0.70), TEXT)
        } else if hovered {
            (LIFT_HOVER, ACCENT.gamma_multiply(0.55), TEXT)
        } else {
            (LIFT, BORDER, TEXT_2)
        };
        let p = ui.painter();
        p.rect_filled(rect, round, fill);
        if rest_gradient && !hovered {
            raise_sheen(p, rect, 4.0);
        }
        if hovered {
            hover_glow(p, rect, 4.0);
        }
        p.rect_stroke(rect, round, Stroke::new(1.0, border));
        p.text(rect.center(), Align2::CENTER_CENTER, text, f, fg);
    }
    resp
}

/// Кнопка-сегмент по шаблону [`seg_btn`], но с цветным градиентом-заливкой снизу
/// для индикации «частичного» состояния. `tint = Some(color)` → лёгкий градиент от
/// прозрачного сверху к `color` снизу + рамка цвета (например, часть галок ядра
/// выключена → подсветка цветом сервера). `tint = None` → обычная серая кнопка.
pub fn seg_btn_tinted(
    ui: &mut egui::Ui,
    text: &str,
    fixed_w: Option<f32>,
    height: f32,
    tint: Option<Color32>,
) -> egui::Response {
    use egui::{vec2, Align2, Rounding, Sense, Stroke};

    let f = font();
    let w = fixed_w.unwrap_or_else(|| text_w(ui, text, &f) + 16.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let round = Rounding::same(4.0);
        let (border, fg) = if hovered {
            (ACCENT.gamma_multiply(0.55), TEXT)
        } else if let Some(c) = tint {
            (c.gamma_multiply(0.75), TEXT)
        } else {
            (BORDER, TEXT_2)
        };
        let p = ui.painter();
        p.rect_filled(rect, round, if hovered { LIFT_HOVER } else { LIFT });
        // Цветной градиент в покое (на ховере уступает место акцентному глоу).
        if let Some(c) = tint {
            if !hovered {
                let g0 = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 0);
                let g1 = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 96);
                rounded_grad(p, rect, 4.0, g0, g1, 0.30);
            }
        }
        if hovered {
            hover_glow(p, rect, 4.0);
        }
        p.rect_stroke(rect, round, Stroke::new(1.0, border));
        p.text(rect.center(), Align2::CENTER_CENTER, text, f, fg);
    }
    resp
}

/// Лёгкое акцентное свечение в нижней части кнопки (на ховере) — скруглённым
/// мешем ПО ФОРМЕ кнопки (на всю ширину, повторяя скругление), чтобы у боков не
/// оставалось пустых углов. Верхняя половина прозрачна, нижняя плавно к акценту.
fn hover_glow(p: &egui::Painter, rect: egui::Rect, radius: f32) {
    let g0 = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 0);
    let g1 = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 30);
    rounded_grad(p, rect, radius, g0, g1, 0.5);
}

/// Лёгкий блик сверху (всегда) — даёт кнопке «объём», чтобы она отрывалась от
/// тёмного фона панели и зазор между кнопками читался. Белый ~5% сверху → прозрачно.
fn raise_sheen(p: &egui::Painter, rect: egui::Rect, radius: f32) {
    rounded_grad(p, rect, radius, Color32::from_white_alpha(14), Color32::TRANSPARENT, 0.0);
}

/// Линейная интерполяция цветов по premultiplied-компонентам (для непрозрачных
/// цветов = обычный lerp). t=0 → a, t=1 → b.
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Заливка скруглённого прямоугольника вертикальным градиентом. Цвет вершины —
/// по её y: до `grad_start` (доля высоты) держим `top`, ниже плавно к `bottom`.
/// Контур = дуги в углах + срединные точки на боках; заливка — веер от центра
/// (фигура выпуклая). Повторяет скругление → нет пустых углов у градиента.
pub fn rounded_grad(
    p: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    top: Color32,
    bottom: Color32,
    grad_start: f32,
) {
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    use egui::Pos2;

    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let h = rect.height().max(1.0);
    let col = |y: f32| -> Color32 {
        let t = ((y - rect.top()) / h).clamp(0.0, 1.0);
        if t <= grad_start {
            top
        } else {
            lerp_color(top, bottom, (t - grad_start) / (1.0 - grad_start).max(1e-3))
        }
    };

    let (l, rt, tp, bt) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let cy = rect.center().y;
    let mut pts: Vec<Pos2> = Vec::new();
    let arc = |cx: f32, cyc: f32, a0: f32, a1: f32, out: &mut Vec<Pos2>| {
        const SEG: usize = 5;
        for i in 0..=SEG {
            let f = i as f32 / SEG as f32;
            let a = (a0 + (a1 - a0) * f).to_radians();
            out.push(Pos2::new(cx + r * a.cos(), cyc + r * a.sin()));
        }
    };
    arc(l + r, tp + r, 180.0, 270.0, &mut pts); // верх-лево
    arc(rt - r, tp + r, 270.0, 360.0, &mut pts); // верх-право
    pts.push(Pos2::new(rt, cy)); // правый бок — точка разлома
    arc(rt - r, bt - r, 0.0, 90.0, &mut pts); // низ-право
    arc(l + r, bt - r, 90.0, 180.0, &mut pts); // низ-лево
    pts.push(Pos2::new(l, cy)); // левый бок — точка разлома

    let mut mesh = Mesh::default();
    let center = rect.center();
    mesh.vertices.push(Vertex { pos: center, uv: WHITE_UV, color: col(center.y) });
    for pt in &pts {
        mesh.vertices.push(Vertex { pos: *pt, uv: WHITE_UV, color: col(pt.y) });
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.indices.extend_from_slice(&[0, 1 + i, 1 + (i + 1) % n]);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Пилл стенда (TP/SL/Lev): скруглённый lift-фон + hairline-рамка, слева
/// приглушённая подпись, справа «жирное» цветное значение. Ширина — по
/// содержимому (как CSS auto), высота 26px. На ховере — светлее + акцентная рамка.
pub fn pill(ui: &mut egui::Ui, label: &str, value: &str, vcolor: Color32) -> egui::Response {
    use egui::{pos2, vec2, Align2, Rounding, Sense, Stroke};

    let pad = 11.0;
    let gap = 7.0;
    let lf = font();
    let vf = font_bold();
    let lw = text_w(ui, label, &lf);
    let vw = text_w(ui, value, &vf);
    let w = pad * 2.0 + lw + gap + vw;

    let (rect, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let p = ui.painter();
        let fill = if hovered { LIFT_HOVER } else { LIFT };
        let border = if hovered { ACCENT.gamma_multiply(0.55) } else { BORDER };
        let round = Rounding::same(4.0);
        p.rect_filled(rect, round, fill);
        if hovered {
            hover_glow(p, rect, 4.0);
        }
        p.rect_stroke(rect, round, Stroke::new(1.0, border));
        let cy = rect.center().y;
        let lx = rect.left() + pad;
        p.text(pos2(lx, cy), Align2::LEFT_CENTER, label, lf, TEXT_2);
        p.text(pos2(lx + lw + gap, cy), Align2::LEFT_CENTER, value, vf, vcolor);
    }
    resp
}
