// Общий префикс шейдеров. Склеивается с конкретным шейдером через concat! в Rust.
// Совпадает с ChartUniform (transform.rs).
struct Chart {
    viewport: vec4<f32>,   // x, y, w(width), h(height) в пикселях
    resolution: vec2<f32>, // w, h фреймбуфера
    time_to_px: f32,
    price_to_px: f32,
    view_time0: f32,
    view_price0: f32,
    marker_half_px: f32,
    _pad: f32,
};

fn px_to_clip(px: vec2<f32>, res: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(px.x / res.x * 2.0 - 1.0, 1.0 - px.y / res.y * 2.0);
}

// (time_rel_ms, price) -> пиксель внутри области (y вниз, цена вверх).
fn data_to_px(c: Chart, time_rel_ms: f32, price: f32) -> vec2<f32> {
    let x = c.viewport.x + (time_rel_ms - c.view_time0) * c.time_to_px;
    let y = c.viewport.y + c.viewport.w - (price - c.view_price0) * c.price_to_px;
    return vec2<f32>(x, y);
}

// 6 углов квада по vertex_index (две треугольника), в диапазоне [-1,1].
fn quad_corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    return c[i];
}

// sRGB-свопчейн кодирует linear→sRGB при записи, поэтому цвета, заданные в
// sRGB (палитра/egui/тема), отдаём в linear — иначе фон осветляется в серый.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}
