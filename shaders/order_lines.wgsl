// Слой 5 (Retained UI): горизонтальные линии ордеров (категория C) —
// вход/стоп/трейлинг/TP/vstop/liq/pending. Инстансы несут ЛОГИЧЕСКУЮ цену; маппинг
// price→y делает шейдер через тот же chart-uniform, что и сетка. Поэтому при
// пане/зуме/Y-scale CPU-буфер НЕ пересобирается — лишь меняется uniform.
@group(0) @binding(0) var<uniform> chart: Chart;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) style: f32,
    @location(2) xpx: f32, // экранный x для пунктира
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) price: f32,
    @location(1) color: vec4<f32>,
    @location(2) style: f32,
    @location(3) thickness: f32,
) -> VsOut {
    let corner = quad_corner(vi);
    // Снап центра линии на пиксель (как NowPhase в Delphi) — без субпиксельного
    // дрожания между кадрами.
    let base = chart.viewport.y + chart.viewport.w;            // нижний край области
    let cy = round(base - (price - chart.view_price0) * chart.price_to_px);
    let left = chart.viewport.x;
    let right = chart.viewport.x + chart.viewport.z;           // .z = ширина области
    let cx = (left + right) * 0.5;
    let half_w = (right - left) * 0.5;
    let half_h = max(thickness, 1.0) * 0.5;

    let px = vec2<f32>(cx + corner.x * half_w, cy + corner.y * half_h);
    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, chart.resolution), 0.0, 1.0);
    out.color = color;
    out.style = style;
    out.xpx = px.x;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // style >= 0.5 → пунктир: 9px штрих / 7px пропуск.
    if (in.style >= 0.5) {
        let period = 16.0;
        if (fract(in.xpx / period) > (9.0 / period)) {
            discard;
        }
    }
    return vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
}
