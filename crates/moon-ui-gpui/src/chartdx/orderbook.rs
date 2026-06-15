//! Слой стакана (OrderBook): СВОЯ зона справа (не временной ряд, без combo). Фон зоны +
//! кумулятивные бары глубины + линии уровней. Инстансы (`LevelInstance`) считаются на CPU
//! (`book.build_instances`, нормировка по видимому окну) и заливаются целиком при изменении
//! книги/окна. Порт moon-chart glass-слоя на DX11.

use std::ffi::c_void;

use gpui::RawGpuAccess;
use moon_core::data::LevelInstance;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    ChartViewGpu, create_alpha_blend, create_dynamic_cb, create_srv, create_structured,
    full_viewport, make_ps, make_vs, update_dynamic,
};
pub use super::types::BookStyle;

const BARS_HLSL: &str = include_str!("shaders/bars.hlsl");
const CAP: u32 = 1 << 12; // уровней стакана (с запасом; реально сотни)

struct BookPipe {
    bars_vs: ID3D11VertexShader,
    bars_ps: ID3D11PixelShader,
    bg_vs: ID3D11VertexShader,
    bg_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
    style_cb: ID3D11Buffer,
}

pub struct OrderBookLayer {
    pipe: Option<BookPipe>,
    count: u32,
    pending: Option<Vec<LevelInstance>>,
    device_ptr: *mut c_void,
}

impl OrderBookLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            count: 0,
            pending: None,
            device_ptr: std::ptr::null_mut(),
        }
    }

    /// Залить уровни стакана (целиком). Зовётся при изменении книги/окна.
    pub fn set(&mut self, levels: Vec<LevelInstance>) {
        self.pending = Some(levels);
    }

    /// Рисует фон зоны + бары. `view` — трансформ ЗОНЫ СТАКАНА (viewport = glass_area),
    /// resolution ставит вызывающий. `style` — цвета (тема).
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        style: &BookStyle,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if view.bounds[2] <= 0.0 || view.bounds[3] <= 0.0 {
            return;
        }
        // device-lost: пересоздать pipe; count=0 — буфер пересоздаётся пустым (prepare зальёт
        // уровни заново этим же кадром через set()/pending, инвариант: новый device = 0 валидных).
        if self.device_ptr != gpu.device {
            self.pipe = None;
            self.count = 0;
            self.device_ptr = gpu.device;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(levels) = self.pending.take() {
            let data: &[LevelInstance] = if levels.len() as u32 > CAP {
                &levels[..CAP as usize]
            } else {
                &levels
            };
            if !data.is_empty() {
                update_dynamic(context, &pipe.buffer, data);
            }
            self.count = data.len() as u32;
        }
        update_dynamic(context, &pipe.view_cb, &[*view]);
        update_dynamic(context, &pipe.style_cb, &[*style]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.VSSetConstantBuffers(1, Some(&[Some(pipe.style_cb.clone())]));
            context.PSSetConstantBuffers(1, Some(&[Some(pipe.style_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            // Фон зоны (всегда, даже при пустой книге).
            context.VSSetShader(&pipe.bg_vs, None);
            context.PSSetShader(&pipe.bg_ps, None);
            context.Draw(6, 0);
            // Бары/линии уровней.
            if self.count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                context.VSSetShader(&pipe.bars_vs, None);
                context.PSSetShader(&pipe.bars_ps, None);
                context.DrawInstanced(6, self.count, 0, 0);
            }
        }
    }

    fn create_pipe(device: &ID3D11Device) -> BookPipe {
        let bars_vs = make_vs(device, BARS_HLSL, "bars_vertex");
        let bars_ps = make_ps(device, BARS_HLSL, "bars_fragment");
        let bg_vs = make_vs(device, BARS_HLSL, "bg_vertex");
        let bg_ps = make_ps(device, BARS_HLSL, "bg_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(device, std::mem::size_of::<LevelInstance>() as u32, CAP);
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<BookStyle>() as u32);
        BookPipe {
            bars_vs,
            bars_ps,
            bg_vs,
            bg_ps,
            blend,
            buffer,
            srv,
            view_cb,
            style_cb,
        }
    }
}
