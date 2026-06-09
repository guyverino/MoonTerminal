//! Слой 7: курсор-перекрестие. Свой uniform, рисуется поверх всего.

use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

use super::make_pipeline;
use crate::chart::view::Rect;

const SHADER: &str = concat!(
    include_str!("../../../shaders/common.wgsl"),
    include_str!("../../../shaders/cursor.wgsl"),
);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CursorUniform {
    pos: [f32; 2],
    resolution: [f32; 2],
    viewport: [f32; 4],
}

pub struct CursorLayer {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    visible: bool,
}

impl CursorLayer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        style_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cursor-uniform"),
            contents: bytemuck::bytes_of(&CursorUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cursor-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cursor-bg"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        let pipeline = make_pipeline(
            device,
            format,
            &[&layout, style_layout],
            SHADER,
            "cursor",
            &[],
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        Self {
            pipeline,
            buffer,
            bind_group,
            visible: false,
        }
    }

    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        pos: Option<(f32, f32)>,
        resolution: [f32; 2],
        area: Rect,
    ) {
        match pos {
            Some((x, y)) => {
                self.visible = true;
                let u = CursorUniform {
                    pos: [x, y],
                    resolution,
                    viewport: [area.x, area.y, area.w, area.h],
                };
                queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&u));
            }
            None => self.visible = false,
        }
    }

    pub fn render<'a>(
        &'a self,
        rpass: &mut wgpu::RenderPass<'a>,
        style_bg: &'a wgpu::BindGroup,
    ) {
        if !self.visible {
            return;
        }
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.bind_group, &[]);
        rpass.set_bind_group(1, style_bg, &[]);
        // 18 вершин: вертикаль (6) + горизонталь (6) + ореол (6).
        rpass.draw(0..18, 0..1);
    }
}
