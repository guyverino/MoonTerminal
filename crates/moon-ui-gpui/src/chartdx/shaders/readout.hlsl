struct ReadoutRect {
    float4 dst;
    float4 bg;
    float4 border;
    float4 m;
};

struct ReadoutGlyph {
    float4 dst;
    float4 color;
    uint4 m;
};

StructuredBuffer<ReadoutRect> rects : register(t1);
StructuredBuffer<ReadoutGlyph> glyphs : register(t2);

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

float4 to_clip(float2 px, float2 resolution) {
    return float4(px.x / resolution.x * 2.0 - 1.0,
                  1.0 - px.y / resolution.y * 2.0,
                  0.0,
                  1.0);
}

uint glyph_packed_lo(uint code) {
    if (code == 48) return 26922542u; // 0
    if (code == 49) return 4329860u;  // 1
    if (code == 50) return 4261422u;  // 2
    if (code == 51) return 1508414u;  // 3
    if (code == 52) return 33106114u; // 4
    if (code == 53) return 2048543u;  // 5
    if (code == 54) return 18825478u; // 6
    if (code == 55) return 8521791u;  // 7
    if (code == 56) return 18302510u; // 8
    if (code == 57) return 1558062u;  // 9
    if (code == 58) return 4198528u;  // :
    if (code == 46) return 0u;        // .
    if (code == 45) return 458752u;   // -
    if (code == 43) return 5214336u;  // +
    if (code == 47) return 8521761u;  // /
    return 0u;
}

uint glyph_packed_hi(uint code) {
    if (code == 48) return 465u;  // 0
    if (code == 49) return 452u;  // 1
    if (code == 50) return 1000u; // 2
    if (code == 51) return 961u;  // 3
    if (code == 52) return 66u;   // 4
    if (code == 53) return 961u;  // 5
    if (code == 54) return 465u;  // 6
    if (code == 55) return 264u;  // 7
    if (code == 56) return 465u;  // 8
    if (code == 57) return 386u;  // 9
    if (code == 58) return 4u;    // :
    if (code == 46) return 396u;  // .
    if (code == 45) return 0u;    // -
    if (code == 43) return 4u;    // +
    if (code == 47) return 528u;  // /
    return 0u;
}

uint glyph_row(uint code, uint row) {
    uint bits = row < 5u
        ? (glyph_packed_lo(code) >> (row * 5u))
        : (glyph_packed_hi(code) >> ((row - 5u) * 5u));
    return bits & 31u;
}

struct RectOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float4 dst : TEXCOORD1;
    float4 bg : COLOR0;
    float4 border : COLOR1;
    float border_width : TEXCOORD2;
};

RectOut readout_rect_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    ReadoutRect r = rects[iid];
    float2 c = CORNERS[vid];
    float2 px = r.dst.xy + c * r.dst.zw;
    RectOut o;
    o.pos = to_clip(px, r.m.yz);
    o.uv = c;
    o.dst = r.dst;
    o.bg = r.bg;
    o.border = r.border;
    o.border_width = max(r.m.x, 0.0);
    return o;
}

float4 readout_rect_fragment(RectOut i) : SV_Target {
    float2 px = i.uv * i.dst.zw;
    float edge = min(min(px.x, i.dst.z - px.x), min(px.y, i.dst.w - px.y));
    return edge <= i.border_width ? i.border : i.bg;
}

struct GlyphOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
    nointerpolation uint code : TEXCOORD1;
};

GlyphOut readout_glyph_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    ReadoutGlyph g = glyphs[iid];
    float2 c = CORNERS[vid];
    float2 px = g.dst.xy + c * g.dst.zw;
    GlyphOut o;
    o.pos = to_clip(px, float2(asfloat(g.m.y), asfloat(g.m.z)));
    o.uv = c;
    o.color = g.color;
    o.code = g.m.x;
    return o;
}

float4 readout_glyph_fragment(GlyphOut i) : SV_Target {
    uint col = min((uint)floor(i.uv.x * 5.0), 4);
    uint row = min((uint)floor(i.uv.y * 7.0), 6);
    uint bits = glyph_row(i.code, row);
    uint mask = 1u << (4u - col);
    return (bits & mask) != 0u ? i.color : float4(i.color.rgb, 0.0);
}
