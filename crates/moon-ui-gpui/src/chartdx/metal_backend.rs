//! macOS GPUI native Metal chart backend. It renders inside GPUI's CAMetalLayer
//! command encoder via the custom GPU pass hook.

use foreign_types::ForeignTypeRef;
use gpui::RawGpuAccess;
use metal::{
    CommandBufferRef, CompileOptions, DeviceRef, MTLBlendFactor, MTLBlendOperation, MTLLoadAction,
    MTLPixelFormat, MTLPrimitiveType, MTLResourceOptions, MTLSamplerMinMagFilter, MTLScissorRect,
    MTLSize, MTLStoreAction, MTLTextureUsage, RenderCommandEncoderRef, RenderPipelineDescriptor,
    RenderPipelineState, SamplerDescriptor, TextureDescriptor,
};
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};
use std::ffi::c_void;

use super::types::{
    BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams, GridParams, HLineGpu,
    MarkerGpu, ReadoutGlyph, ReadoutRect, SegGpu, ZoneGpu,
};

const SHADER: &str = include_str!("shaders/chart_native.metal");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");
const MIN_COMBO_CAPACITY: usize = 1;

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
    readout_rect: RenderPipelineState,
    readout_glyph: RenderPipelineState,
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

struct BaseTexture {
    texture: metal::Texture,
    w: u32,
    h: u32,
    generation: u64,
    pixel_format: MTLPixelFormat,
}

#[derive(Default)]
struct BaseCache {
    texture: Option<BaseTexture>,
    blit_uniform: BufferSlot,
    valid: bool,
}

impl BaseCache {
    fn is_valid_for(&self, gpu: &RawGpuAccess, pixel_format: MTLPixelFormat) -> bool {
        let w = gpu.width();
        let h = gpu.height();
        let generation = gpu.device_generation();
        self.valid
            && self.texture.as_ref().is_some_and(|tex| {
                tex.w == w
                    && tex.h == h
                    && tex.generation == generation
                    && tex.pixel_format == pixel_format
            })
    }

    fn needs_rebuild(&self, gpu: &RawGpuAccess, pixel_format: Option<MTLPixelFormat>) -> bool {
        let Some(pixel_format) = pixel_format else {
            return true;
        };
        !self.is_valid_for(gpu, pixel_format)
    }

    fn ensure_texture(
        &mut self,
        device: &DeviceRef,
        gpu: &RawGpuAccess,
        pixel_format: MTLPixelFormat,
    ) -> &metal::TextureRef {
        let w = gpu.width().max(1);
        let h = gpu.height().max(1);
        let generation = gpu.device_generation();
        let recreate = self.texture.as_ref().is_none_or(|tex| {
            tex.w != w
                || tex.h != h
                || tex.generation != generation
                || tex.pixel_format != pixel_format
        });
        if recreate {
            let desc = TextureDescriptor::new();
            desc.set_texture_type(metal::MTLTextureType::D2);
            desc.set_pixel_format(pixel_format);
            desc.set_width(w as u64);
            desc.set_height(h as u64);
            desc.set_depth(1);
            desc.set_mipmap_level_count(1);
            desc.set_array_length(1);
            desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            desc.set_storage_mode(metal::MTLStorageMode::Private);
            let texture = device.new_texture(&desc);
            self.texture = Some(BaseTexture {
                texture,
                w,
                h,
                generation,
                pixel_format,
            });
            self.valid = false;
        }
        self.texture.as_ref().unwrap().texture.as_ref()
    }

    fn write_blit_uniform(
        &mut self,
        device: &DeviceRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        gpu: &RawGpuAccess,
    ) {
        let dst = panel_dst(view, orderbook_view, gpu.width(), gpu.height());
        let w = gpu.width().max(1) as f32;
        let h = gpu.height().max(1) as f32;
        let params = BackgroundParams {
            dst,
            resolution: [w, h],
            uv_off: [dst[0] / w, dst[1] / h],
            uv_scale: [dst[2] / w, dst[3] / h],
            opacity: 1.0,
            _pad: 0.0,
            bg: [0.0, 0.0, 0.0, 1.0],
        };
        self.blit_uniform
            .write(device, "moon_chart_base_blit_uniform", &[params]);
    }
}

pub struct MetalLayers {
    device_generation: u64,
    pixel_format: Option<MTLPixelFormat>,
    pipelines: Option<Pipelines>,
    background_texture: Option<BackgroundTexture>,
    base_cache: BaseCache,
    crosses: Vec<ChartCross>,
    last_line: Vec<PriceLinePoint>,
    mark_line: Vec<PriceLinePoint>,
    combo_capacity: usize,
    price_line_capacity: usize,
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
    readout_rect_buffer: BufferSlot,
    readout_glyph_buffer: BufferSlot,
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
    combo_buffers_dirty: bool,
    price_line_buffers_dirty: bool,
    book_buffer_dirty: bool,
    userdata_buffers_dirty: bool,
}

impl MetalLayers {
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            pixel_format: None,
            pipelines: None,
            background_texture: None,
            base_cache: BaseCache::default(),
            crosses: Vec::new(),
            last_line: Vec::new(),
            mark_line: Vec::new(),
            combo_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
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
            readout_rect_buffer: BufferSlot::default(),
            readout_glyph_buffer: BufferSlot::default(),
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
            combo_buffers_dirty: true,
            price_line_buffers_dirty: true,
            book_buffer_dirty: true,
            userdata_buffers_dirty: true,
        }
    }

    pub fn set_combo_capacity(&mut self, combo_capacity: usize, price_line_capacity: usize) {
        let combo_capacity = sanitize_capacity(combo_capacity);
        let price_line_capacity = sanitize_capacity(price_line_capacity);
        if self.combo_capacity == combo_capacity && self.price_line_capacity == price_line_capacity
        {
            return;
        }
        self.combo_capacity = combo_capacity;
        self.price_line_capacity = price_line_capacity;
        if self.crosses.len() > self.combo_capacity {
            let drop = self.crosses.len() - self.combo_capacity;
            self.crosses.drain(0..drop);
        }
        if self.last_line.len() > self.price_line_capacity {
            let drop = self.last_line.len() - self.price_line_capacity;
            self.last_line.drain(0..drop);
        }
        if self.mark_line.len() > self.price_line_capacity {
            let drop = self.mark_line.len() - self.price_line_capacity;
            self.mark_line.drain(0..drop);
        }
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        self.price_line_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn reset_combo(&mut self, data: Vec<ChartCross>) {
        self.crosses = cap_tail(data, self.combo_capacity);
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn append_combo(&mut self, data: &[ChartCross]) {
        if data.is_empty() {
            return;
        }
        self.crosses.extend_from_slice(data);
        if self.crosses.len() > self.combo_capacity {
            let drop = self.crosses.len() - self.combo_capacity;
            self.crosses.drain(0..drop);
        }
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.last_line = cap_tail(last.to_vec(), self.price_line_capacity);
        self.mark_line = cap_tail(mark.to_vec(), self.price_line_capacity);
        self.price_line_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn set_orderbook(&mut self, levels: Vec<LevelInstance>) {
        self.levels = levels;
        self.book_buffer_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn set_userdata(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.zones = zones.iter().map(zone_of).collect();
        self.hlines = hlines.iter().map(hl_of).collect();
        self.segs = segs.iter().map(seg_of).collect();
        self.markers = markers.iter().map(mk_of).collect();
        self.userdata_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn needs_base_cache(&self, gpu: &RawGpuAccess) -> bool {
        self.base_cache.needs_rebuild(gpu, self.pixel_format)
    }

    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        readout_glyphs: &[ReadoutGlyph],
        orderbook_view: &ChartViewGpu,
        gpu: &RawGpuAccess,
    ) -> anyhow::Result<()> {
        let Some((device, encoder)) = (unsafe { borrow_metal_draw(gpu) }) else {
            anyhow::bail!("chart Metal draw received empty Metal raw gpu handles");
        };
        self.upload_frame_uniforms(
            device,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            readout_rects,
            readout_glyphs,
        );
        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        encoder.set_scissor_rect(sc);

        let pixel_format = self
            .pixel_format
            .expect("Metal pixel format must be prepared");
        if self.base_cache.is_valid_for(gpu, pixel_format) {
            self.draw_cached_base(device, encoder, view, orderbook_view, gpu);
        } else {
            self.draw_base_layers(encoder);
        }
        self.draw_cursor_layer(encoder, cursor_params, readout_rects, readout_glyphs);
        Ok(())
    }

    fn draw_base_layers(&self, encoder: &RenderCommandEncoderRef) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let bg = self.background_texture.as_ref().unwrap();

        crate::diag::bump(&crate::diag::CHART_BG_DRAW);
        set_uniform(encoder, 0, self.bg_uniform.buffer());
        encoder.set_fragment_texture(0, Some(bg.texture.as_ref()));
        encoder.set_fragment_sampler_state(0, Some(pipelines.sampler.as_ref()));
        draw(encoder, &pipelines.background, 6, 1);

        crate::diag::bump(&crate::diag::CHART_GRID_DRAW);
        set_uniform(encoder, 0, self.grid_uniform.buffer());
        draw(encoder, &pipelines.grid, 6, 1);

        set_uniform(encoder, 0, self.view_uniform.buffer());
        set_storage(encoder, 1, self.cross_buffer.buffer());
        if !self.crosses.is_empty() {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw(encoder, &pipelines.volume, 6, self.crosses.len() as u64);
        }
        if self.last_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, self.last_line_buffer.buffer());
            draw(
                encoder,
                &pipelines.price_last,
                6,
                (self.last_line.len() - 1) as u64,
            );
        }
        if self.mark_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, self.mark_line_buffer.buffer());
            draw(
                encoder,
                &pipelines.price_mark,
                6,
                (self.mark_line.len() - 1) as u64,
            );
        }
        if !self.crosses.is_empty() {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, self.cross_buffer.buffer());
            draw(encoder, &pipelines.crosses, 6, self.crosses.len() as u64);
        }

        crate::diag::bump(&crate::diag::CHART_BOOK_DRAW);
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
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.zone_buffer.buffer());
            draw(encoder, &pipelines.zone, 6, self.zones.len() as u64);
        }
        if !self.hlines.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.hline_buffer.buffer());
            draw(encoder, &pipelines.hline, 6, self.hlines.len() as u64);
        }
        if !self.segs.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.seg_buffer.buffer());
            draw(encoder, &pipelines.seg, 6, self.segs.len() as u64);
        }
        if !self.markers.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.marker_buffer.buffer());
            draw(encoder, &pipelines.marker, 6, self.markers.len() as u64);
        }
    }

    fn draw_cursor_layer(
        &self,
        encoder: &RenderCommandEncoderRef,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        readout_glyphs: &[ReadoutGlyph],
    ) {
        let pipelines = self.pipelines.as_ref().unwrap();
        if cursor_params.enabled > 0.0 {
            crate::diag::bump(&crate::diag::CHART_CURSOR_DRAW);
            set_uniform(encoder, 0, self.cursor_uniform.buffer());
            draw(encoder, &pipelines.cursor, 12, 1);
        }
        if !readout_rects.is_empty() {
            set_storage(encoder, 1, self.readout_rect_buffer.buffer());
            draw(
                encoder,
                &pipelines.readout_rect,
                6,
                readout_rects.len() as u64,
            );
        }
        if !readout_glyphs.is_empty() {
            set_storage(encoder, 1, self.readout_glyph_buffer.buffer());
            draw(
                encoder,
                &pipelines.readout_glyph,
                6,
                readout_glyphs.len() as u64,
            );
        }
    }

    fn draw_cached_base(
        &mut self,
        device: &DeviceRef,
        encoder: &RenderCommandEncoderRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        gpu: &RawGpuAccess,
    ) {
        self.base_cache
            .write_blit_uniform(device, view, orderbook_view, gpu);
        let pipelines = self.pipelines.as_ref().unwrap();
        let texture = self.base_cache.texture.as_ref().unwrap().texture.as_ref();
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
        set_uniform(encoder, 0, self.base_cache.blit_uniform.buffer());
        encoder.set_fragment_texture(0, Some(texture));
        encoder.set_fragment_sampler_state(0, Some(pipelines.sampler.as_ref()));
        draw(encoder, &pipelines.background, 6, 1);
    }

    pub fn prepare(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &RawGpuAccess,
        rebuild_base: bool,
    ) -> anyhow::Result<()> {
        let Some((device, command_buffer, pixel_format)) = (unsafe { borrow_metal_prepare(gpu) })
        else {
            anyhow::bail!("chart Metal prepare received empty Metal raw gpu handles");
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
        if rebuild_base || self.base_cache.needs_rebuild(gpu, Some(pixel_format)) {
            self.rebuild_base_cache(
                device,
                command_buffer,
                gpu,
                pixel_format,
                view,
                orderbook_view,
            )?;
        }
        Ok(())
    }

    fn rebuild_base_cache(
        &mut self,
        device: &DeviceRef,
        command_buffer: &CommandBufferRef,
        gpu: &RawGpuAccess,
        pixel_format: MTLPixelFormat,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
    ) -> anyhow::Result<()> {
        let texture = self
            .base_cache
            .ensure_texture(device, gpu, pixel_format)
            .to_owned();
        let pass = metal::RenderPassDescriptor::new();
        let color = pass.color_attachments().object_at(0).unwrap();
        color.set_texture(Some(texture.as_ref()));
        color.set_load_action(MTLLoadAction::Clear);
        color.set_store_action(MTLStoreAction::Store);
        color.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
        let encoder = command_buffer.new_render_command_encoder(pass);
        encoder.set_scissor_rect(scissor_rect(
            view,
            orderbook_view,
            gpu.width(),
            gpu.height(),
        ));
        self.draw_base_layers(encoder);
        encoder.end_encoding();
        self.base_cache.valid = true;
        crate::diag::bump(&crate::diag::CHART_BASE_BAKE);
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
        self.readout_rect_buffer
            .write(device, "moon_chart_readout_rects", &[] as &[ReadoutRect]);
        self.readout_glyph_buffer.write(
            device,
            "moon_chart_readout_glyphs",
            &[] as &[ReadoutGlyph],
        );
        self.view_uniform
            .write(device, "moon_chart_view_uniform", &[view]);
        self.book_view_uniform
            .write(device, "moon_chart_book_view_uniform", &[*orderbook_view]);
        self.book_style_uniform
            .write(device, "moon_chart_book_style_uniform", &[*book_style]);
        if self.combo_buffers_dirty || self.cross_buffer.buffer.is_none() {
            self.cross_buffer
                .write(device, "moon_chart_crosses", &self.crosses);
            self.combo_buffers_dirty = false;
        }
        if self.price_line_buffers_dirty
            || self.last_line_buffer.buffer.is_none()
            || self.mark_line_buffer.buffer.is_none()
        {
            self.last_line_buffer
                .write(device, "moon_chart_last_line", &self.last_line);
            self.mark_line_buffer
                .write(device, "moon_chart_mark_line", &self.mark_line);
            self.price_line_buffers_dirty = false;
        }
        if self.book_buffer_dirty || self.level_buffer.buffer.is_none() {
            self.level_buffer
                .write(device, "moon_chart_book_levels", &self.levels);
            self.book_buffer_dirty = false;
        }
        if self.userdata_buffers_dirty
            || self.zone_buffer.buffer.is_none()
            || self.hline_buffer.buffer.is_none()
            || self.seg_buffer.buffer.is_none()
            || self.marker_buffer.buffer.is_none()
        {
            self.zone_buffer
                .write(device, "moon_chart_zones", &self.zones);
            self.hline_buffer
                .write(device, "moon_chart_hlines", &self.hlines);
            self.seg_buffer.write(device, "moon_chart_segs", &self.segs);
            self.marker_buffer
                .write(device, "moon_chart_markers", &self.markers);
            self.userdata_buffers_dirty = false;
        }
    }

    fn upload_frame_uniforms(
        &mut self,
        device: &DeviceRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        readout_glyphs: &[ReadoutGlyph],
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
        self.readout_rect_buffer
            .write(device, "moon_chart_readout_rects", readout_rects);
        self.readout_glyph_buffer
            .write(device, "moon_chart_readout_glyphs", readout_glyphs);
        self.view_uniform
            .write(device, "moon_chart_view_uniform", &[view]);
        self.book_view_uniform
            .write(device, "moon_chart_book_view_uniform", &[*orderbook_view]);
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

unsafe fn borrow_metal_prepare<'a>(
    gpu: &RawGpuAccess,
) -> Option<(&'a DeviceRef, &'a CommandBufferRef, MTLPixelFormat)> {
    let RawGpuAccess::Metal(gpu) = gpu else {
        return None;
    };
    if gpu.render_target_format == 0 {
        return None;
    }
    // device — NonNull<c_void> (по контракту не null): берём сырой указатель и кастуем
    // к *mut MTLDevice, как dx11-путь делает через `.as_ptr()`.
    Some((
        unsafe { DeviceRef::from_ptr(gpu.device.as_ptr().cast()) },
        unsafe { CommandBufferRef::from_ptr(gpu.command_buffer.as_ptr().cast()) },
        unsafe { std::mem::transmute::<u64, MTLPixelFormat>(gpu.render_target_format) },
    ))
}

unsafe fn borrow_metal_draw<'a>(
    gpu: &RawGpuAccess,
) -> Option<(&'a DeviceRef, &'a RenderCommandEncoderRef)> {
    let RawGpuAccess::Metal(gpu) = gpu else {
        return None;
    };
    // command_encoder — Option<NonNull<c_void>>: None во время prepare (энкодера ещё нет).
    let encoder = gpu.command_encoder?;
    Some((
        unsafe { DeviceRef::from_ptr(gpu.device.as_ptr().cast()) },
        unsafe { RenderCommandEncoderRef::from_ptr(encoder.as_ptr().cast()) },
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

fn panel_dst(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> [f32; 4] {
    let sc = scissor_rect(view, orderbook_view, width, height);
    [sc.x as f32, sc.y as f32, sc.width as f32, sc.height as f32]
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
        readout_rect: pipeline(
            device,
            &library,
            pixel_format,
            "readout_rect_vertex",
            "readout_rect_fragment",
        ),
        readout_glyph: pipeline(
            device,
            &library,
            pixel_format,
            "readout_glyph_vertex",
            "readout_glyph_fragment",
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
        book_bg: opaque_pipeline(
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
    pipeline_with_blend(device, library, pixel_format, vertex, fragment, true)
}

fn opaque_pipeline(
    device: &DeviceRef,
    library: &metal::Library,
    pixel_format: MTLPixelFormat,
    vertex: &str,
    fragment: &str,
) -> RenderPipelineState {
    pipeline_with_blend(device, library, pixel_format, vertex, fragment, false)
}

fn pipeline_with_blend(
    device: &DeviceRef,
    library: &metal::Library,
    pixel_format: MTLPixelFormat,
    vertex: &str,
    fragment: &str,
    alpha_blend: bool,
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
    color.set_blending_enabled(alpha_blend);
    if alpha_blend {
        color.set_rgb_blend_operation(MTLBlendOperation::Add);
        color.set_alpha_blend_operation(MTLBlendOperation::Add);
        color.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
        color.set_source_alpha_blend_factor(MTLBlendFactor::One);
        color.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        color.set_destination_alpha_blend_factor(MTLBlendFactor::One);
    }
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

fn sanitize_capacity(capacity: usize) -> usize {
    capacity.max(MIN_COMBO_CAPACITY)
}
