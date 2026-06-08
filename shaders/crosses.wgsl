// Слой 2 (часть): тики-крестики. Склеивается с common.wgsl.
@group(0) @binding(0) var<uniform> chart: Chart;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) side: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) time_rel_ms: f32,
    @location(1) price: f32,
    @location(2) side: f32,
) -> VsOut {
    let corner = quad_corner(vi);
    // Снап центра к пиксельной сетке — крестик 7×7 рисуется чётко по пикселям.
    let c = data_to_px(chart, time_rel_ms, price);
    let center = vec2<f32>(round(c.x), round(c.y));
    let px = center + corner * chart.marker_half_px;
    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, chart.resolution), 0.0, 1.0);
    out.uv = corner;
    out.side = side;
    return out;
}

// Фирменная матрица тика MoonBot «Normal Trade X» (7×7): бочкообразное ядро
// 5×3 + полные «плечи» + «усики» с разрывом по центру сверху/снизу.
//   r0/r6: ### . ###   r1/r5: #######   r2..r4: . ##### .
fn cross_on(r: i32, c: i32) -> bool {
    if (r == 0 || r == 6) {
        return c != 3;
    }
    if (r == 1 || r == 5) {
        return true;
    }
    return c >= 1 && c <= 5;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // uv в [-1,1] → ячейка 0..6 матрицы 7×7.
    let col = clamp(i32(floor((in.uv.x * 0.5 + 0.5) * 7.0)), 0, 6);
    let row = clamp(i32(floor((in.uv.y * 0.5 + 0.5) * 7.0)), 0, 6);
    if (!cross_on(row, col)) {
        discard;
    }
    // Палитра MoonBot: buy graphGreen #218331, sell graphRed (Coral) #FF7F50.
    let buy = vec3<f32>(0.1294, 0.5137, 0.1922);
    let sell = vec3<f32>(1.0, 0.4980, 0.3137);
    let rgb = mix(buy, sell, in.side);
    return vec4<f32>(rgb, 1.0);
}
