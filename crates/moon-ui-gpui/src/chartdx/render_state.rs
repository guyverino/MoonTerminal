//! Рендер-состояние чарта (`impl RenderState`): сведение per-pane GPU-состояния,
//! тайминг present (пейсер ~60Гц), курсор/ридаут, отрисовка слоёв own-pass.
//! Вынесено из `mod.rs`; структура `RenderState` объявлена там.

use super::*;

impl RenderState {
    pub(super) fn set_target_present_rate_hz(&mut self, hz: f32) {
        let hz = hz.clamp(1.0, 240.0);
        self.target_present_interval_ms = 1000.0 / hz as f64;
    }

    pub(super) fn record_camera_shift(&mut self, now_ms: f64) {
        if self.camera_shift_window_start_ms <= 0.0 {
            self.camera_shift_window_start_ms = now_ms;
        }
        self.camera_shift_count = self.camera_shift_count.saturating_add(1);
        self.update_camera_shift_hz(now_ms);
    }

    pub(super) fn camera_shift_hz(&mut self, now_ms: f64) -> f32 {
        self.update_camera_shift_hz(now_ms);
        self.camera_shift_hz
    }

    pub(super) fn update_camera_shift_hz(&mut self, now_ms: f64) {
        if self.camera_shift_window_start_ms <= 0.0 {
            self.camera_shift_window_start_ms = now_ms;
            return;
        }
        let elapsed = now_ms - self.camera_shift_window_start_ms;
        if elapsed < 1000.0 {
            return;
        }
        self.camera_shift_hz = self.camera_shift_count as f32 * 1000.0 / elapsed.max(1.0) as f32;
        self.camera_shift_count = 0;
        self.camera_shift_window_start_ms = now_ms;
    }

    pub(super) fn set_slot_origin(&mut self, x: f32, y: f32) {
        let next = [x, y];
        if self.slot_origin != next {
            self.slot_origin = next;
            self.base_dirty = true;
            self.needs_present = true;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_cursor_style(&mut self, color: [f32; 4], thickness: f32) {
        let thickness = thickness.max(1.0);
        if self.cursor_color != color || self.cursor_thickness != thickness {
            self.cursor_color = color;
            self.cursor_thickness = thickness;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_pixel_scale(&mut self, scale: f32) {
        let scale = scale.max(0.1);
        if (self.pixel_scale - scale).abs() > 0.001 {
            self.pixel_scale = scale;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_cursor(&mut self, cursor: Option<CursorState>) -> bool {
        if self.cursor == cursor {
            return false;
        }
        self.cursor = cursor;
        self.sync_cursor_params();
        self.needs_present = true;
        true
    }

    pub(super) fn sync_cursor_params(&mut self) {
        for (idx, pr) in self.panes.iter_mut().enumerate() {
            let right = (pr.orderbook_view.bounds[0] + pr.orderbook_view.bounds[2])
                .max(pr.view.bounds[0] + pr.view.bounds[2]);
            let bounds = [
                pr.view.bounds[0],
                pr.view.bounds[1],
                (right - pr.view.bounds[0]).max(1.0),
                pr.view.bounds[3].max(1.0),
            ];
            let mut params = CursorParams {
                bounds,
                resolution: pr.view.resolution,
                color: self.cursor_color,
                thickness: self.cursor_thickness.max(1.0),
                ..CursorParams::default()
            };
            if pr.active {
                if let Some(cursor) = self.cursor.filter(|c| c.pane == idx) {
                    params.cursor = [
                        self.slot_origin[0] + cursor.local[0],
                        self.slot_origin[1] + cursor.local[1],
                    ];
                    params.enabled = 1.0;
                }
            }
            #[cfg(not(windows))]
            let changed = pr.cursor_params != params;
            pr.cursor_params = params;
            #[cfg(not(windows))]
            if changed {
                pr.gpu_prepare_dirty = true;
            }
        }
        self.sync_readout_params();
    }

    pub(super) fn sync_readout_params(&mut self) {
        for pr in &mut self.panes {
            pr.readout_rects.clear();
            pr.readout_glyphs.clear();
        }
    }

    pub(super) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        crate::diag::bump(&crate::diag::CHART_FRAME);
        if !info.presentable || info.bounds.is_empty() {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_NOT_PRESENTABLE);
            return GpuFrameDecision::Skip;
        }

        let now_ms = now_unix_ms();
        let mut wants_present = std::mem::take(&mut self.needs_present);
        let cap_due = self.last_present_ms <= 0.0
            || now_ms - self.last_present_ms >= self.target_present_interval_ms;
        let mut camera_moved = false;
        for pr in &mut self.panes {
            if pr.active && (wants_present || cap_due) && pr.advance_camera(now_ms) {
                crate::diag::bump(&crate::diag::CHART_CAM_STEP);
                camera_moved = true;
                self.base_dirty = true;
                wants_present = true;
            }
        }
        if camera_moved {
            self.record_camera_shift(now_ms);
        }

        if wants_present {
            self.last_present_ms = now_ms;
            crate::diag::bump(&crate::diag::CHART_FRAME_REQUEST);
            GpuFrameDecision::RequestPresent
        } else {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_IDLE);
            GpuFrameDecision::Skip
        }
    }

    pub(super) fn prepare_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        let generation = gpu.device_generation();
        if self.last_gpu_prepare_generation != generation {
            self.last_gpu_prepare_generation = generation;
            self.base_dirty = true;
            for pr in &mut self.panes {
                pr.gpu_prepare_dirty = true;
            }
        }

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let Some((device, context, _rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 prepare received empty D3D11 raw gpu handles");
                };
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if !pr.active || !pr.gpu_prepare_dirty {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_d3d(
                        &view,
                        &orderbook_view,
                        &pr.book_style,
                        &device,
                        &context,
                        gpu,
                    );
                    pr.gpu_prepare_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                let rebuild_base = self.base_dirty;
                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let needs_base = rebuild_base || pr.layers.needs_base_cache(gpu);
                    if !pr.gpu_prepare_dirty && !needs_base {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut background_params = pr.background_params;
                    let mut grid_params = pr.grid_params;
                    let mut cursor_params = pr.cursor_params;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    background_params.resolution = res;
                    grid_params.resolution = res;
                    cursor_params.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_wgpu(
                        &view,
                        &background_params,
                        &grid_params,
                        &cursor_params,
                        &orderbook_view,
                        &pr.book_style,
                        gpu,
                        needs_base,
                    )?;
                    pr.gpu_prepare_dirty = false;
                }
                if rebuild_base {
                    self.base_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                let rebuild_base = self.base_dirty;
                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let needs_base = rebuild_base || pr.layers.needs_base_cache(gpu);
                    if !pr.gpu_prepare_dirty && !needs_base {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut background_params = pr.background_params;
                    let mut grid_params = pr.grid_params;
                    let mut cursor_params = pr.cursor_params;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    background_params.resolution = res;
                    grid_params.resolution = res;
                    cursor_params.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_metal(
                        &view,
                        &background_params,
                        &grid_params,
                        &cursor_params,
                        &orderbook_view,
                        &pr.book_style,
                        gpu,
                        needs_base,
                    )?;
                    pr.gpu_prepare_dirty = false;
                }
                if rebuild_base {
                    self.base_dirty = false;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    #[cfg(windows)]
    pub(super) fn render_window_background_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        let base = BackgroundParams {
            dst: [0.0, 0.0, res[0], res[1]],
            resolution: res,
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            opacity: 0.0,
            _pad: 0.0,
            bg: self.window_bg_color,
        };
        self.window_bg.render(&base, device, context, rtv, gpu);
    }

    #[cfg(windows)]
    pub(super) fn render_chart_base_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        scissor_rs: &ID3D11RasterizerState,
    ) {
        for pr in &mut self.panes {
            if !pr.active {
                continue;
            }
            let mut view = pr.view;
            let mut background_params = pr.background_params;
            let mut grid_params = pr.grid_params;
            let mut orderbook_view = pr.orderbook_view;
            view.resolution = res;
            background_params.resolution = res;
            grid_params.resolution = res;
            orderbook_view.resolution = res;
            let panel_clip = [
                view.bounds[0],
                view.bounds[1],
                orderbook_view.bounds[0] + orderbook_view.bounds[2],
                view.bounds[1] + view.bounds[3],
            ];
            gpu::set_scissor(
                context,
                scissor_rs,
                panel_clip[0],
                panel_clip[1],
                panel_clip[2],
                panel_clip[3],
            );
            pr.layers.render_base_d3d(
                &view,
                &background_params,
                &grid_params,
                &orderbook_view,
                &pr.book_style,
                device,
                context,
                rtv,
                gpu,
                panel_clip,
            );
        }
    }

    pub(super) fn draw_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        crate::diag::bump(&crate::diag::CHART_PRESENT);

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let RawGpuAccess::D3d11(d3d) = gpu else {
                    anyhow::bail!("chart dx11 draw received non-D3D11 raw gpu access");
                };
                let Some((device, context, rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 draw received empty D3D11 raw gpu handles");
                };

                let d3d_device_ptr = d3d.device.as_ptr();
                if self.scissor_dev != d3d_device_ptr {
                    self.scissor_rs = Some(gpu::create_scissor_rasterizer(&device));
                    self.scissor_dev = d3d_device_ptr;
                }
                let res = [width as f32, height as f32];
                let scissor_rs = self.scissor_rs.clone().unwrap();
                let prev_rs = unsafe { context.RSGetState().ok() };

                if self.base_dirty {
                    self.render_window_background_d3d(res, &device, &context, &rtv, gpu);
                    self.render_chart_base_d3d(res, &device, &context, &rtv, gpu, &scissor_rs);
                    self.base_cache.invalidate();
                    self.base_dirty = false;
                } else {
                    if self.base_cache.needs_rebuild(gpu) {
                        let base_rtv = self.base_cache.begin_rebuild(&device, &context, gpu)?;
                        self.render_window_background_d3d(res, &device, &context, &base_rtv, gpu);
                        self.render_chart_base_d3d(
                            res,
                            &device,
                            &context,
                            &base_rtv,
                            gpu,
                            &scissor_rs,
                        );
                    }
                    self.base_cache.blit_to(&context, &rtv, gpu);
                }

                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let mut cursor_params = pr.cursor_params;
                    let mut view = pr.view;
                    let mut orderbook_view = pr.orderbook_view;
                    cursor_params.resolution = res;
                    view.resolution = res;
                    orderbook_view.resolution = res;
                    let panel_clip = [
                        view.bounds[0],
                        view.bounds[1],
                        orderbook_view.bounds[0] + orderbook_view.bounds[2],
                        view.bounds[1] + view.bounds[3],
                    ];
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        panel_clip[0],
                        panel_clip[1],
                        panel_clip[2],
                        panel_clip[3],
                    );
                    pr.layers.render_cursor_d3d(
                        &cursor_params,
                        &pr.readout_rects,
                        &pr.readout_glyphs,
                        &device,
                        &context,
                        &rtv,
                        gpu,
                    );
                }
                unsafe {
                    context.RSSetState(prev_rs.as_ref());
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        pr.layers.render_wgpu(
                            &view,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &pr.readout_rects,
                            &pr.readout_glyphs,
                            &orderbook_view,
                            gpu,
                        )?;
                    }
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        pr.layers.render_metal(
                            &view,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &pr.readout_rects,
                            &pr.readout_glyphs,
                            &orderbook_view,
                            gpu,
                        )?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

