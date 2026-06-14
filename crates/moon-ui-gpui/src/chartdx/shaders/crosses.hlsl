// Тиковые кресты own-pass: резидентный instanced-слой в проходе GPUI.
// Кресты лежат в GPU StructuredBuffer как семантика (time_rel, price, side);
// пан/зум — смена cbuffer ChartView (юнформ), CPU массив не трогает.
// Форма 7×7 «Normal Trade X» через ROW_MASK + discard. Off-screen → вне NDC (hw clip).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — область чарта (привязка)
    float2 cv_resolution; // w, h бэкбуфера (px)
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half; // 3.5 для 7×7
    float  cv_pad;
};

struct Cross {
    float time_rel;
    float price;
    uint  side; // 0 buy / 1 sell
    uint  pad;
};

StructuredBuffer<Cross> crosses : register(t1);

struct CrossOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
    nointerpolation uint side : TEXCOORD1;
};

static const float2 CORNERS[6] = {
    float2(-1, -1), float2(1, -1), float2(-1, 1),
    float2(-1,  1), float2(1, -1), float2( 1, 1)
};

CrossOut crosses_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Cross c = crosses[iid];
    CrossOut o;

    // семантика → экранные пиксели (привязка к левому/нижнему краю области чарта)
    float sx = cv_bounds.x + (c.time_rel - cv_view_time0) * cv_time_to_px;
    float sy = cv_bounds.y + cv_bounds.w - (c.price - cv_view_price0) * cv_price_to_px;
    sx = round(sx);
    sy = round(sy);

    // off-screen по X/Y → выкинуть из NDC, растеризатор отсечёт бесплатно
    if (sx < cv_bounds.x - 8.0 || sx > cv_bounds.x + cv_bounds.z + 8.0 ||
        sy < cv_bounds.y - 8.0 || sy > cv_bounds.y + cv_bounds.w + 8.0) {
        o.pos = float4(2.0, 2.0, 0.0, 1.0);
        o.uv = float2(0.0, 0.0);
        o.side = 0u;
        return o;
    }

    float2 corner = CORNERS[vid];
    float2 px = float2(sx, sy) + corner * cv_marker_half;
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = corner;
    o.side = c.side;
    return o;
}

float4 crosses_fragment(CrossOut i) : SV_Target {
    // uv в [-1,1] → ячейка 0..6 матрицы 7×7
    int col = clamp((int)floor((i.uv.x * 0.5 + 0.5) * 7.0), 0, 6);
    int row = clamp((int)floor((i.uv.y * 0.5 + 0.5) * 7.0), 0, 6);
    // r0/r6 = c!=3 (0x77), r1/r5 = all (0x7F), r2..4 = c1..5 (0x3E)
    uint mask;
    if (row == 0 || row == 6) {
        mask = 0x77u;
    } else if (row == 1 || row == 5) {
        mask = 0x7Fu;
    } else {
        mask = 0x3Eu;
    }
    if (((mask >> (uint)col) & 1u) == 0u) {
        discard;
    }
    // Канон-палитра приложения: --long (GREEN) / --short (ORANGE) — те же, что bid/ask стакана,
    // чтобы buy-трейд и bid-книга были одного зелёного. sRGB напрямую (таргет UNORM, см. grid.hlsl).
    float3 buy  = float3(0.18431, 0.65882, 0.36078); // #2FA85C palette GREEN
    float3 sell = float3(1.0,     0.55686, 0.35294); // #FF8E5A palette ORANGE
    float3 rgb = (i.side == 0u) ? buy : sell;
    return float4(rgb, 1.0);
}
