//! Слой UserData (ордера юзера): МУТИРУЕТ задним числом (юзер двигает ордер → линия едет),
//! поэтому НЕ в combo — отдельный слой, перерисовка по событию. Три геометрии (порт
//! moon-chart order_lines): горизонтали (вход/стоп/liq), отрезки (лестница), маркеры
//! (крест начала/конца, узелки). Геометрию строит `moon_chart::build_order_geometry`
//! (логические time_rel/price), мы конвертим в 16-байт-выровненные GPU-структы и рисуем
//! own-pass тем же chart-трансформом (view = chart_area, линии тянутся в зону стакана).

use std::ffi::c_void;

use gpui::RawGpuAccess;
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    ChartViewGpu, create_alpha_blend, create_dynamic_cb, create_srv, create_structured,
    d3d_device_ptr, full_viewport, make_ps, make_vs, update_dynamic,
};
use super::types::{HLineGpu, MarkerGpu, SegGpu, ZoneGpu};

const HLSL: &str = include_str!("shaders/order_lines.hlsl");
const CAP: u32 = 1 << 12; // ордерных примитивов с запасом (реально десятки)

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

struct UdPipe {
    zone_vs: ID3D11VertexShader,
    zone_ps: ID3D11PixelShader,
    hl_vs: ID3D11VertexShader,
    hl_ps: ID3D11PixelShader,
    seg_vs: ID3D11VertexShader,
    seg_ps: ID3D11PixelShader,
    mk_vs: ID3D11VertexShader,
    mk_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    view_cb: ID3D11Buffer,
    zone_buf: ID3D11Buffer,
    zone_srv: ID3D11ShaderResourceView,
    hl_buf: ID3D11Buffer,
    hl_srv: ID3D11ShaderResourceView,
    seg_buf: ID3D11Buffer,
    seg_srv: ID3D11ShaderResourceView,
    mk_buf: ID3D11Buffer,
    mk_srv: ID3D11ShaderResourceView,
}

#[derive(Default)]
struct Pending {
    zone: Vec<ZoneGpu>,
    hl: Vec<HLineGpu>,
    seg: Vec<SegGpu>,
    mk: Vec<MarkerGpu>,
}

pub struct UserDataLayer {
    pipe: Option<UdPipe>,
    zone_count: u32,
    hl_count: u32,
    seg_count: u32,
    mk_count: u32,
    pending: Option<Pending>,
    device_ptr: *mut c_void,
}

impl UserDataLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            zone_count: 0,
            hl_count: 0,
            seg_count: 0,
            mk_count: 0,
            pending: None,
            device_ptr: std::ptr::null_mut(),
        }
    }

    /// Залить геометрию ордеров (целиком). Зовётся по изменению ордеров/вида (мутация).
    pub fn set(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.pending = Some(Pending {
            zone: zones.iter().map(zone_of).collect(),
            hl: hlines.iter().map(hl_of).collect(),
            seg: segs.iter().map(seg_of).collect(),
            mk: markers.iter().map(mk_of).collect(),
        });
    }

    /// Рисует ордера поверх данных. `view` — тот же chart_area-трансформ, что у combo
    /// (линии тянутся в зону стакана; scissor не ставим — как у движка друга).
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        // device-lost: пересоздать pipe; счётчики 0 — буферы пересоздаются пустыми (prepare зальёт
        // ордера заново этим же кадром через set()/pending, инвариант: новый device = 0 валидных).
        let device_ptr = d3d_device_ptr(gpu);
        if self.device_ptr != device_ptr {
            self.pipe = None;
            self.zone_count = 0;
            self.hl_count = 0;
            self.seg_count = 0;
            self.mk_count = 0;
            self.device_ptr = device_ptr;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(p) = self.pending.take() {
            self.zone_count = upload_capped(context, &pipe.zone_buf, &p.zone);
            self.hl_count = upload_capped(context, &pipe.hl_buf, &p.hl);
            self.seg_count = upload_capped(context, &pipe.seg_buf, &p.seg);
            self.mk_count = upload_capped(context, &pipe.mk_buf, &p.mk);
        }
        if self.zone_count == 0 && self.hl_count == 0 && self.seg_count == 0 && self.mk_count == 0 {
            return;
        }
        update_dynamic(context, &pipe.view_cb, &[*view]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            // Зоны → горизонтали (вход/стоп/liq) → отрезки (лестница) → маркеры (поверх).
            if self.zone_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.zone_srv.clone())]));
                context.VSSetShader(&pipe.zone_vs, None);
                context.PSSetShader(&pipe.zone_ps, None);
                context.DrawInstanced(6, self.zone_count, 0, 0);
            }
            if self.hl_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.hl_srv.clone())]));
                context.VSSetShader(&pipe.hl_vs, None);
                context.PSSetShader(&pipe.hl_ps, None);
                context.DrawInstanced(6, self.hl_count, 0, 0);
            }
            if self.seg_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.seg_srv.clone())]));
                context.VSSetShader(&pipe.seg_vs, None);
                context.PSSetShader(&pipe.seg_ps, None);
                context.DrawInstanced(6, self.seg_count, 0, 0);
            }
            if self.mk_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.mk_srv.clone())]));
                context.VSSetShader(&pipe.mk_vs, None);
                context.PSSetShader(&pipe.mk_ps, None);
                context.DrawInstanced(6, self.mk_count, 0, 0);
            }
        }
    }

    fn create_pipe(device: &ID3D11Device) -> UdPipe {
        let hl_buf = create_structured(device, std::mem::size_of::<HLineGpu>() as u32, CAP);
        let zone_buf = create_structured(device, std::mem::size_of::<ZoneGpu>() as u32, CAP);
        let seg_buf = create_structured(device, std::mem::size_of::<SegGpu>() as u32, CAP);
        let mk_buf = create_structured(device, std::mem::size_of::<MarkerGpu>() as u32, CAP);
        UdPipe {
            zone_vs: make_vs(device, HLSL, "zone_vertex"),
            zone_ps: make_ps(device, HLSL, "zone_fragment"),
            hl_vs: make_vs(device, HLSL, "hline_vertex"),
            hl_ps: make_ps(device, HLSL, "hline_fragment"),
            seg_vs: make_vs(device, HLSL, "seg_vertex"),
            seg_ps: make_ps(device, HLSL, "seg_fragment"),
            mk_vs: make_vs(device, HLSL, "marker_vertex"),
            mk_ps: make_ps(device, HLSL, "marker_fragment"),
            blend: create_alpha_blend(device),
            view_cb: create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32),
            zone_srv: create_srv(device, &zone_buf),
            hl_srv: create_srv(device, &hl_buf),
            seg_srv: create_srv(device, &seg_buf),
            mk_srv: create_srv(device, &mk_buf),
            zone_buf,
            hl_buf,
            seg_buf,
            mk_buf,
        }
    }
}

fn upload_capped<T: Copy>(context: &ID3D11DeviceContext, buf: &ID3D11Buffer, data: &[T]) -> u32 {
    let data: &[T] = if data.len() as u32 > CAP {
        &data[..CAP as usize]
    } else {
        data
    };
    if !data.is_empty() {
        update_dynamic(context, buf, data);
    }
    data.len() as u32
}
