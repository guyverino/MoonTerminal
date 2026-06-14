//! Крестик-курсор own-pass (временно, на период OverScene — GPUI-крестик перекрыт чартом
//! в плоте). Две тонкие линии на позиции курсора. Подписи времени/цены — в желобах GPUI.
//! При переходе на UnderScene (свои компоненты) крестик вернётся в GPUI, слой убрать.

use std::ffi::c_void;

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{create_alpha_blend, create_dynamic_cb, full_viewport, make_ps, make_vs, update_dynamic};

const HLSL: &str = include_str!("shaders/cursor.hlsl");

/// cbuffer `CursorParams` (cursor.hlsl). 64 байта.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CursorParams {
    pub bounds: [f32; 4],     // x, y, w(плот+стакан), h — область крестика (px окна)
    pub resolution: [f32; 2], // backbuffer
    pub cursor: [f32; 2],     // позиция курсора (px окна)
    pub color: [f32; 4],      // rgb sRGB + alpha
    pub thickness: f32,
    pub pad: [f32; 3],
}

struct CurPipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    cb: ID3D11Buffer,
}

pub struct CursorLayer {
    pipe: Option<CurPipe>,
    device_ptr: *mut c_void,
}

impl CursorLayer {
    pub fn new() -> Self {
        Self { pipe: None, device_ptr: std::ptr::null_mut() }
    }

    /// Рисует крестик (2 линии = 12 вершин). `params.resolution` ставит вызывающий.
    pub fn render(
        &mut self,
        params: &CursorParams,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if params.bounds[2] <= 0.0 || params.bounds[3] <= 0.0 {
            return;
        }
        if self.device_ptr != gpu.device {
            self.pipe = None;
            self.device_ptr = gpu.device;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        update_dynamic(context, &pipe.cb, &[*params]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.Draw(12, 0); // 2 линии × 6 вершин
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CurPipe {
        CurPipe {
            vs: make_vs(device, HLSL, "cursor_vertex"),
            ps: make_ps(device, HLSL, "cursor_fragment"),
            blend: create_alpha_blend(device),
            cb: create_dynamic_cb(device, std::mem::size_of::<CursorParams>() as u32),
        }
    }
}
