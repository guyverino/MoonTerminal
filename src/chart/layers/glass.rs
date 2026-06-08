//! Слой 4: стакан (glass) — фон зоны + бары глубины/линии уровней.

use super::{make_pipeline, InstanceBuf};
use crate::chart::data::LevelInstance;

const BARS_SHADER: &str = concat!(
    include_str!("../../../shaders/common.wgsl"),
    include_str!("../../../shaders/glass.wgsl"),
);
const BG_SHADER: &str = concat!(
    include_str!("../../../shaders/common.wgsl"),
    include_str!("../../../shaders/glass_bg.wgsl"),
);

const ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32, 1 => Float32, 2 => Float32, 3 => Float32];

pub struct GlassLayer {
    bg_pipeline: wgpu::RenderPipeline,
    pipeline: wgpu::RenderPipeline,
    instances: InstanceBuf,
}

impl GlassLayer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        chart_layout: &wgpu::BindGroupLayout,
        style_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let bg_pipeline = make_pipeline(
            device,
            format,
            &[chart_layout, style_layout],
            BG_SHADER,
            "glass-bg",
            &[],
            None,
        );
        let vbuf = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LevelInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        };
        let pipeline = make_pipeline(
            device,
            format,
            &[chart_layout, style_layout],
            BARS_SHADER,
            "glass",
            &[vbuf],
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        Self {
            bg_pipeline,
            pipeline,
            instances: InstanceBuf::new(),
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[LevelInstance]) {
        self.instances
            .upload(device, queue, bytemuck::cast_slice(data), data.len() as u32);
    }

    pub fn render<'a>(
        &'a self,
        rpass: &mut wgpu::RenderPass<'a>,
        chart_bg: &'a wgpu::BindGroup,
        style_bg: &'a wgpu::BindGroup,
    ) {
        // Фон зоны стакана (всегда, даже при пустом стакане).
        rpass.set_pipeline(&self.bg_pipeline);
        rpass.set_bind_group(0, chart_bg, &[]);
        rpass.set_bind_group(1, style_bg, &[]);
        rpass.draw(0..6, 0..1);

        // Бары/линии уровней.
        let (Some(buf), count) = (self.instances.buf.as_ref(), self.instances.count) else {
            return;
        };
        if count == 0 {
            return;
        }
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, chart_bg, &[]);
        rpass.set_bind_group(1, style_bg, &[]);
        rpass.set_vertex_buffer(0, buf.slice(..));
        rpass.draw(0..6, 0..count);
    }
}
