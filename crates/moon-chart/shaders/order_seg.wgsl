// Слой 5: отрезок линии ордера между двумя точками (t0,p0)-(t1,p1) в данных.
// Толстая линия (quad по направлению сегмента). time→x, price→y делает шейдер по
// chart-uniform. Опциональный пунктир по длине в пикселях.
@group(0) @binding(0) var<uniform> chart: Chart;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) dashed: f32,
    @location(2) dist_px: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) t0: f32,
    @location(1) p0: f32,
    @location(2) t1: f32,
    @location(3) p1: f32,
    @location(4) thickness: f32,
    @location(5) dashed: f32,
    @location(6) color: vec4<f32>,
) -> VsOut {
    let a = data_to_px(chart, t0, p0);
    let b = data_to_px(chart, t1, p1);
    var dir = b - a;
    let len = max(length(dir), 1e-4);
    dir = dir / len;
    let nrm = vec2<f32>(-dir.y, dir.x) * max(thickness, 1.0) * 0.5;

    // 6 вершин квада: along ∈ {0,1} вдоль сегмента, side ∈ {-1,1} поперёк.
    var along = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    var side = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let al = along[vi];
    let sd = side[vi];
    let px = mix(a, b, al) + nrm * sd;

    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, chart.resolution), 0.0, 1.0);
    out.color = color;
    out.dashed = dashed;
    out.dist_px = len * al;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (in.dashed >= 0.5) {
        let period = 16.0;
        if (fract(in.dist_px / period) > (9.0 / period)) {
            discard;
        }
    }
    return vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
}
