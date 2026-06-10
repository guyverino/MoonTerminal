// Слой 5: маркер на линии ордера. shape<0.5 — крест под 45° (X) в точке начала/
// конца (плечи = size, толщина = thickness). shape>=0.5 — узелок-точка (диск
// радиуса size) на перестановке. Координата (t_rel, price) → пиксель по chart-uniform.
@group(0) @binding(0) var<uniform> chart: Chart;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>, // смещение от центра, px
    @location(2) shape: f32,
    @location(3) thickness: f32,
    @location(4) size: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) t_rel: f32,
    @location(1) price: f32,
    @location(2) size: f32,
    @location(3) thickness: f32,
    @location(4) shape: f32,
    @location(5) color: vec4<f32>,
) -> VsOut {
    let corner = quad_corner(vi);
    let c = data_to_px(chart, t_rel, price);
    let center = vec2<f32>(round(c.x), round(c.y));
    let half = max(size, 1.0);
    let px = center + corner * half;

    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, chart.resolution), 0.0, 1.0);
    out.color = color;
    out.local = corner * half;
    out.shape = shape;
    out.thickness = thickness;
    out.size = half;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (in.shape < 0.5) {
        // Крест 45°: расстояние до двух диагоналей; внутри полутолщины — рисуем.
        let h = max(in.thickness, 1.0) * 0.5;
        let d1 = abs(in.local.x - in.local.y) * 0.70710678; // /sqrt(2)
        let d2 = abs(in.local.x + in.local.y) * 0.70710678;
        if (min(d1, d2) > h) {
            discard;
        }
    } else {
        // Узелок: диск радиуса size.
        if (length(in.local) > in.size) {
            discard;
        }
    }
    return vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
}
