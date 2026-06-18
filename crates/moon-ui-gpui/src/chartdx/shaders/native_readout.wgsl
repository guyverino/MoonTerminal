struct ReadoutRect {
    dst: vec4<f32>,
    bg: vec4<f32>,
    border: vec4<f32>,
    m: vec4<f32>,
};

struct ReadoutGlyph {
    dst: vec4<f32>,
    color: vec4<f32>,
    m: vec4<u32>,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

fn glyph_packed_lo(code: u32) -> u32 {
    switch code {
        case 48u: { return 26922542u; }
        case 49u: { return 4329860u; }
        case 50u: { return 4261422u; }
        case 51u: { return 1508414u; }
        case 52u: { return 33106114u; }
        case 53u: { return 2048543u; }
        case 54u: { return 18825478u; }
        case 55u: { return 8521791u; }
        case 56u: { return 18302510u; }
        case 57u: { return 1558062u; }
        case 58u: { return 4198528u; }
        case 46u: { return 0u; }
        case 45u: { return 458752u; }
        case 43u: { return 5214336u; }
        case 47u: { return 8521761u; }
        default: { return 0u; }
    }
}

fn glyph_packed_hi(code: u32) -> u32 {
    switch code {
        case 48u: { return 465u; }
        case 49u: { return 452u; }
        case 50u: { return 1000u; }
        case 51u: { return 961u; }
        case 52u: { return 66u; }
        case 53u: { return 961u; }
        case 54u: { return 465u; }
        case 55u: { return 264u; }
        case 56u: { return 465u; }
        case 57u: { return 386u; }
        case 58u: { return 4u; }
        case 46u: { return 396u; }
        case 45u: { return 0u; }
        case 43u: { return 4u; }
        case 47u: { return 528u; }
        default: { return 0u; }
    }
}

fn glyph_row(code: u32, row: u32) -> u32 {
    if row < 5u {
        return (glyph_packed_lo(code) >> (row * 5u)) & 31u;
    }
    return (glyph_packed_hi(code) >> ((row - 5u) * 5u)) & 31u;
}

@group(0) @binding(0) var<storage, read> rects: array<ReadoutRect>;
@group(0) @binding(1) var<storage, read> glyphs: array<ReadoutGlyph>;

struct RectOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dst: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) border: vec4<f32>,
    @location(4) border_width: f32,
};

@vertex
fn readout_rect_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> RectOut {
    let r = rects[iid];
    let c = CORNERS_01[vid];
    let px = r.dst.xy + c * r.dst.zw;
    var out: RectOut;
    out.pos = to_clip(px, r.m.yz);
    out.uv = c;
    out.dst = r.dst;
    out.bg = r.bg;
    out.border = r.border;
    out.border_width = max(r.m.x, 0.0);
    return out;
}

@fragment
fn readout_rect_fragment(in: RectOut) -> @location(0) vec4<f32> {
    let px = in.uv * in.dst.zw;
    let edge = min(min(px.x, in.dst.z - px.x), min(px.y, in.dst.w - px.y));
    if edge <= in.border_width {
        return in.border;
    }
    return in.bg;
}

struct GlyphOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @interpolate(flat) @location(2) code: u32,
};

@vertex
fn readout_glyph_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> GlyphOut {
    let g = glyphs[iid];
    let c = CORNERS_01[vid];
    let px = g.dst.xy + c * g.dst.zw;
    let resolution = vec2<f32>(bitcast<f32>(g.m.y), bitcast<f32>(g.m.z));
    var out: GlyphOut;
    out.pos = to_clip(px, resolution);
    out.uv = c;
    out.color = g.color;
    out.code = g.m.x;
    return out;
}

@fragment
fn readout_glyph_fragment(in: GlyphOut) -> @location(0) vec4<f32> {
    let col = min(u32(floor(in.uv.x * 5.0)), 4u);
    let row = min(u32(floor(in.uv.y * 7.0)), 6u);
    let bits = glyph_row(in.code, row);
    let mask = 1u << (4u - col);
    if (bits & mask) != 0u {
        return in.color;
    }
    return vec4<f32>(in.color.rgb, 0.0);
}
