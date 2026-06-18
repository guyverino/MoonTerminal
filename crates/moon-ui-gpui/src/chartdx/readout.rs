//! Native cursor readout overlay for the Windows DX11 chart backend.
//!
//! This intentionally renders only the tiny price/time readouts that follow the cursor.
//! Full axis typography remains GPUI text because it changes slowly.

use std::ffi::c_void;

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_srv, create_structured, d3d_device_ptr, full_viewport, make_ps,
    make_vs, update_dynamic,
};
use super::types::{ReadoutGlyph, ReadoutRect};

const HLSL: &str = include_str!("shaders/readout.hlsl");
const INITIAL_RECT_CAP: u32 = 4;
const INITIAL_GLYPH_CAP: u32 = 64;

struct ReadoutPipe {
    rect_vs: ID3D11VertexShader,
    rect_ps: ID3D11PixelShader,
    glyph_vs: ID3D11VertexShader,
    glyph_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    rect_buf: ID3D11Buffer,
    rect_srv: ID3D11ShaderResourceView,
    rect_cap: u32,
    glyph_buf: ID3D11Buffer,
    glyph_srv: ID3D11ShaderResourceView,
    glyph_cap: u32,
}

pub struct ReadoutLayer {
    pipe: Option<ReadoutPipe>,
    device_ptr: *mut c_void,
}

impl ReadoutLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            device_ptr: std::ptr::null_mut(),
        }
    }

    pub fn render(
        &mut self,
        rects: &[ReadoutRect],
        glyphs: &[ReadoutGlyph],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if rects.is_empty() && glyphs.is_empty() {
            return;
        }
        let device_ptr = d3d_device_ptr(gpu);
        if self.device_ptr != device_ptr {
            self.pipe = None;
            self.device_ptr = device_ptr;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device, INITIAL_RECT_CAP, INITIAL_GLYPH_CAP));
        }
        let rect_cap = next_buffer_cap(rects.len(), INITIAL_RECT_CAP);
        let glyph_cap = next_buffer_cap(glyphs.len(), INITIAL_GLYPH_CAP);
        if self
            .pipe
            .as_ref()
            .is_none_or(|p| p.rect_cap < rect_cap || p.glyph_cap < glyph_cap)
        {
            self.pipe = Some(Self::create_pipe(device, rect_cap, glyph_cap));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if !rects.is_empty() {
            update_dynamic(context, &pipe.rect_buf, rects);
        }
        if !glyphs.is_empty() {
            update_dynamic(context, &pipe.glyph_buf, glyphs);
        }

        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            if !rects.is_empty() {
                context.VSSetShaderResources(1, Some(&[Some(pipe.rect_srv.clone())]));
                context.VSSetShader(&pipe.rect_vs, None);
                context.PSSetShader(&pipe.rect_ps, None);
                context.DrawInstanced(6, rects.len() as u32, 0, 0);
            }
            if !glyphs.is_empty() {
                context.VSSetShaderResources(2, Some(&[Some(pipe.glyph_srv.clone())]));
                context.VSSetShader(&pipe.glyph_vs, None);
                context.PSSetShader(&pipe.glyph_ps, None);
                context.DrawInstanced(6, glyphs.len() as u32, 0, 0);
            }
        }
    }

    fn create_pipe(device: &ID3D11Device, rect_cap: u32, glyph_cap: u32) -> ReadoutPipe {
        let rect_buf =
            create_structured(device, std::mem::size_of::<ReadoutRect>() as u32, rect_cap);
        let glyph_buf =
            create_structured(device, std::mem::size_of::<ReadoutGlyph>() as u32, glyph_cap);
        ReadoutPipe {
            rect_vs: make_vs(device, HLSL, "readout_rect_vertex"),
            rect_ps: make_ps(device, HLSL, "readout_rect_fragment"),
            glyph_vs: make_vs(device, HLSL, "readout_glyph_vertex"),
            glyph_ps: make_ps(device, HLSL, "readout_glyph_fragment"),
            blend: create_alpha_blend(device),
            rect_srv: create_srv(device, &rect_buf),
            glyph_srv: create_srv(device, &glyph_buf),
            rect_cap,
            glyph_cap,
            rect_buf,
            glyph_buf,
        }
    }
}

fn next_buffer_cap(len: usize, floor: u32) -> u32 {
    let need = (len as u32).max(1);
    floor.max(need).next_power_of_two()
}
