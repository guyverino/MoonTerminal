//! Слой 2 (часть): тики-крестики через instanced quads.

use super::{make_pipeline, InstanceBuf};
use moon_core::data::TickInstance;

const SHADER: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    include_str!("../../shaders/crosses.wgsl"),
);

const ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32, 1 => Float32, 2 => Float32];

pub struct CrossesLayer {
    pipeline: wgpu::RenderPipeline,
    instances: InstanceBuf,
}

impl CrossesLayer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        chart_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let vbuf = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TickInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        };
        let pipeline = make_pipeline(
            device,
            format,
            &[chart_layout],
            SHADER,
            "crosses",
            &[vbuf],
            None,
        );
        Self {
            pipeline,
            instances: InstanceBuf::new(),
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[TickInstance]) {
        self.instances
            .upload(device, queue, bytemuck::cast_slice(data), data.len() as u32);
    }

    /// Рисует только видимый срез инстансов [start, start+count) — culling.
    pub fn render<'a>(
        &'a self,
        rpass: &mut wgpu::RenderPass<'a>,
        chart_bg: &'a wgpu::BindGroup,
        start: u32,
        count: u32,
    ) {
        let Some(buf) = self.instances.buf.as_ref() else {
            return;
        };
        if count == 0 || start >= self.instances.count {
            return;
        }
        let stride = std::mem::size_of::<TickInstance>() as u64;
        let begin = start as u64 * stride;
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, chart_bg, &[]);
        rpass.set_vertex_buffer(0, buf.slice(begin..));
        rpass.draw(0..6, 0..count);
    }
}
