// Слой 1: фон + сетка. Заливает chart-область и рисует решётку.
@group(0) @binding(0) var<uniform> chart: Chart;

// Настраиваемая тема (group 1) — см. chart/style.rs.
struct Style {
    bg: vec4<f32>,     // rgb фона (sRGB)
    grid: vec4<f32>,   // rgb сетки (sRGB), w = видимость
    cross: vec4<f32>,  // не используется здесь
    params: vec4<f32>, // не используется здесь
};
@group(1) @binding(0) var<uniform> style: Style;

// sRGB-формат свопчейна сам кодирует linear→sRGB при записи, поэтому цвета,
// заданные в sRGB (как в палитре/egui), нужно отдать в linear — иначе фон
// осветляется в серый.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let corner = quad_corner(vi);
    let cx = chart.viewport.x + chart.viewport.z * 0.5;
    let cy = chart.viewport.y + chart.viewport.w * 0.5;
    let px = vec2<f32>(
        cx + corner.x * chart.viewport.z * 0.5,
        cy + corner.y * chart.viewport.w * 0.5,
    );
    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, chart.resolution), 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy; // координаты в пикселях фреймбуфера
    let lx = p.x - chart.viewport.x;
    let ly = p.y - chart.viewport.y;
    let w = chart.viewport.z;
    let h = chart.viewport.w;
    // Фон/сетка — из настраиваемой темы (style). Цвета в sRGB.
    let bg = style.bg.rgb;
    let line = mix(bg, style.grid.rgb, style.grid.w);
    let sep = mix(bg, vec3<f32>(1.0), 0.05);
    // Пропорциональная решётка: 10 горизонталей × 60 вертикалей (как moonweb).
    let rows = 10.0;
    let cols = 60.0;
    let stepx = w / cols;
    let stepy = h / rows;
    let fx = abs(lx - round(lx / stepx) * stepx);
    let fy = abs(ly - round(ly / stepy) * stepy);
    var col = bg;
    if (fx < 0.55 || fy < 0.55) {
        col = line;
    }
    // Вертикальный разделитель чарт/стакан (правый край зоны графика).
    if (lx > w - 1.5) {
        col = sep;
    }
    return vec4<f32>(srgb_to_linear(col), 1.0);
}
