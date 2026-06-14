//! Combo-слой: вся неизменная рыночная история (Trades; PriceLines/Volume — добавятся
//! сюда же). Кресты лежат в резидентном кольце VRAM; «фон-битмап» combo шире экрана на
//! +20% запекается крестовым шейдером и блитится с UV-паном. Прошлое неизменно → двигаем
//! готовый битмап (scroll) + дорисовываем (append) живой край, НЕ перерисовывая историю.
//!
//! Защита от device-lost (P0-4): при смене raw-указателя device хука (GPUI пересоздал
//! устройство) сбрасываем ВСЕ ресурсы — иначе рисовали бы stale-буферами на новом контексте.

use std::ffi::c_void;

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_point_sampler, create_srv, create_srv_range,
    create_structured, full_viewport, ring_write_no_overwrite, update_dynamic, BlitParams,
    ChartCross, ChartViewGpu,
};

/// Ёмкость кольца тиков (~131k, плотно под 100k видимых на максимальной плотности).
const CAP: u32 = 1 << 17;
const CROSSES_HLSL: &str = include_str!("shaders/crosses.hlsl");
const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");

/// Pipeline крестов + резидентное кольцо тиков в VRAM.
struct CrossPipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
}

/// Фон-битмап combo (W*1.2 × H): запечённая история + точка привязки UV-скролла.
struct ComboTex {
    _tex: ID3D11Texture2D, // RAII: держит текстуру (rtv/srv ссылаются)
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    tex_w: u32,
    tex_h: u32,
    blit_vs: ID3D11VertexShader,
    blit_fs: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    bake_t0: f32,
    last_baked_head: u32,
    valid: bool,
}

pub struct ComboLayer {
    pipe: Option<CrossPipe>,
    tex: Option<ComboTex>,
    count: u32,
    head: u32,
    pending_reset: Option<Vec<ChartCross>>,
    pending_append: Vec<ChartCross>,
    /// Raw-указатель device, на котором созданы ресурсы. Сменился → device-lost, сбрасываем.
    device_ptr: *mut c_void,
    /// Поколение device: ++ при пересоздании (device-lost). Оркестратор сравнивает со своим
    /// last → перезаливает ВСЮ историю (кольцо новое и пустое, append живого края не хватит).
    device_gen: u64,
}

impl ComboLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            tex: None,
            count: 0,
            head: 0,
            pending_reset: None,
            pending_append: Vec::new(),
            device_ptr: std::ptr::null_mut(),
            device_gen: 0,
        }
    }

    /// Поколение device combo (++ на каждый device-lost). Оркестратор сравнивает со своим
    /// last_device_gen: сменилось → кольцо пустое, нужна полная перезаливка истории.
    pub fn device_gen(&self) -> u64 {
        self.device_gen
    }

    /// Полная перезаливка набора тиков (reload истории монеты). Сбрасывает append.
    pub fn reset(&mut self, data: Vec<ChartCross>) {
        self.pending_reset = Some(data);
        self.pending_append.clear();
    }

    /// Дополнить кольцо новыми тиками (живой край) — каждый приход данных.
    pub fn append(&mut self, data: &[ChartCross]) {
        if !data.is_empty() {
            self.pending_append.extend_from_slice(data);
        }
    }

    /// Рисует Combo в backbuffer хука (фаза UnderScene). `view` — трансформ ЭТОЙ панели.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        // device-lost guard (P0-4): новый device → старые буферы/шейдеры/кольцо невалидны.
        // Сбрасываем ресурсы И счётчики кольца: пересозданный буфер пуст, а stale count заставил
        // бы DrawInstanced читать мусор. device_gen++ → prepare перезальёт всю историю (collect_all).
        if self.device_ptr != gpu.device {
            self.pipe = None;
            self.tex = None;
            self.count = 0;
            self.head = 0;
            self.device_ptr = gpu.device;
            self.device_gen = self.device_gen.wrapping_add(1);
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        self.apply_uploads(context);
        if self.count == 0 {
            return;
        }
        self.render_combo(view, device, context, rtv, gpu);
    }

    /// Combo: инкрементальный bake новых тиков в текстуру + блит видимого окна с UV-паном.
    /// Полный re-bake при исчерпании 20%-запаса или невалидном битмапе (зум/resize/первый кадр).
    fn render_combo(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        let tex_w = (bw * 1.2).round().max(1.0) as u32;
        let tex_h = bh.round().max(1.0) as u32;
        let need_new = self
            .tex
            .as_ref()
            .map_or(true, |c| c.tex_w != tex_w || c.tex_h != tex_h);
        if need_new {
            self.tex = Some(Self::create_tex(device, tex_w, tex_h));
        }
        let pipe = self.pipe.as_ref().unwrap();
        let tex = self.tex.as_mut().unwrap();
        let ttp = view.time_to_px;
        let margin_px = bw * 0.2;
        let mut u_left_px = (view.view_time0 - tex.bake_t0) * ttp;
        let need_full = !tex.valid || u_left_px < 0.0 || u_left_px > margin_px;
        // bake-юнформ: левый край времени = bake_t0 (фикс), viewport = весь битмап.
        let bake_view = ChartViewGpu {
            bounds: [0.0, 0.0, tex_w as f32, tex_h as f32],
            resolution: [tex_w as f32, tex_h as f32],
            time_to_px: ttp,
            view_time0: if need_full { view.view_time0 } else { tex.bake_t0 },
            price_to_px: view.price_to_px,
            view_price0: view.view_price0,
            marker_half: view.marker_half,
            pad: 0.0,
        };
        update_dynamic(context, &pipe.view_cb, &[bake_view]);
        let tex_vp = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: tex_w as f32,
            Height: tex_h as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(tex.rtv.clone())]), None);
            context.RSSetViewports(Some(&[tex_vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            if need_full {
                tex.bake_t0 = view.view_time0;
                u_left_px = 0.0;
                // ПРОЗРАЧНЫЙ фон битмапа: только кресты непрозрачны → при блите (alpha) сетка/фон
                // нижнего слоя (grid) просвечивают между крестами. Фон #131416 красит grid-слой.
                context.ClearRenderTargetView(&tex.rtv, &[0.0, 0.0, 0.0, 0.0]);
                context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                context.DrawInstanced(6, self.count, 0, 0);
                tex.last_baked_head = self.head;
                tex.valid = true;
            } else if self.head != tex.last_baked_head {
                // инкрементально: только новые тики кольца [last_head, head) (с заворотом)
                let delta = (self.head + CAP - tex.last_baked_head) % CAP;
                let runs: [(u32, u32); 2] = if tex.last_baked_head + delta <= CAP {
                    [(tex.last_baked_head, delta), (0, 0)]
                } else {
                    [
                        (tex.last_baked_head, CAP - tex.last_baked_head),
                        (0, delta - (CAP - tex.last_baked_head)),
                    ]
                };
                for (rf, rc) in runs {
                    if rc == 0 {
                        continue;
                    }
                    let srv_r = create_srv_range(device, &pipe.buffer, rf, rc);
                    context.VSSetShaderResources(1, Some(&[Some(srv_r)]));
                    context.DrawInstanced(6, rc, 0, 0);
                }
                tex.last_baked_head = self.head;
            }
        }
        // композит: блит видимого окна битмапа → чарт-область backbuffer (point-семпл, UV-пан).
        let u_left = u_left_px / tex_w as f32;
        let u_span = bw / tex_w as f32;
        let bp = BlitParams {
            dst: view.bounds,
            resolution: view.resolution,
            uv_off: [u_left, 0.0],
            uv_scale: [u_span, 1.0],
            pad: [0.0, 0.0],
        };
        update_dynamic(context, &tex.blit_cb, &[bp]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&tex.blit_vs, None);
            context.PSSetShader(&tex.blit_fs, None);
            context.VSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(tex.srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(tex.sampler.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
    }

    fn apply_uploads(&mut self, context: &ID3D11DeviceContext) {
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(data) = self.pending_reset.take() {
            // при переполнении оставляем последний хвост ёмкости
            let data: &[ChartCross] = if data.len() as u32 > CAP {
                &data[data.len() - CAP as usize..]
            } else {
                &data
            };
            update_dynamic(context, &pipe.buffer, data);
            self.count = data.len() as u32;
            self.head = (data.len() as u32) % CAP;
        }
        if !self.pending_append.is_empty() {
            let data = std::mem::take(&mut self.pending_append);
            let data: &[ChartCross] = if data.len() as u32 > CAP {
                &data[data.len() - CAP as usize..]
            } else {
                &data
            };
            let n = data.len() as u32;
            ring_write_no_overwrite(context, &pipe.buffer, self.head, CAP, data);
            self.head = (self.head + n) % CAP;
            self.count = (self.count + n).min(CAP);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CrossPipe {
        let vs = super::gpu::make_vs(device, CROSSES_HLSL, "crosses_vertex");
        let ps = super::gpu::make_ps(device, CROSSES_HLSL, "crosses_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(device, std::mem::size_of::<ChartCross>() as u32, CAP);
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        CrossPipe { vs, ps, blend, buffer, srv, view_cb }
    }

    fn create_tex(device: &ID3D11Device, tex_w: u32, tex_h: u32) -> ComboTex {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: tex_w,
            Height: tex_h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let tex = unsafe {
            let mut o = None;
            device.CreateTexture2D(&desc, None, Some(&mut o)).unwrap();
            o.unwrap()
        };
        let rtv = unsafe {
            let mut o = None;
            device.CreateRenderTargetView(&tex, None, Some(&mut o)).unwrap();
            o.unwrap()
        };
        let srv = unsafe {
            let mut o = None;
            device.CreateShaderResourceView(&tex, None, Some(&mut o)).unwrap();
            o.unwrap()
        };
        let blit_vs = super::gpu::make_vs(device, BLIT_HLSL, "blit_vertex");
        let blit_fs = super::gpu::make_ps(device, BLIT_HLSL, "blit_fragment");
        let blit_cb = create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32);
        let sampler = create_point_sampler(device);
        ComboTex {
            _tex: tex,
            rtv,
            srv,
            tex_w,
            tex_h,
            blit_vs,
            blit_fs,
            blit_cb,
            sampler,
            bake_t0: 0.0,
            last_baked_head: u32::MAX,
            valid: false,
        }
    }
}
