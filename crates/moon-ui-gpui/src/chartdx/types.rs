//! Backend-neutral GPU structs shared by DX11, Metal, and wgpu chart passes.

/// One trade marker in GPU memory. Layout matches chart shaders.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChartCross {
    pub time_rel: f32,
    pub price: f32,
    pub side: u32,
    pub qty: f32,
}

/// Chart transform uniform. Keep field order in sync with HLSL/MSL/WGSL.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChartViewGpu {
    pub bounds: [f32; 4],
    pub resolution: [f32; 2],
    pub time_to_px: f32,
    pub view_time0: f32,
    pub price_to_px: f32,
    pub view_price0: f32,
    pub marker_half: f32,
    pub pad: f32,
    pub volume_buy_inv: f32,
    pub volume_sell_inv: f32,
    pub volume_alpha: f32,
    pub _pad2: f32,
}

/// Quad blit/background uniform.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlitParams {
    pub dst: [f32; 4],
    pub resolution: [f32; 2],
    pub uv_off: [f32; 2],
    pub uv_scale: [f32; 2],
    pub pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BackgroundParams {
    pub dst: [f32; 4],
    pub resolution: [f32; 2],
    pub uv_off: [f32; 2],
    pub uv_scale: [f32; 2],
    pub opacity: f32,
    pub _pad: f32,
    pub bg: [f32; 4],
}

impl Default for BackgroundParams {
    fn default() -> Self {
        Self {
            dst: [0.0, 0.0, 1.0, 1.0],
            resolution: [1.0, 1.0],
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            opacity: 0.0,
            _pad: 0.0,
            bg: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridParams {
    pub bounds: [f32; 4],
    pub resolution: [f32; 2],
    pub n_vert: f32,
    pub price_to_px: f32,
    pub view_price0: f32,
    pub price_interval: f32,
    pub grid_alpha: f32,
    pub bg_alpha: f32,
    pub bg: [f32; 4],
    pub grid_col: [f32; 4],
}

/// Native cursor/crosshair overlay. Coordinates are physical window pixels.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CursorParams {
    /// Combined chart+book area: x, y, w, h.
    pub bounds: [f32; 4],
    pub resolution: [f32; 2],
    pub cursor: [f32; 2],
    pub color: [f32; 4],
    pub thickness: f32,
    pub enabled: f32,
    pub _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BookStyle {
    pub book_bg: [f32; 4],
    pub bid: [f32; 4],
    pub ask: [f32; 4],
}

impl Default for BookStyle {
    fn default() -> Self {
        Self {
            book_bg: [0.0745, 0.0784, 0.0863, 1.0],
            bid: [0.1294, 0.5137, 0.1922, 1.0],
            ask: [1.0, 0.4980, 0.3137, 1.0],
        }
    }
}

pub fn cover_uv(dst_w: f32, dst_h: f32, img_aspect: f32) -> ([f32; 2], [f32; 2]) {
    let dst_aspect = dst_w.max(1.0) / dst_h.max(1.0);
    if img_aspect > dst_aspect {
        let u = (dst_aspect / img_aspect).clamp(0.0, 1.0);
        ([(1.0 - u) * 0.5, 0.0], [u, 1.0])
    } else {
        let v = (img_aspect / dst_aspect).clamp(0.0, 1.0);
        ([0.0, (1.0 - v) * 0.5], [1.0, v])
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HLineGpu {
    pub color: [f32; 4],
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ZoneGpu {
    pub color: [f32; 4],
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegGpu {
    pub pts: [f32; 4],
    pub color: [f32; 4],
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerGpu {
    pub color: [f32; 4],
    pub pos: [f32; 4],
    pub m: [f32; 4],
}
