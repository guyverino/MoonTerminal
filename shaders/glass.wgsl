// Слой 4: стакан (glass) в стиле стенда — кумулятивная глубина + линии уровней.
@group(0) @binding(0) var<uniform> chart: Chart;

// Настраиваемая тема (group 1) — см. chart/style.rs.
struct Style {
    bg: vec4<f32>,
    grid: vec4<f32>,
    cross: vec4<f32>,
    params: vec4<f32>,
    book_bg: vec4<f32>,
    bid: vec4<f32>,  // rgb bid (sRGB)
    ask: vec4<f32>,  // rgb ask (sRGB)
};
@group(1) @binding(0) var<uniform> style: Style;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) kind: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) price: f32,
    @location(1) span_price: f32,
    @location(2) len_norm: f32,
    @location(3) kind: f32,
) -> VsOut {
    let corner = quad_corner(vi);
    let zone = chart.viewport.z;             // ширина зоны стакана (px)
    let right = chart.viewport.x + zone;     // правый край
    let len = max(len_norm * zone, 1.0);
    let cx = right - len * 0.5;

    // Геометрию fill-полос строим по ДВУМ ОКРУГЛЁННЫМ краям-ценам, а не из
    // «центр ± высота». Внутренний край полосы (цена соседнего уровня к спреду) и
    // внешний край соседней полосы — это ОДНА и та же цена, поэтому после round
    // дают один пиксель → полосы стыкуются без 1px-швов (тех самых чёрных
    // полосок) и без пропусков. bid тянем к спреду вверх (цена соседа выше),
    // ask — вниз (ниже).
    let base = chart.viewport.y + chart.viewport.w;
    let y_price = base - (price - chart.view_price0) * chart.price_to_px;
    var inner_price = price;
    if (kind < 0.5) {
        inner_price = price + span_price;       // bid: сосед к спреду выше
    } else if (kind < 1.5) {
        inner_price = price - span_price;       // ask: сосед к спреду ниже
    }
    let y_inner = base - (inner_price - chart.view_price0) * chart.price_to_px;

    var top = round(min(y_price, y_inner));
    var bot = round(max(y_price, y_inner));
    if (bot - top < 1.0) {
        bot = top + 1.0;                        // минимум 1px, без чёрных щелей
    }
    var cy_center = (top + bot) * 0.5;
    var h = bot - top;

    if (kind >= 2.0) {
        // Линия индивидуального объёма — тонкая, по центру цены.
        cy_center = round(y_price);
        h = 1.5;
    }

    let px = vec2<f32>(cx + corner.x * len * 0.5, cy_center + corner.y * h * 0.5);
    var out: VsOut;
    out.pos = vec4<f32>(px_to_clip(px, chart.resolution), 0.0, 1.0);
    out.kind = kind;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Цвета из темы: fill полупрозрачный, line — поярче и непрозрачный.
    let bid = srgb_to_linear(style.bid.rgb);
    let ask = srgb_to_linear(style.ask.rgb);
    let bid_line = srgb_to_linear(min(style.bid.rgb * 1.25, vec3<f32>(1.0)));
    let ask_line = srgb_to_linear(min(style.ask.rgb * 1.25, vec3<f32>(1.0)));
    if (in.kind < 0.5) {
        return vec4<f32>(bid, 0.82);
    } else if (in.kind < 1.5) {
        return vec4<f32>(ask, 0.82);
    } else if (in.kind < 2.5) {
        return vec4<f32>(bid_line, 1.0);
    }
    return vec4<f32>(ask_line, 1.0);
}
