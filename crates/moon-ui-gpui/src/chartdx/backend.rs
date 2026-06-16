//! Platform GPU layer bundle. The chart orchestrator owns one `PlatformLayers`
//! per pane and feeds it backend-neutral data; this module maps that data to
//! the current native GPUI backend.

use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};

use super::types::{BackgroundParams, BookStyle, ChartCross, ChartViewGpu, GridParams};

#[cfg(target_os = "macos")]
use super::metal_backend::MetalLayers;
#[cfg(target_os = "linux")]
use super::wgpu_backend::WgpuLayers;

#[cfg(windows)]
use super::{
    background::{BACKGROUND_3DLOGO_PNG, BackgroundLayer},
    combo::ComboLayer,
    grid::GridLayer,
    orderbook::OrderBookLayer,
    userdata::UserDataLayer,
};

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
};

pub struct PlatformLayers {
    #[cfg(windows)]
    background: BackgroundLayer,
    #[cfg(windows)]
    combo: ComboLayer,
    #[cfg(windows)]
    grid: GridLayer,
    #[cfg(windows)]
    orderbook: OrderBookLayer,
    #[cfg(windows)]
    userdata: UserDataLayer,
    #[cfg(target_os = "linux")]
    wgpu: WgpuLayers,
    #[cfg(target_os = "macos")]
    metal: MetalLayers,
}

impl PlatformLayers {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            background: BackgroundLayer::new(BACKGROUND_3DLOGO_PNG),
            #[cfg(windows)]
            combo: ComboLayer::new(),
            #[cfg(windows)]
            grid: GridLayer::new(),
            #[cfg(windows)]
            orderbook: OrderBookLayer::new(),
            #[cfg(windows)]
            userdata: UserDataLayer::new(),
            #[cfg(target_os = "linux")]
            wgpu: WgpuLayers::new(),
            #[cfg(target_os = "macos")]
            metal: MetalLayers::new(),
        }
    }

    pub fn device_gen(&self) -> u64 {
        #[cfg(windows)]
        {
            return self.combo.device_gen();
        }
        #[allow(unreachable_code)]
        0
    }

    pub fn reset_combo(&mut self, data: Vec<ChartCross>) {
        #[cfg(windows)]
        self.combo.reset(data);
        #[cfg(target_os = "linux")]
        self.wgpu.reset_combo(data);
        #[cfg(target_os = "macos")]
        self.metal.reset_combo(data);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = data;
        }
    }

    pub fn append_combo(&mut self, data: &[ChartCross]) {
        #[cfg(windows)]
        self.combo.append(data);
        #[cfg(target_os = "linux")]
        self.wgpu.append_combo(data);
        #[cfg(target_os = "macos")]
        self.metal.append_combo(data);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = data;
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        #[cfg(windows)]
        self.combo.set_price_lines(last, mark);
        #[cfg(target_os = "linux")]
        self.wgpu.set_price_lines(last, mark);
        #[cfg(target_os = "macos")]
        self.metal.set_price_lines(last, mark);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (last, mark);
        }
    }

    pub fn set_orderbook(&mut self, levels: Vec<LevelInstance>) {
        #[cfg(windows)]
        self.orderbook.set(levels);
        #[cfg(target_os = "linux")]
        self.wgpu.set_orderbook(levels);
        #[cfg(target_os = "macos")]
        self.metal.set_orderbook(levels);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = levels;
        }
    }

    pub fn set_userdata(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        #[cfg(windows)]
        self.userdata.set(zones, hlines, segs, markers);
        #[cfg(target_os = "linux")]
        self.wgpu.set_userdata(zones, hlines, segs, markers);
        #[cfg(target_os = "macos")]
        self.metal.set_userdata(zones, hlines, segs, markers);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (zones, hlines, segs, markers);
        }
    }

    #[cfg(windows)]
    pub fn render_d3d(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &gpui::RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        // Послойные DRAW-счётчики (чокпоинт, мандат AGENTS.md): раз на present на каждый слой.
        crate::diag::bump(&crate::diag::CHART_BG_DRAW);
        self.background
            .render(background_params, device, context, rtv, gpu);
        crate::diag::bump(&crate::diag::CHART_GRID_DRAW);
        self.grid.render(grid_params, device, context, rtv, gpu);
        crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
        self.combo
            .render(view, device, context, rtv, gpu, panel_clip);
        crate::diag::bump(&crate::diag::CHART_BOOK_DRAW);
        self.orderbook.render(
            orderbook_view,
            book_style,
            device,
            context,
            rtv,
            gpu,
            panel_clip,
        );
        crate::diag::bump(&crate::diag::CHART_USER_DRAW);
        self.userdata.render(view, device, context, rtv, gpu);
    }

    #[cfg(target_os = "linux")]
    pub fn render_wgpu(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &gpui::RawGpuAccess,
    ) -> anyhow::Result<()> {
        self.wgpu.render(
            view,
            background_params,
            grid_params,
            orderbook_view,
            book_style,
            gpu,
        )
    }

    #[cfg(target_os = "macos")]
    pub fn render_metal(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &gpui::RawGpuAccess,
    ) -> anyhow::Result<()> {
        self.metal.render(
            view,
            background_params,
            grid_params,
            orderbook_view,
            book_style,
            gpu,
        )
    }
}
