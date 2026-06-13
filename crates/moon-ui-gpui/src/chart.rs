//! Чарт в GPUI-оболочке (вариант A, доказан Лабой 2b): собственный wgpu-девайс
//! рендерит панели контейнера движком `moon_chart::paint::render_panes` в offscreen-
//! текстуру, та читается в CPU (readback) и отдаётся GPUI как `RenderImage`. Движок
//! переносится 1:1 — здесь только обвязка offscreen+readback вместо egui-wgpu surface.
//!
//! Размер фиксирован 1024×576 (W*4=4096 кратно 256 → readback без паддинга строк).
//! Оси/курсор (egui-overlay в egui-версии) пока не рисуем — следующий шаг (GPUI-текст).

use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

use moon_chart::container::{Container, ContainerKind};
use moon_chart::paint::{now_unix_ms, render_panes};
use moon_chart::view::Rect as ChartRect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::session::{CoreId, SessionManager};

const W: u32 = 1024;
const H: u32 = 576;

pub struct ChartGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    staging: wgpu::Buffer,
    pub container: Container,
    epoch: f64,
    theme: ChartTheme,
    orders: OrdersStyle,
}

impl ChartGpu {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("wgpu adapter");
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default(), None),
        )
        .expect("wgpu device");

        // BGRA sRGB — как surface egui-версии: байты сразу в порядке, который GPUI
        // ждёт от RenderImage (BGRA), иначе каналы R↔B свопаются. + sRGB-таргет.
        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpui-chart-offscreen"),
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpui-chart-staging"),
            size: (W * H * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            device,
            queue,
            format,
            _texture: texture,
            view,
            staging,
            container: Container::new(ContainerKind::Main),
            epoch,
            theme,
            orders: OrdersStyle::default(),
        }
    }

    /// Открыть монету (фулскрин-панель) — 1:1 как egui host.
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container
            .open_manual(core, market, &self.device, self.format, self.epoch);
    }

    pub fn is_empty(&self) -> bool {
        self.container.is_empty()
    }

    /// Рендер панелей движком в offscreen → readback → RenderImage (BGRA для GPUI).
    pub fn render(&mut self, session: &SessionManager) -> Arc<RenderImage> {
        let area = ChartRect { x: 0.0, y: 0.0, w: W as f32, h: H as f32 };
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gpui-chart") });

        // Движок 1:1: рисует панели контейнера в наш offscreen-view.
        let _layout = render_panes(
            &mut self.container,
            &self.device,
            &self.queue,
            &mut enc,
            &self.view,
            area,
            [W as f32, H as f32],
            1.0,
            now_unix_ms(),
            &self.theme,
            &self.orders,
            None,
            None,
            session,
        );

        enc.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self._texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.staging,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));

        let slice = self.staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        let pixels = data.to_vec();
        drop(data);
        self.staging.unmap();

        let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(W, H, pixels).expect("chart buf");
        Arc::new(RenderImage::new(vec![Frame::new(buf)]))
    }
}
