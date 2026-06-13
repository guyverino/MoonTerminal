//! Style-uniform (group 1) для слоёв grid и cursor: настраиваемая тема
//! (фон/сетка/перекрестие/ореол). Цвета в sRGB; шейдеры конвертируют в linear.

use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

use moon_core::config::ChartTheme;

/// Должен совпадать со `struct Style` в grid.wgsl/cursor.wgsl (std140, vec4-выравн.).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StyleUniform {
    /// rgb фона (sRGB), w не используется.
    pub bg: [f32; 4],
    /// rgb сетки (sRGB), w = видимость сетки 0..1.
    pub grid: [f32; 4],
    /// rgb перекрестия (sRGB), w = прозрачность линий 0..1.
    pub cross: [f32; 4],
    /// x = полутолщина линии (px), y = радиус ореола (px), z = яркость ореола, w = 0.
    pub params: [f32; 4],
    /// rgb фона стакана (sRGB).
    pub book_bg: [f32; 4],
    /// rgb bid-стороны стакана (sRGB).
    pub bid: [f32; 4],
    /// rgb ask-стороны стакана (sRGB).
    pub ask: [f32; 4],
}

fn srgb3(c: [u8; 3]) -> [f32; 3] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    ]
}

impl StyleUniform {
    pub fn from_theme(t: &ChartTheme) -> Self {
        let bg = srgb3(t.bg);
        let g = srgb3(t.grid);
        let c = srgb3(t.cross);
        let bbg = srgb3(t.book_bg);
        let bid = srgb3(t.book_bid);
        let ask = srgb3(t.book_ask);
        Self {
            bg: [bg[0], bg[1], bg[2], 1.0],
            grid: [g[0], g[1], g[2], t.grid_alpha],
            cross: [c[0], c[1], c[2], t.cross_alpha],
            params: [t.cross_thickness, t.halo_radius, t.halo_intensity, 0.0],
            book_bg: [bbg[0], bbg[1], bbg[2], 1.0],
            bid: [bid[0], bid[1], bid[2], 1.0],
            ask: [ask[0], ask[1], ask[2], 1.0],
        }
    }
}

/// Буфер + layout + bind group для StyleUniform (общий для grid и cursor).
pub struct StyleGlobals {
    pub buffer: wgpu::Buffer,
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl StyleGlobals {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("style-uniform"),
            contents: bytemuck::bytes_of(&StyleUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("style-uniform-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("style-uniform-bg"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            buffer,
            layout,
            bind_group,
        }
    }

    pub fn update(&self, queue: &wgpu::Queue, u: &StyleUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(u));
    }
}
