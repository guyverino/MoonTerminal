//! Offscreen-канвас крестиков (Stage 2c). Тики рисуются в текстуру, а в swapchain
//! блитятся одним fullscreen-проходом. База для UV-scroll/append: smooth
//! wall-clock движение без перерисовки всех крестиков каждый кадр.
//!
//! 2c-1 (этот шаг): bake каждый кадр + блит 1:1 (scroll=0). Выигрыша нет — это
//! верификация координат/формата/композита. 2c-2 добавит scroll/append/re-bake.

use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

use super::layers::CrossesLayer;
use super::transform::{ChartGlobals, ChartUniform};
use super::view::Rect;

const SHADER: &str = include_str!("../../shaders/canvas.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlitUniform {
    viewport: [f32; 4],
    resolution: [f32; 2],
    canvas_size: [f32; 2],
    scroll_px: f32,
    _pad: [f32; 3],
}

/// Запас канваса справа от экрана (px): сколько можно проскроллить вперёд UV-
/// сдвигом до полного re-bake. Больше — реже re-bake, больше памяти.
pub const MARGIN_PX: u32 = 1024;

pub struct ChartCanvas {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    _tex: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    /// Uniform для рендера крестиков В канвас (viewport = весь канвас).
    globals: ChartGlobals,
    sampler: wgpu::Sampler,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    blit_uniform: wgpu::Buffer,
    blit_bind: Option<wgpu::BindGroup>,

    // Состояние запечённого канваса (Stage 2c-2). Канвас валиден, пока Y/зум не
    // менялись и экран не вышел за пределы [0, W-chart_w] по скроллу.
    baked: bool,
    bake_time0: f32,
    bake_ppm: f32,
    bake_center: f32,
    bake_range: f32,
    /// Сколько инстансов тиков уже отрисовано в канвас (append рисует хвост).
    baked_count: u32,
    /// Значение TickRing::dropped на момент bake — при росте индексы съехали.
    bake_dropped: u64,
}

impl ChartCanvas {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let globals = ChartGlobals::new(device);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let blit_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("canvas-blit-uniform"),
            contents: bytemuck::bytes_of(&BlitUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas-blit-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas-blit"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas-blit"),
            bind_group_layouts: &[&blit_layout],
            push_constant_ranges: &[],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas-blit"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            size: (0, 0),
            format,
            _tex: None,
            view: None,
            globals,
            sampler,
            blit_pipeline,
            blit_layout,
            blit_uniform,
            blit_bind: None,
            baked: false,
            bake_time0: 0.0,
            bake_ppm: 0.0,
            bake_center: 0.0,
            bake_range: 0.0,
            baked_count: 0,
            bake_dropped: 0,
        }
    }

    pub fn width(&self) -> u32 {
        self.size.0
    }

    pub fn baked_count(&self) -> u32 {
        self.baked_count
    }

    /// Целочисленный сдвиг сэмпла канваса по X для текущего левого края экрана.
    pub fn scroll_px(&self, screen_time0: f32, ppm: f32) -> f32 {
        ((screen_time0 - self.bake_time0) * ppm).round()
    }

    /// Нужен ли полный re-bake: нет кэша, сменился Y/зум, или экран вышел за
    /// пределы запечённой полосы [0, W-chart_w].
    #[allow(clippy::too_many_arguments)]
    pub fn need_rebake(
        &self,
        screen_time0: f32,
        ppm: f32,
        center: f32,
        range: f32,
        chart_w: f32,
        dropped: u64,
    ) -> bool {
        if !self.baked {
            return true;
        }
        // ring срезал начало → индексы инстансов съехали, append невалиден.
        if dropped != self.bake_dropped {
            return true;
        }
        if ppm != self.bake_ppm || center != self.bake_center || range != self.bake_range {
            return true;
        }
        let scroll = self.scroll_px(screen_time0, ppm);
        scroll < 0.0 || scroll + chart_w > self.size.0 as f32
    }

    /// (Пере)создаёт текстуру канваса при смене размера chart-области.
    pub fn ensure(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        let w = w.max(1);
        let h = h.max(1);
        if self.size == (w, h) && self.view.is_some() {
            return;
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas-tex"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.blit_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas-blit-bind"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.blit_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
        self.view = Some(view);
        self._tex = Some(tex);
        self.size = (w, h);
        self.baked = false; // новая текстура → форсим re-bake
    }

    /// Полный re-bake: clear + рисуем инстансы [start, total) с `uniform` (его
    /// view_time0 = `bake_time0`). Запоминаем привязку для последующих append.
    #[allow(clippy::too_many_arguments)]
    pub fn rebake(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        crosses: &CrossesLayer,
        uniform: &ChartUniform,
        start: u32,
        total: u32,
        bake_time0: f32,
        ppm: f32,
        center: f32,
        range: f32,
        dropped: u64,
    ) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        self.globals.update(queue, uniform);
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas-rebake"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        crosses.render(&mut rpass, &self.globals.bind_group, start, total.saturating_sub(start));
        drop(rpass);
        self.bake_time0 = bake_time0;
        self.bake_ppm = ppm;
        self.bake_center = center;
        self.bake_range = range;
        self.baked_count = total;
        self.bake_dropped = dropped;
        self.baked = true;
    }

    /// Append: дорисовываем новые инстансы [start, total) поверх канваса (LOAD,
    /// без clear), с уже запечённой геометрией (globals не трогаем).
    pub fn append(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        crosses: &CrossesLayer,
        start: u32,
        total: u32,
    ) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        if total <= start {
            return;
        }
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas-append"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        crosses.render(&mut rpass, &self.globals.bind_group, start, total - start);
        drop(rpass);
        self.baked_count = total;
    }

    /// Блитит канвас в активный swapchain render pass (поверх grid).
    pub fn composite<'a>(
        &'a self,
        queue: &wgpu::Queue,
        rpass: &mut wgpu::RenderPass<'a>,
        chart_area: Rect,
        resolution: [f32; 2],
        scroll_px: f32,
    ) {
        let Some(bind) = self.blit_bind.as_ref() else {
            return;
        };
        let u = BlitUniform {
            viewport: [chart_area.x, chart_area.y, chart_area.w, chart_area.h],
            resolution,
            canvas_size: [self.size.0 as f32, self.size.1 as f32],
            scroll_px,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.blit_uniform, 0, bytemuck::bytes_of(&u));
        rpass.set_pipeline(&self.blit_pipeline);
        rpass.set_bind_group(0, bind, &[]);
        rpass.draw(0..6, 0..1);
    }
}
