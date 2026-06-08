// Фон зоны стакана: заливает viewport (= glass_area) цветом из темы.
@group(0) @binding(0) var<uniform> chart: Chart;

struct Style {
    bg: vec4<f32>,
    grid: vec4<f32>,
    cross: vec4<f32>,
    params: vec4<f32>,
    book_bg: vec4<f32>, // rgb фона стакана (sRGB)
    bid: vec4<f32>,
    ask: vec4<f32>,
};
@group(1) @binding(0) var<uniform> style: Style;

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
    return vec4<f32>(srgb_to_linear(style.book_bg.rgb), 1.0);
}
