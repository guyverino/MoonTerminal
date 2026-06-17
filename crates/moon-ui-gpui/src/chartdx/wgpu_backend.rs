//! Linux GPUI native wgpu chart backend. This is an own-pass renderer inside
//! GPUI's existing wgpu frame, not the old moon-chart offscreen/readback path.

use std::num::NonZeroU64;

use gpui::RawGpuAccess;
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};

use super::types::{
    BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams, GridParams, HLineGpu,
    MarkerGpu, SegGpu, ZoneGpu,
};

const BACKGROUND_SHADER: &str = include_str!("shaders/native_background.wgsl");
const GRID_SHADER: &str = include_str!("shaders/native_grid.wgsl");
const CURSOR_SHADER: &str = include_str!("shaders/native_cursor.wgsl");
const CROSSES_SHADER: &str = include_str!("shaders/native_crosses.wgsl");
const PRICE_SHADER: &str = include_str!("shaders/native_price.wgsl");
const BOOK_SHADER: &str = include_str!("shaders/native_book.wgsl");
const ZONE_SHADER: &str = include_str!("shaders/native_zone.wgsl");
const HLINE_SHADER: &str = include_str!("shaders/native_hline.wgsl");
const SEG_SHADER: &str = include_str!("shaders/native_seg.wgsl");
const MARKER_SHADER: &str = include_str!("shaders/native_marker.wgsl");
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
    buffer: Option<wgpu::Buffer>,
    size: u64,
}

impl BufferSlot {
    fn write<T: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        usage: wgpu::BufferUsages,
        data: &[T],
    ) {
        let bytes = bytemuck::cast_slice(data);
        let need = bytes.len().max(4) as u64;
        if self.buffer.as_ref().is_none() || self.size < need {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: need.next_power_of_two(),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.size = need.next_power_of_two();
        }
        if !bytes.is_empty() {
            queue.write_buffer(self.buffer.as_ref().unwrap(), 0, bytes);
        }
    }

    fn binding(&self) -> wgpu::BindingResource<'_> {
        self.buffer.as_ref().unwrap().as_entire_binding()
    }
}

struct Pipelines {
    bg_layout: wgpu::BindGroupLayout,
    grid_layout: wgpu::BindGroupLayout,
    cursor_layout: wgpu::BindGroupLayout,
    view_storage_layout: wgpu::BindGroupLayout,
    book_layout: wgpu::BindGroupLayout,
    background: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    cursor: wgpu::RenderPipeline,
    crosses: wgpu::RenderPipeline,
    volume: wgpu::RenderPipeline,
    price_last: wgpu::RenderPipeline,
    price_mark: wgpu::RenderPipeline,
    book_bg: wgpu::RenderPipeline,
    book_bars: wgpu::RenderPipeline,
    zone: wgpu::RenderPipeline,
    hline: wgpu::RenderPipeline,
    seg: wgpu::RenderPipeline,
    marker: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

struct BackgroundTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct PreparedBindGroups {
    bg: wgpu::BindGroup,
    grid: wgpu::BindGroup,
    cursor: wgpu::BindGroup,
    cross: wgpu::BindGroup,
    last: wgpu::BindGroup,
    mark: wgpu::BindGroup,
    book: wgpu::BindGroup,
    zone: wgpu::BindGroup,
    hline: wgpu::BindGroup,
    seg: wgpu::BindGroup,
    marker: wgpu::BindGroup,
}

pub struct WgpuLayers {
    device_generation: u64,
    format: Option<wgpu::TextureFormat>,
    pipelines: Option<Pipelines>,
    background_texture: Option<BackgroundTexture>,
    prepared_binds: Option<PreparedBindGroups>,
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

impl WgpuLayers {
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            format: None,
            pipelines: None,
            background_texture: None,
            prepared_binds: None,
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
        gpu: &RawGpuAccess,
    ) -> anyhow::Result<()> {
        let Some((device, queue, pass)) = (unsafe { borrow_wgpu_draw(gpu) }) else {
            anyhow::bail!("chart wgpu draw received empty wgpu raw gpu handles");
        };
        self.upload_frame_uniforms(
            device,
            queue,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
        );
        self.prepare_bind_groups(device);
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();

        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        pass.set_scissor_rect(sc.0, sc.1, sc.2, sc.3);
        draw_pipeline(pass, &pipelines.background, &binds.bg, 6, 1);
        draw_pipeline(pass, &pipelines.grid, &binds.grid, 6, 1);
        if !self.crosses.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.volume,
                &binds.cross,
                6,
                self.crosses.len() as u32,
            );
        }
        if self.last_line.len() > 1 {
            draw_pipeline(
                pass,
                &pipelines.price_last,
                &binds.last,
                6,
                (self.last_line.len() - 1) as u32,
            );
        }
        if self.mark_line.len() > 1 {
            draw_pipeline(
                pass,
                &pipelines.price_mark,
                &binds.mark,
                6,
                (self.mark_line.len() - 1) as u32,
            );
        }
        if !self.crosses.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.crosses,
                &binds.cross,
                6,
                self.crosses.len() as u32,
            );
        }
        draw_pipeline(pass, &pipelines.book_bg, &binds.book, 6, 1);
        if !self.levels.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.book_bars,
                &binds.book,
                6,
                self.levels.len() as u32,
            );
        }
        if !self.zones.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.zone,
                &binds.zone,
                6,
                self.zones.len() as u32,
            );
        }
        if !self.hlines.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.hline,
                &binds.hline,
                6,
                self.hlines.len() as u32,
            );
        }
        if !self.segs.is_empty() {
            draw_pipeline(pass, &pipelines.seg, &binds.seg, 6, self.segs.len() as u32);
        }
        if !self.markers.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.marker,
                &binds.marker,
                6,
                self.markers.len() as u32,
            );
        }
        if cursor_params.enabled > 0.0 {
            crate::diag::bump(&crate::diag::CHART_CURSOR_DRAW);
            draw_pipeline(pass, &pipelines.cursor, &binds.cursor, 12, 1);
        }
        Ok(())
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
    ) -> anyhow::Result<()> {
        let Some((device, queue, format)) = (unsafe { borrow_wgpu_prepare(gpu) }) else {
            anyhow::bail!("chart wgpu prepare received empty wgpu raw gpu handles");
        };
        if self.device_generation != gpu.device_generation() || self.format != Some(format) {
            self.device_generation = gpu.device_generation();
            self.format = Some(format);
            self.pipelines = Some(create_pipelines(device, format));
            self.background_texture = Some(create_background_texture(device, queue));
            self.prepared_binds = None;
        }
        self.upload_common(
            device,
            queue,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            book_style,
        );
        self.prepare_bind_groups(device);
        Ok(())
    }

    fn upload_common(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
        self.bg_uniform.write(
            device,
            queue,
            "moon_chart_bg_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*background_params],
        );
        self.grid_uniform.write(
            device,
            queue,
            "moon_chart_grid_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*grid_params],
        );
        self.cursor_uniform.write(
            device,
            queue,
            "moon_chart_cursor_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*cursor_params],
        );
        self.view_uniform.write(
            device,
            queue,
            "moon_chart_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[view],
        );
        self.book_view_uniform.write(
            device,
            queue,
            "moon_chart_book_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*orderbook_view],
        );
        self.book_style_uniform.write(
            device,
            queue,
            "moon_chart_book_style_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*book_style],
        );
        self.cross_buffer.write(
            device,
            queue,
            "moon_chart_crosses",
            wgpu::BufferUsages::STORAGE,
            &self.crosses,
        );
        self.last_line_buffer.write(
            device,
            queue,
            "moon_chart_last_line",
            wgpu::BufferUsages::STORAGE,
            &self.last_line,
        );
        self.mark_line_buffer.write(
            device,
            queue,
            "moon_chart_mark_line",
            wgpu::BufferUsages::STORAGE,
            &self.mark_line,
        );
        self.level_buffer.write(
            device,
            queue,
            "moon_chart_book_levels",
            wgpu::BufferUsages::STORAGE,
            &self.levels,
        );
        self.zone_buffer.write(
            device,
            queue,
            "moon_chart_zones",
            wgpu::BufferUsages::STORAGE,
            &self.zones,
        );
        self.hline_buffer.write(
            device,
            queue,
            "moon_chart_hlines",
            wgpu::BufferUsages::STORAGE,
            &self.hlines,
        );
        self.seg_buffer.write(
            device,
            queue,
            "moon_chart_segs",
            wgpu::BufferUsages::STORAGE,
            &self.segs,
        );
        self.marker_buffer.write(
            device,
            queue,
            "moon_chart_markers",
            wgpu::BufferUsages::STORAGE,
            &self.markers,
        );
    }

    fn upload_frame_uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
    ) {
        let mut view = *view;
        view.volume_buy_inv = 1.0 / self.volume_buy_max.max(1e-6);
        view.volume_sell_inv = 1.0 / self.volume_sell_max.max(1e-6);
        view.volume_alpha = 0.34;
        self.bg_uniform.write(
            device,
            queue,
            "moon_chart_bg_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*background_params],
        );
        self.grid_uniform.write(
            device,
            queue,
            "moon_chart_grid_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*grid_params],
        );
        self.cursor_uniform.write(
            device,
            queue,
            "moon_chart_cursor_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*cursor_params],
        );
        self.view_uniform.write(
            device,
            queue,
            "moon_chart_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[view],
        );
        self.book_view_uniform.write(
            device,
            queue,
            "moon_chart_book_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*orderbook_view],
        );
    }

    fn bind_uniform<'a>(
        &'a self,
        device: &wgpu::Device,
        layout: &'a wgpu::BindGroupLayout,
        uniform: &'a BufferSlot,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_uniform_bind"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.binding(),
            }],
        })
    }

    fn bind_view_storage<'a>(
        &'a self,
        device: &wgpu::Device,
        layout: &'a wgpu::BindGroupLayout,
        storage: &'a BufferSlot,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_view_storage_bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.view_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: storage.binding(),
                },
            ],
        })
    }

    fn prepare_bind_groups(&mut self, device: &wgpu::Device) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let bg = self.background_texture.as_ref().unwrap();
        let bg_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_bg_bind"),
            layout: &pipelines.bg_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.bg_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&bg.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&pipelines.sampler),
                },
            ],
        });
        let grid_bind = self.bind_uniform(device, &pipelines.grid_layout, &self.grid_uniform);
        let cursor_bind = self.bind_uniform(device, &pipelines.cursor_layout, &self.cursor_uniform);
        let cross_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.cross_buffer);
        let last_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.last_line_buffer,
        );
        let mark_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.mark_line_buffer,
        );
        let book_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_book_bind"),
            layout: &pipelines.book_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.book_view_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.book_style_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.level_buffer.binding(),
                },
            ],
        });
        let zone_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.zone_buffer);
        let hline_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.hline_buffer);
        let seg_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.seg_buffer);
        let marker_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.marker_buffer);
        self.prepared_binds = Some(PreparedBindGroups {
            bg: bg_bind,
            grid: grid_bind,
            cursor: cursor_bind,
            cross: cross_bind,
            last: last_bind,
            mark: mark_bind,
            book: book_bind,
            zone: zone_bind,
            hline: hline_bind,
            seg: seg_bind,
            marker: marker_bind,
        });
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

fn draw_pipeline(
    pass: &mut wgpu::RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    vertices: u32,
    instances: u32,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..vertices, 0..instances);
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

unsafe fn borrow_wgpu_prepare<'a>(
    gpu: &RawGpuAccess,
) -> Option<(&'a wgpu::Device, &'a wgpu::Queue, wgpu::TextureFormat)> {
    let RawGpuAccess::Wgpu(gpu) = gpu else {
        return None;
    };
    // Все поля — NonNull<c_void> (по контракту не null): берём сырой указатель `.as_ptr()`.
    Some((
        unsafe { &*(gpu.device.as_ptr() as *const wgpu::Device) },
        unsafe { &*(gpu.queue.as_ptr() as *const wgpu::Queue) },
        unsafe { *(gpu.render_target_format.as_ptr() as *const wgpu::TextureFormat) },
    ))
}

unsafe fn borrow_wgpu_draw<'a>(
    gpu: &RawGpuAccess,
) -> Option<(
    &'a wgpu::Device,
    &'a wgpu::Queue,
    &'a mut wgpu::RenderPass<'a>,
)> {
    let RawGpuAccess::Wgpu(gpu) = gpu else {
        return None;
    };
    // render_pass — Option<NonNull<c_void>>: None во время prepare (пасса ещё нет).
    let render_pass = gpu.render_pass?;
    Some((
        unsafe { &*(gpu.device.as_ptr() as *const wgpu::Device) },
        unsafe { &*(gpu.queue.as_ptr() as *const wgpu::Queue) },
        unsafe { &mut *(render_pass.as_ptr() as *mut wgpu::RenderPass<'a>) },
    ))
}

fn scissor_rect(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> (u32, u32, u32, u32) {
    let l = view.bounds[0].floor().max(0.0) as u32;
    let t = view.bounds[1].floor().max(0.0) as u32;
    let r = (orderbook_view.bounds[0] + orderbook_view.bounds[2])
        .ceil()
        .clamp(l as f32 + 1.0, width.max(1) as f32) as u32;
    let b = (view.bounds[1] + view.bounds[3])
        .ceil()
        .clamp(t as f32 + 1.0, height.max(1) as f32) as u32;
    (l, t, (r - l).max(1), (b - t).max(1))
}

fn create_pipelines(device: &wgpu::Device, format: wgpu::TextureFormat) -> Pipelines {
    let background_shader = shader(device, "moon_chart_background_wgsl", BACKGROUND_SHADER);
    let grid_shader = shader(device, "moon_chart_grid_wgsl", GRID_SHADER);
    let cursor_shader = shader(device, "moon_chart_cursor_wgsl", CURSOR_SHADER);
    let crosses_shader = shader(device, "moon_chart_crosses_wgsl", CROSSES_SHADER);
    let price_shader = shader(device, "moon_chart_price_wgsl", PRICE_SHADER);
    let book_shader = shader(device, "moon_chart_book_wgsl", BOOK_SHADER);
    let zone_shader = shader(device, "moon_chart_zone_wgsl", ZONE_SHADER);
    let hline_shader = shader(device, "moon_chart_hline_wgsl", HLINE_SHADER);
    let seg_shader = shader(device, "moon_chart_seg_wgsl", SEG_SHADER);
    let marker_shader = shader(device, "moon_chart_marker_wgsl", MARKER_SHADER);
    let bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_bg_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<BackgroundParams>()),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let grid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_grid_layout"),
        entries: &[uniform_entry(0, std::mem::size_of::<GridParams>())],
    });
    let cursor_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_cursor_layout"),
        entries: &[uniform_entry(0, std::mem::size_of::<CursorParams>())],
    });
    let view_storage_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_view_storage_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<ChartViewGpu>()),
            storage_entry(1),
        ],
    });
    let book_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_book_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<ChartViewGpu>()),
            uniform_entry(1, std::mem::size_of::<BookStyle>()),
            storage_entry(2),
        ],
    });
    let background = pipeline(
        device,
        format,
        &background_shader,
        &bg_layout,
        "background_vertex",
        "background_fragment",
    );
    let grid = pipeline(
        device,
        format,
        &grid_shader,
        &grid_layout,
        "grid_vertex",
        "grid_fragment",
    );
    let cursor = pipeline(
        device,
        format,
        &cursor_shader,
        &cursor_layout,
        "cursor_vertex",
        "cursor_fragment",
    );
    let crosses = pipeline(
        device,
        format,
        &crosses_shader,
        &view_storage_layout,
        "crosses_vertex",
        "crosses_fragment",
    );
    let volume = pipeline(
        device,
        format,
        &crosses_shader,
        &view_storage_layout,
        "volume_vertex",
        "volume_fragment",
    );
    let price_last = pipeline(
        device,
        format,
        &price_shader,
        &view_storage_layout,
        "price_line_vertex",
        "price_last_fragment",
    );
    let price_mark = pipeline(
        device,
        format,
        &price_shader,
        &view_storage_layout,
        "price_line_vertex",
        "price_mark_fragment",
    );
    let book_bg = pipeline(
        device,
        format,
        &book_shader,
        &book_layout,
        "book_bg_vertex",
        "book_bg_fragment",
    );
    let book_bars = pipeline(
        device,
        format,
        &book_shader,
        &book_layout,
        "book_bars_vertex",
        "book_bars_fragment",
    );
    let zone = pipeline(
        device,
        format,
        &zone_shader,
        &view_storage_layout,
        "zone_vertex",
        "zone_fragment",
    );
    let hline = pipeline(
        device,
        format,
        &hline_shader,
        &view_storage_layout,
        "hline_vertex",
        "hline_fragment",
    );
    let seg = pipeline(
        device,
        format,
        &seg_shader,
        &view_storage_layout,
        "seg_vertex",
        "seg_fragment",
    );
    let marker = pipeline(
        device,
        format,
        &marker_shader,
        &view_storage_layout,
        "marker_vertex",
        "marker_fragment",
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("moon_chart_bg_sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    Pipelines {
        bg_layout,
        grid_layout,
        cursor_layout,
        view_storage_layout,
        book_layout,
        background,
        grid,
        cursor,
        crosses,
        volume,
        price_last,
        price_mark,
        book_bg,
        book_bars,
        zone,
        hline,
        seg,
        marker,
        sampler,
    }
}

fn shader(device: &wgpu::Device, label: &'static str, source: &'static str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn uniform_entry(binding: u32, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("moon_chart_pipeline_layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("moon_chart_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_background_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> BackgroundTexture {
    let image = image::load_from_memory(BACKGROUND_PNG)
        .expect("embedded chart background must decode")
        .to_rgba8();
    let size = wgpu::Extent3d {
        width: image.width().max(1),
        height: image.height().max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("moon_chart_background"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width() * 4),
            rows_per_image: None,
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    BackgroundTexture {
        _texture: texture,
        view,
    }
}
