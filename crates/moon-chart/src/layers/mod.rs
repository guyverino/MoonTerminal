//! Слои графика и общие хелперы для wgpu-пайплайнов.

pub mod crosses;
pub mod cursor;
pub mod glass;
pub mod grid;
pub mod order_lines;

pub use crosses::CrossesLayer;
pub use cursor::CursorLayer;
pub use glass::GlassLayer;
pub use grid::GridLayer;
pub use order_lines::{LineInstance, MarkerInstance, OrderLinesLayer, SegInstance};

/// Создаёт простой render pipeline (triangle list) с заданными layout/шейдером.
pub fn make_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    bind_layouts: &[&wgpu::BindGroupLayout],
    shader_src: &str,
    label: &str,
    vbuffers: &[wgpu::VertexBufferLayout],
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader_src.into()),
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: bind_layouts,
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: "vs_main",
            buffers: vbuffers,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// Динамический instance-буфер, растёт по мере надобности.
pub struct InstanceBuf {
    pub buf: Option<wgpu::Buffer>,
    pub count: u32,
    cap_bytes: u64,
}

impl InstanceBuf {
    pub fn new() -> Self {
        Self {
            buf: None,
            count: 0,
            cap_bytes: 0,
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8], count: u32) {
        let need = bytes.len() as u64;
        if need == 0 {
            self.count = 0;
            return;
        }
        if self.buf.is_none() || need > self.cap_bytes {
            let cap = need.next_power_of_two();
            self.buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instance-buffer"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.cap_bytes = cap;
        }
        queue.write_buffer(self.buf.as_ref().unwrap(), 0, bytes);
        self.count = count;
    }
}

impl Default for InstanceBuf {
    fn default() -> Self {
        Self::new()
    }
}
