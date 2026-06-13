//! Слой 5 (Retained UI): линии ордеров — категория C, доведённая до жизненного
//! цикла. Три лёгких direct-overlay пайплайна:
//!   1. hline   — непрерывная горизонталь во всю ширину (ликвидация, без маркеров);
//!   2. segment — отрезок линии между двумя (time,price) точками (лестница ордера);
//!   3. marker  — крест начала/конца (45°) и узелки перестановок.
//!
//! Геометрия — инстансы с ЛОГИЧЕСКИМИ координатами (time_rel/price); time→x,
//! price→y делает шейдер по chart-uniform. Объём крошечный (десятки ордеров),
//! рисуется поверх композита внутри уже идущего chart-pass.

use super::{make_pipeline, InstanceBuf};

/// Инстанс непрерывной горизонтали (ликвидация). Совпадает с order_lines.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    pub price: f32,
    pub color: [f32; 4],
    pub style: f32, // 0 = сплошная, 1 = пунктир
    pub thickness: f32,
}

/// Инстанс отрезка линии. Совпадает с order_seg.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegInstance {
    pub t0_rel: f32,
    pub p0: f32,
    pub t1_rel: f32,
    pub p1: f32,
    pub thickness: f32,
    pub dashed: f32,
    pub color: [f32; 4],
}

/// Инстанс маркера (крест/узелок). Совпадает с order_marker.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerInstance {
    pub t_rel: f32,
    pub price: f32,
    pub size: f32,
    pub thickness: f32,
    pub shape: f32, // 0 = крест, 1 = узелок
    pub color: [f32; 4],
}

const HLINE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32, 1 => Float32x4, 2 => Float32, 3 => Float32];
const SEG_ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Float32, 1 => Float32, 2 => Float32, 3 => Float32, 4 => Float32, 5 => Float32, 6 => Float32x4
];
const MARKER_ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32, 1 => Float32, 2 => Float32, 3 => Float32, 4 => Float32, 5 => Float32x4
];

const HLINE_SHADER: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    include_str!("../../shaders/order_lines.wgsl"),
);
const SEG_SHADER: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    include_str!("../../shaders/order_seg.wgsl"),
);
const MARKER_SHADER: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    include_str!("../../shaders/order_marker.wgsl"),
);

pub struct OrderLinesLayer {
    hline_pipeline: wgpu::RenderPipeline,
    seg_pipeline: wgpu::RenderPipeline,
    marker_pipeline: wgpu::RenderPipeline,
    hlines: InstanceBuf,
    segments: InstanceBuf,
    markers: InstanceBuf,
}

impl OrderLinesLayer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        chart_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let hline_pipeline = make_pipeline(
            device,
            format,
            &[chart_layout],
            HLINE_SHADER,
            "order-hline",
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LineInstance>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &HLINE_ATTRS,
            }],
            blend,
        );
        let seg_pipeline = make_pipeline(
            device,
            format,
            &[chart_layout],
            SEG_SHADER,
            "order-seg",
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<SegInstance>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &SEG_ATTRS,
            }],
            blend,
        );
        let marker_pipeline = make_pipeline(
            device,
            format,
            &[chart_layout],
            MARKER_SHADER,
            "order-marker",
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<MarkerInstance>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &MARKER_ATTRS,
            }],
            blend,
        );
        Self {
            hline_pipeline,
            seg_pipeline,
            marker_pipeline,
            hlines: InstanceBuf::new(),
            segments: InstanceBuf::new(),
            markers: InstanceBuf::new(),
        }
    }

    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        hlines: &[LineInstance],
        segments: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.hlines
            .upload(device, queue, bytemuck::cast_slice(hlines), hlines.len() as u32);
        self.segments.upload(
            device,
            queue,
            bytemuck::cast_slice(segments),
            segments.len() as u32,
        );
        self.markers.upload(
            device,
            queue,
            bytemuck::cast_slice(markers),
            markers.len() as u32,
        );
    }

    /// Линия ликвидации (hline, во всю ширину графика). Рисуется в зоне графика
    /// ДО стакана — она не должна заходить в стакан.
    pub fn render_liq<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>, chart_bg: &'a wgpu::BindGroup) {
        draw(rpass, &self.hline_pipeline, &self.hlines, chart_bg);
    }

    /// Отрезки линий ордеров + маркеры. Рисуются ПОСЛЕ стакана в общей plot-зоне
    /// (поверх стакана) — линии без конца проходят через стакан вправо.
    pub fn render_overlay<'a>(
        &'a self,
        rpass: &mut wgpu::RenderPass<'a>,
        chart_bg: &'a wgpu::BindGroup,
    ) {
        draw(rpass, &self.seg_pipeline, &self.segments, chart_bg);
        draw(rpass, &self.marker_pipeline, &self.markers, chart_bg);
    }
}

fn draw<'a>(
    rpass: &mut wgpu::RenderPass<'a>,
    pipeline: &'a wgpu::RenderPipeline,
    buf: &'a InstanceBuf,
    chart_bg: &'a wgpu::BindGroup,
) {
    let (Some(b), count) = (buf.buf.as_ref(), buf.count) else {
        return;
    };
    if count == 0 {
        return;
    }
    rpass.set_pipeline(pipeline);
    rpass.set_bind_group(0, chart_bg, &[]);
    rpass.set_vertex_buffer(0, b.slice(..));
    rpass.draw(0..6, 0..count);
}
