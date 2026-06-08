//! Слой 1: фон + сетка.

use super::make_pipeline;

const SHADER: &str = concat!(
    include_str!("../../../shaders/common.wgsl"),
    include_str!("../../../shaders/grid.wgsl"),
);

pub struct GridLayer {
    pipeline: wgpu::RenderPipeline,
}

impl GridLayer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        chart_layout: &wgpu::BindGroupLayout,
        style_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let pipeline = make_pipeline(
            device,
            format,
            &[chart_layout, style_layout],
            SHADER,
            "grid",
            &[],
            None,
        );
        Self { pipeline }
    }

    pub fn render<'a>(
        &'a self,
        rpass: &mut wgpu::RenderPass<'a>,
        chart_bg: &'a wgpu::BindGroup,
        style_bg: &'a wgpu::BindGroup,
    ) {
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, chart_bg, &[]);
        rpass.set_bind_group(1, style_bg, &[]);
        rpass.draw(0..6, 0..1);
    }
}
