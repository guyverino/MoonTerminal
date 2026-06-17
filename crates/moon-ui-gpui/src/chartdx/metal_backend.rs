//! macOS GPUI native Metal chart backend. It renders inside GPUI's CAMetalLayer
//! command encoder via the custom GPU pass hook.

use foreign_types::ForeignTypeRef;
use gpui::RawGpuAccess;
use metal::{
    CompileOptions, DeviceRef, MTLBlendFactor, MTLBlendOperation, MTLPixelFormat, MTLPrimitiveType,
    MTLResourceOptions, MTLSamplerMinMagFilter, MTLScissorRect, MTLSize, MTLTextureUsage,
    RenderCommandEncoderRef, RenderPipelineDescriptor, RenderPipelineState, SamplerDescriptor,
    TextureDescriptor,
};
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};
use std::ffi::c_void;

use super::types::{
    BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams, GridParams, HLineGpu,
    MarkerGpu, SegGpu, ZoneGpu,
};

const SHADER: &str = include_str!("shaders/chart_native.metal");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");

fn hl_of(h: &LineInstance) -> HLineGpu {
    HLineGpu {
        color: h.color,
        m: [h.price, h.style, h.thickness, 0.0],
    }
}

fn zone_of(z: &ZoneInstance) -> ZoneGpu {
    ZoneGpu {
        color: z.color,
        m: [z.price0, z.price1, 0.0, 0.0],
    }
}

fn seg_of(s: &SegInstance) -> SegGpu {
    SegGpu {
        pts: [s.t0_rel, s.p0, s.t1_rel, s.p1],
        color: s.color,
        m: [s.thickness, s.dashed, s.extend, 0.0],
    }
}

fn mk_of(m: &MarkerInstance) -> MarkerGpu {
    MarkerGpu {
        color: m.color,
        pos: [m.t_rel, m.price, m.size, m.thickness],
        m: [m.shape, 0.0, 0.0, 0.0],
    }
}

#[derive(Default)]
struct BufferSlot {
    buffer: Option<metal::Buffer>,
    size: u64,
}

impl BufferSlot {
    fn write<T: bytemuck::Pod>(&mut self, device: &DeviceRef, label: &str, data: &[T]) {
        let bytes = bytemuck::cast_slice(data);
        let need = bytes.len().max(4) as u64;
        if self.buffer.as_ref().is_none() || self.size < need {
            let buffer = device.new_buffer(
                need.next_power_of_two(),
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined,
            );
            buffer.set_label(label);
            self.buffer = Some(buffer);
            self.size = need.next_power_of_two();
        }
        if !bytes.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.buffer.as_ref().unwrap().contents() as *mut u8,
                    bytes.len(),
                );
            }
        }
    }

    fn buffer(&self) -> &metal::BufferRef {
        self.buffer.as_ref().unwrap().as_ref()
    }
}

struct Pipelines {
    background: RenderPipelineState,
    grid: RenderPipelineState,
    cursor: RenderPipelineState,
    crosses: RenderPipelineState,
    volume: RenderPipelineState,
    price_last: RenderPipelineState,
    price_mark: RenderPipelineState,
    book_bg: RenderPipelineState,
    book_bars: RenderPipelineState,
    zone: RenderPipelineState,
    hline: RenderPipelineState,
    seg: RenderPipelineState,
    marker: RenderPipelineState,
    sampler: metal::SamplerState,
}

struct BackgroundTexture {
    texture: metal::Texture,
}

pub struct MetalLayers {
    device_generation: u64,
    pixel_format: Option<MTLPixelFormat>,
    pipelines: Option<Pipelines>,
    background_texture: Option<BackgroundTexture>,
    crosses: Vec<ChartCross>,
    last_line: Vec<PriceLinePoint>,
    mark_line: Vec<PriceLinePoint>,
    levels: Vec<LevelInstance>,
    zones: Vec<ZoneGpu>,
    hlines: Vec<HLineGpu>,
    segs: Vec<SegGpu>,
    markers: Vec<MarkerGpu>,
    volume_buy_max: f32,
    volume_sell_max: f32,
    bg_uniform: BufferSlot,
    grid_uniform: BufferSlot,
    cursor_uniform: BufferSlot,
    view_uniform: BufferSlot,
    book_view_uniform: BufferSlot,
    book_style_uniform: BufferSlot,
    cross_buffer: BufferSlot,
    last_line_buffer: BufferSlot,
    mark_line_buffer: BufferSlot,
    level_buffer: BufferSlot,
    zone_buffer: BufferSlot,
    hline_buffer: BufferSlot,
    seg_buffer: BufferSlot,
    marker_buffer: BufferSlot,
}

impl MetalLayers {
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            pixel_format: None,
            pipelines: None,
            background_texture: None,
            crosses: Vec::new(),
            last_line: Vec::new(),
            mark_line: Vec::new(),
            levels: Vec::new(),
            zones: Vec::new(),
            hlines: Vec::new(),
            segs: Vec::new(),
            markers: Vec::new(),
            volume_buy_max: 1e-6,
            volume_sell_max: 1e-6,
            bg_uniform: BufferSlot::default(),
            grid_uniform: BufferSlot::default(),
            cursor_uniform: BufferSlot::default(),
            view_uniform: BufferSlot::default(),
            book_view_uniform: BufferSlot::default(),
            book_style_uniform: BufferSlot::default(),
            cross_buffer: BufferSlot::default(),
            last_line_buffer: BufferSlot::default(),
            mark_line_buffer: BufferSlot::default(),
            level_buffer: BufferSlot::default(),
            zone_buffer: BufferSlot::default(),
            hline_buffer: BufferSlot::default(),
            seg_buffer: BufferSlot::default(),
            marker_buffer: BufferSlot::default(),
        }
    }

    pub fn reset_combo(&mut self, data: Vec<ChartCross>) {
        self.crosses = cap_tail(data, 1 << 17);
        self.recalc_volume_scale();
    }

    pub fn append_combo(&mut self, data: &[ChartCross]) {
        if data.is_empty() {
            return;
        }
        self.crosses.extend_from_slice(data);
        if self.crosses.len() > (1 << 17) {
            let drop = self.crosses.len() - (1 << 17);
            self.crosses.drain(0..drop);
        }
        self.recalc_volume_scale();
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.last_line = cap_tail(last.to_vec(), 1 << 17);
        self.mark_line = cap_tail(mark.to_vec(), 1 << 17);
    }

    pub fn set_orderbook(&mut self, levels: Vec<LevelInstance>) {
        self.levels = cap_head(levels, 1 << 12);
    }

    pub fn set_userdata(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.zones = cap_head(zones.iter().map(zone_of).collect(), 1 << 12);
        self.hlines = cap_head(hlines.iter().map(hl_of).collect(), 1 << 12);
        self.segs = cap_head(segs.iter().map(seg_of).collect(), 1 << 12);
        self.markers = cap_head(markers.iter().map(mk_of).collect(), 1 << 12);
    }

    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &RawGpuAccess,
    ) -> anyhow::Result<()> {
        let Some((device, encoder, pixel_format)) = (unsafe { borrow_metal(gpu) }) else {
            anyhow::bail!("chart Metal draw received empty Metal raw gpu handles");
        };
        if self.device_generation != gpu.device_generation()
            || self.pixel_format != Some(pixel_format)
        {
            self.device_generation = gpu.device_generation();
            self.pixel_format = Some(pixel_format);
            self.pipelines = Some(create_pipelines(device, pixel_format));
            self.background_texture = Some(create_background_texture(device));
        }
        self.upload_common(
            device,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            book_style,
        );
        let pipelines = self.pipelines.as_ref().unwrap();
        let bg = self.background_texture.as_ref().unwrap();
        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        encoder.set_scissor_rect(sc);

        set_uniform(encoder, 0, self.bg_uniform.buffer());
        encoder.set_fragment_texture(0, Some(bg.texture.as_ref()));
        encoder.set_fragment_sampler_state(0, Some(pipelines.sampler.as_ref()));
        draw(encoder, &pipelines.background, 6, 1);

        set_uniform(encoder, 0, self.grid_uniform.buffer());
        draw(encoder, &pipelines.grid, 6, 1);

        set_uniform(encoder, 0, self.view_uniform.buffer());
        set_storage(encoder, 1, self.cross_buffer.buffer());
        if !self.crosses.is_empty() {
            draw(encoder, &pipelines.volume, 6, self.crosses.len() as u64);
        }
        if self.last_line.len() > 1 {
            set_storage(encoder, 1, self.last_line_buffer.buffer());
            draw(
                encoder,
                &pipelines.price_last,
                6,
                (self.last_line.len() - 1) as u64,
            );
        }
        if self.mark_line.len() > 1 {
            set_storage(encoder, 1, self.mark_line_buffer.buffer());
            draw(
                encoder,
                &pipelines.price_mark,
                6,
                (self.mark_line.len() - 1) as u64,
            );
        }
        if !self.crosses.is_empty() {
            set_storage(encoder, 1, self.cross_buffer.buffer());
            draw(encoder, &pipelines.crosses, 6, self.crosses.len() as u64);
        }

        set_uniform(encoder, 0, self.book_view_uniform.buffer());
        encoder.set_vertex_buffer(1, Some(self.book_style_uniform.buffer()), 0);
        encoder.set_fragment_buffer(1, Some(self.book_style_uniform.buffer()), 0);
        set_storage(encoder, 2, self.level_buffer.buffer());
        draw(encoder, &pipelines.book_bg, 6, 1);
        if !self.levels.is_empty() {
            draw(encoder, &pipelines.book_bars, 6, self.levels.len() as u64);
        }

        set_uniform(encoder, 0, self.view_uniform.buffer());
        if !self.zones.is_empty() {
            set_storage(encoder, 1, self.zone_buffer.buffer());
            draw(encoder, &pipelines.zone, 6, self.zones.len() as u64);
        }
        if !self.hlines.is_empty() {
            set_storage(encoder, 1, self.hline_buffer.buffer());
            draw(encoder, &pipelines.hline, 6, self.hlines.len() as u64);
        }
        if !self.segs.is_empty() {
            set_storage(encoder, 1, self.seg_buffer.buffer());
            draw(encoder, &pipelines.seg, 6, self.segs.len() as u64);
        }
        if !self.markers.is_empty() {
            set_storage(encoder, 1, self.marker_buffer.buffer());
            draw(encoder, &pipelines.marker, 6, self.markers.len() as u64);
        }
        if cursor_params.enabled > 0.0 {
            crate::diag::bump(&crate::diag::CHART_CURSOR_DRAW);
            set_uniform(encoder, 0, self.cursor_uniform.buffer());
            draw(encoder, &pipelines.cursor, 12, 1);
        }
        Ok(())
    }

    fn upload_common(
        &mut self,
        device: &DeviceRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        book_style: &BookStyle,
    ) {
        let mut view = *view;
        view.volume_buy_inv = 1.0 / self.volume_buy_max.max(1e-6);
        view.volume_sell_inv = 1.0 / self.volume_sell_max.max(1e-6);
        view.volume_alpha = 0.34;
        self.bg_uniform
            .write(device, "moon_chart_bg_uniform", &[*background_params]);
        self.grid_uniform
            .write(device, "moon_chart_grid_uniform", &[*grid_params]);
        self.cursor_uniform
            .write(device, "moon_chart_cursor_uniform", &[*cursor_params]);
        self.view_uniform
            .write(device, "moon_chart_view_uniform", &[view]);
        self.book_view_uniform
            .write(device, "moon_chart_book_view_uniform", &[*orderbook_view]);
        self.book_style_uniform
            .write(device, "moon_chart_book_style_uniform", &[*book_style]);
        self.cross_buffer
            .write(device, "moon_chart_crosses", &self.crosses);
        self.last_line_buffer
            .write(device, "moon_chart_last_line", &self.last_line);
        self.mark_line_buffer
            .write(device, "moon_chart_mark_line", &self.mark_line);
        self.level_buffer
            .write(device, "moon_chart_book_levels", &self.levels);
        self.zone_buffer
            .write(device, "moon_chart_zones", &self.zones);
        self.hline_buffer
            .write(device, "moon_chart_hlines", &self.hlines);
        self.seg_buffer.write(device, "moon_chart_segs", &self.segs);
        self.marker_buffer
            .write(device, "moon_chart_markers", &self.markers);
    }

    fn recalc_volume_scale(&mut self) {
        self.volume_buy_max = 1e-6;
        self.volume_sell_max = 1e-6;
        for c in &self.crosses {
            if c.side == 0 {
                self.volume_buy_max = self.volume_buy_max.max(c.qty);
            } else {
                self.volume_sell_max = self.volume_sell_max.max(c.qty);
            }
        }
    }
}

fn set_uniform(encoder: &RenderCommandEncoderRef, index: u64, buffer: &metal::BufferRef) {
    encoder.set_vertex_buffer(index, Some(buffer), 0);
    encoder.set_fragment_buffer(index, Some(buffer), 0);
}

fn set_storage(encoder: &RenderCommandEncoderRef, index: u64, buffer: &metal::BufferRef) {
    encoder.set_vertex_buffer(index, Some(buffer), 0);
}

fn draw(
    encoder: &RenderCommandEncoderRef,
    pipeline: &RenderPipelineState,
    vertices: u64,
    instances: u64,
) {
    encoder.set_render_pipeline_state(pipeline);
    encoder.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, vertices, instances);
}

unsafe fn borrow_metal<'a>(
    gpu: &RawGpuAccess,
) -> Option<(&'a DeviceRef, &'a RenderCommandEncoderRef, MTLPixelFormat)> {
    let RawGpuAccess::Metal(gpu) = gpu else {
        return None;
    };
    if gpu.device.is_null() || gpu.command_encoder.is_null() || gpu.render_target_format == 0 {
        return None;
    }
    Some((
        unsafe { DeviceRef::from_ptr(gpu.device.cast()) },
        unsafe { RenderCommandEncoderRef::from_ptr(gpu.command_encoder.cast()) },
        unsafe { std::mem::transmute::<u64, MTLPixelFormat>(gpu.render_target_format) },
    ))
}

fn scissor_rect(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> MTLScissorRect {
    let x = view.bounds[0].floor().max(0.0) as u64;
    let y = view.bounds[1].floor().max(0.0) as u64;
    let r = (orderbook_view.bounds[0] + orderbook_view.bounds[2])
        .ceil()
        .clamp(x as f32 + 1.0, width.max(1) as f32) as u64;
    let b = (view.bounds[1] + view.bounds[3])
        .ceil()
        .clamp(y as f32 + 1.0, height.max(1) as f32) as u64;
    MTLScissorRect {
        x,
        y,
        width: (r - x).max(1),
        height: (b - y).max(1),
    }
}

fn create_pipelines(device: &DeviceRef, pixel_format: MTLPixelFormat) -> Pipelines {
    let library = device
        .new_library_with_source(SHADER, &CompileOptions::new())
        .expect("chart Metal shaders must compile");
    let sampler_desc = SamplerDescriptor::new();
    sampler_desc.set_min_filter(MTLSamplerMinMagFilter::Linear);
    sampler_desc.set_mag_filter(MTLSamplerMinMagFilter::Linear);
    let sampler = device.new_sampler(&sampler_desc);
    Pipelines {
        background: pipeline(
            device,
            &library,
            pixel_format,
            "background_vertex",
            "background_fragment",
        ),
        grid: pipeline(
            device,
            &library,
            pixel_format,
            "grid_vertex",
            "grid_fragment",
        ),
        cursor: pipeline(
            device,
            &library,
            pixel_format,
            "cursor_vertex",
            "cursor_fragment",
        ),
        crosses: pipeline(
            device,
            &library,
            pixel_format,
            "crosses_vertex",
            "crosses_fragment",
        ),
        volume: pipeline(
            device,
            &library,
            pixel_format,
            "volume_vertex",
            "volume_fragment",
        ),
        price_last: pipeline(
            device,
            &library,
            pixel_format,
            "price_line_vertex",
            "price_last_fragment",
        ),
        price_mark: pipeline(
            device,
            &library,
            pixel_format,
            "price_line_vertex",
            "price_mark_fragment",
        ),
        book_bg: pipeline(
            device,
            &library,
            pixel_format,
            "book_bg_vertex",
            "book_bg_fragment",
        ),
        book_bars: pipeline(
            device,
            &library,
            pixel_format,
            "book_bars_vertex",
            "book_bars_fragment",
        ),
        zone: pipeline(
            device,
            &library,
            pixel_format,
            "zone_vertex",
            "zone_fragment",
        ),
        hline: pipeline(
            device,
            &library,
            pixel_format,
            "hline_vertex",
            "hline_fragment",
        ),
        seg: pipeline(device, &library, pixel_format, "seg_vertex", "seg_fragment"),
        marker: pipeline(
            device,
            &library,
            pixel_format,
            "marker_vertex",
            "marker_fragment",
        ),
        sampler,
    }
}

fn pipeline(
    device: &DeviceRef,
    library: &metal::Library,
    pixel_format: MTLPixelFormat,
    vertex: &str,
    fragment: &str,
) -> RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex, None)
        .expect("chart vertex function exists");
    let fragment_fn = library
        .get_function(fragment, None)
        .expect("chart fragment function exists");
    let descriptor = RenderPipelineDescriptor::new();
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color = descriptor.color_attachments().object_at(0).unwrap();
    color.set_pixel_format(pixel_format);
    color.set_blending_enabled(true);
    color.set_rgb_blend_operation(MTLBlendOperation::Add);
    color.set_alpha_blend_operation(MTLBlendOperation::Add);
    color.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
    color.set_source_alpha_blend_factor(MTLBlendFactor::One);
    color.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
    color.set_destination_alpha_blend_factor(MTLBlendFactor::One);
    device
        .new_render_pipeline_state(&descriptor)
        .expect("chart render pipeline must build")
}

fn create_background_texture(device: &DeviceRef) -> BackgroundTexture {
    let image = image::load_from_memory(BACKGROUND_PNG)
        .expect("embedded chart background must decode")
        .to_rgba8();
    let desc = TextureDescriptor::new();
    desc.set_texture_type(metal::MTLTextureType::D2);
    desc.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
    desc.set_width(image.width() as u64);
    desc.set_height(image.height() as u64);
    desc.set_depth(1);
    desc.set_mipmap_level_count(1);
    desc.set_array_length(1);
    desc.set_usage(MTLTextureUsage::ShaderRead);
    desc.set_storage_mode(metal::MTLStorageMode::Managed);
    let texture = device.new_texture(&desc);
    let region = metal::MTLRegion {
        origin: metal::MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: image.width() as u64,
            height: image.height() as u64,
            depth: 1,
        },
    };
    texture.replace_region(
        region,
        0,
        image.as_ptr() as *const c_void,
        image.width() as u64 * 4,
    );
    BackgroundTexture { texture }
}

fn cap_tail<T>(mut data: Vec<T>, cap: usize) -> Vec<T> {
    if data.len() > cap {
        data.drain(0..data.len() - cap);
    }
    data
}

fn cap_head<T>(mut data: Vec<T>, cap: usize) -> Vec<T> {
    if data.len() > cap {
        data.truncate(cap);
    }
    data
}
