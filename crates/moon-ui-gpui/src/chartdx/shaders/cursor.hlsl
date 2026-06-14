// Крестик-курсор own-pass. OverScene перекрывает GPUI-крестик в плоте → рисуем own-pass
// (та же фаза, поверх данных). Две тонкие линии: вертикаль на cursor.x (высота плота),
// горизонталь на cursor.y (ширина плота+стакан). Подписи времени/цены — в желобах GPUI (выживают).
// Когда уйдём на UnderScene (свои компоненты) — крестик вернётся в GPUI, этот слой убрать.

cbuffer CursorParams : register(b0) {
    float4 cu_bounds;     // x, y, w(плот+стакан), h — область крестика (px окна)
    float2 cu_resolution; // backbuffer w, h
    float2 cu_cursor;     // позиция курсора (px окна)
    float4 cu_color;      // rgb (sRGB) + alpha
    float  cu_thickness;
    float3 cu_pad;
};

struct CurOut { float4 pos : SV_Position; };

// quad-угол [0,1] по индексу (6 вершин на линию).
static const float2 Q[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

CurOut cursor_vertex(uint vid : SV_VertexID) {
    uint seg = vid / 6;       // 0 = вертикаль, 1 = горизонталь
    float2 c = Q[vid % 6];
    float t = max(cu_thickness, 1.0);
    float2 px;
    if (seg == 0u) {
        // вертикаль: толщина по X на cu_cursor.x, высота = плот
        px = float2(round(cu_cursor.x) - t * 0.5 + c.x * t, cu_bounds.y + c.y * cu_bounds.w);
    } else {
        // горизонталь: толщина по Y на cu_cursor.y, ширина = плот+стакан
        px = float2(cu_bounds.x + c.x * cu_bounds.z, round(cu_cursor.y) - t * 0.5 + c.y * t);
    }
    CurOut o;
    o.pos = float4(px.x / cu_resolution.x * 2.0 - 1.0, 1.0 - px.y / cu_resolution.y * 2.0, 0.0, 1.0);
    return o;
}

float4 cursor_fragment(CurOut i) : SV_Target {
    // Таргет = B8G8R8A8_UNORM: пишем sRGB-цвет НАПРЯМУЮ (как GPUI/кресты), без конверсии
    // в linear — иначе крестик темнее заданного cu_color (см. grid.hlsl).
    return float4(cu_color.rgb, cu_color.a);
}
