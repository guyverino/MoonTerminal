// Stage 2c: композит (blit) offscreen-канваса крестиков в swapchain поверх grid.
// Канвас хранит запечённые тики; здесь сэмплим его в chart-область с UV-сдвигом
// (scroll_px) — nearest, без блюра, как требует CHART_RENDERING_TZ.

struct Blit {
    viewport: vec4<f32>,    // chart-область в swapchain: x, y, w, h (px)
    resolution: vec2<f32>,  // swapchain w, h
    canvas_size: vec2<f32>, // W_ctex, H_ctex (px текстуры канваса)
    scroll_px: f32,         // сдвиг сэмпла канваса по X (целые пиксели)
    _pad: f32,
};

@group(0) @binding(0) var<uniform> blit: Blit;
@group(0) @binding(1) var canvas_tex: texture_2d<f32>;
@group(0) @binding(2) var canvas_smp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) cpx: vec2<f32>, // пиксель ВНУТРИ chart-области (0..w, 0..h)
};

fn corner01(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    return c[i];
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let k = corner01(vi);
    let px = vec2<f32>(
        blit.viewport.x + k.x * blit.viewport.z,
        blit.viewport.y + k.y * blit.viewport.w,
    );
    var o: VsOut;
    o.pos = vec4<f32>(
        px.x / blit.resolution.x * 2.0 - 1.0,
        1.0 - px.y / blit.resolution.y * 2.0,
        0.0,
        1.0,
    );
    o.cpx = vec2<f32>(k.x * blit.viewport.z, k.y * blit.viewport.w);
    return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let cx = in.cpx.x + blit.scroll_px;
    let cy = in.cpx.y;
    let uv = vec2<f32>(cx / blit.canvas_size.x, cy / blit.canvas_size.y);
    return textureSampleLevel(canvas_tex, canvas_smp, uv, 0.0);
}
