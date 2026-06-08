//! Общий GPU-uniform графика: преобразование (time, price) -> пиксели -> clip.
//! Один bind group (group 0), переиспользуется слоями chart-области.

use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

/// Должен совпадать с `struct Chart` в шейдерах.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChartUniform {
    /// Прямоугольник chart-области: x, y, w, h (пиксели, top-left origin).
    pub viewport: [f32; 4],
    /// Размер всего фреймбуфера: w, h.
    pub resolution: [f32; 2],
    /// Пикселей на миллисекунду (зум по X).
    pub time_to_px: f32,
    /// Пикселей на единицу цены (зум по Y).
    pub price_to_px: f32,
    /// Относительное время (мс от epoch) у левого края области.
    pub view_time0: f32,
    /// Цена у нижнего края области.
    pub view_price0: f32,
    /// Полуразмер маркера-крестика в пикселях.
    pub marker_half_px: f32,
    pub _pad: f32,
}

/// Буфер + layout + bind group для ChartUniform.
pub struct ChartGlobals {
    pub buffer: wgpu::Buffer,
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl ChartGlobals {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chart-uniform"),
            contents: bytemuck::bytes_of(&ChartUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chart-uniform-layout"),
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
            label: Some("chart-uniform-bg"),
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

    pub fn update(&self, queue: &wgpu::Queue, u: &ChartUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(u));
    }
}
