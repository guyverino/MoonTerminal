//! Логотип MoonBot: встроенный SVG (assets/brand/moonbot-logo.svg) растрится в
//! egui-текстуру один раз при создании Shell. SVG — только пути (без текста),
//! поэтому resvg подключён без шрифтовых фич.

use resvg::{tiny_skia, usvg};

const LOGO_SVG: &[u8] = include_bytes!("../../assets/brand/moonbot-logo.svg");

/// Растрит лого в текстуру под высоту показа `display_h` (точки egui). Растр
/// делаем ×2 от экранного размера: при 100–200% это даёт ≤2× даунскейл —
/// билинейный фильтр egui тут чистый, без «каши» от сильного ужатия большой
/// текстуры. None — если SVG не распарсился/не отрисовался (лого не покажем).
pub fn load_logo(ctx: &egui::Context, display_h: f32) -> Option<egui::TextureHandle> {
    let tree = usvg::Tree::from_data(LOGO_SVG, &usvg::Options::default()).ok()?;
    let size = tree.size();
    // Целевая высота растра ≈ ×2 от точечной высоты показа (супер-сэмпл).
    let scale = (display_h * 2.0) / size.height().max(1.0);
    let w = (size.width() * scale).ceil() as u32;
    let h = (size.height() * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia отдаёт premultiplied RGBA — ровно формат Color32.
    let pixels: Vec<egui::Color32> = pixmap
        .data()
        .chunks_exact(4)
        .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    let image = egui::ColorImage {
        size: [w as usize, h as usize],
        pixels,
    };
    Some(ctx.load_texture("moonbot-logo", image, egui::TextureOptions::LINEAR))
}
