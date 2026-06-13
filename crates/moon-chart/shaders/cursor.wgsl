// Слой 7: курсор-перекрестие. Тонкие линии + размытый ореол, едущий за курсором.
// Цвет/толщина/радиус ореола — из настраиваемой темы (style, group 1).
struct Cursor {
    pos: vec2<f32>,        // позиция курсора, px
    resolution: vec2<f32>, // фреймбуфер, px
    viewport: vec4<f32>,   // x, y, w, h области (чарт + стакан), px
};
@group(0) @binding(0) var<uniform> cur: Cursor;

// Настраиваемая тема (group 1) — см. chart/style.rs.
struct Style {
    bg: vec4<f32>,
    grid: vec4<f32>,
    cross: vec4<f32>,  // rgb (sRGB), w = прозрачность линий
    params: vec4<f32>, // x = полутолщина линии, y = радиус ореола, z = яркость ореола
};
@group(1) @binding(0) var<uniform> style: Style;

// px_to_clip / srgb_to_linear / quad_corner приходят из common.wgsl (склейка в cursor.rs).

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) kind: f32, // 0 = линия, 1 = ореол
    @location(1) coord: vec2<f32>,             // локальные px (перпендикуляр / радиус)
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let q = vi / 6u;   // 0 = вертикаль, 1 = горизонталь, 2 = ореол
    let ci = vi % 6u;
    let corner = quad_corner(ci);

    let lh = max(style.params.x, 0.1);   // полутолщина линии, px
    let halo = max(style.params.y, 0.1); // радиус ореола, px

    var center: vec2<f32>;
    var half: vec2<f32>;
    var kind: f32;
    var coord: vec2<f32>;

    if (q == 0u) {
        center = vec2<f32>(cur.pos.x, cur.viewport.y + cur.viewport.w * 0.5);
        half = vec2<f32>(lh, cur.viewport.w * 0.5);
        kind = 0.0;
        coord = vec2<f32>(corner.x * lh, 0.0); // перпендикуляр — x
    } else if (q == 1u) {
        center = vec2<f32>(cur.viewport.x + cur.viewport.z * 0.5, cur.pos.y);
        half = vec2<f32>(cur.viewport.z * 0.5, lh);
        kind = 0.0;
        coord = vec2<f32>(corner.y * lh, 0.0); // перпендикуляр — y
    } else {
        center = cur.pos;
        half = vec2<f32>(halo, halo);
        kind = 1.0;
        coord = corner * halo;
    }

    let px = center + corner * half;
    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, cur.resolution), 0.0, 1.0);
    out.kind = kind;
    out.coord = coord;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let color = style.cross.rgb;
    let halo = max(style.params.y, 0.1);
    var a: f32;
    if (in.kind < 0.5) {
        // Тонкая линия: пик в центре, спад к полутолщине.
        a = (1.0 - smoothstep(0.0, max(style.params.x, 0.1), abs(in.coord.x))) * style.cross.w;
    } else {
        // Размытый ореол: мягкий радиальный спад.
        let dist = length(in.coord);
        a = pow(1.0 - smoothstep(0.0, halo, dist), 2.0) * style.params.z;
    }
    return vec4<f32>(srgb_to_linear(color), a);
}
