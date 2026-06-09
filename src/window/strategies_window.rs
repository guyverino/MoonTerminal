//! Отдельное нативное окно «Стратегии»: 4 панели (дерево/секции/параметры/хэлп).
//! Читает аккуратный план ядер из `SessionManager` (store + имена ядер); по
//! «Применить» возвращает наружу команды старт/стоп (App шлёт их через session).

use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::gpu::GpuContext;
use crate::session::{CoreId, SessionManager};
use crate::shell::theme;
use crate::strategies::{StratAction, StrategiesState};

/// Итог кадра окна для App: действия со стратегиями (синхронизация галок + старт/стоп).
pub struct StrategiesWinOut {
    pub actions: Vec<StratAction>,
}

pub struct StrategiesWindow {
    pub window: Arc<Window>,
    gpu: GpuContext,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    dirty: bool,
    state: StrategiesState,
    /// Сигнатура данных стратегий/схемы по ядрам — для авто-перерисовки при приходе
    /// новых снимков (как generation у окна отчётов).
    last_data_sig: u64,
}

impl StrategiesWindow {
    pub fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(t!("strat.window_title").to_string())
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1180.0, 680.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_window_icon(crate::icons::brand_winit_icon());

        let gpu = GpuContext::new(window.clone())?;
        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx); // тема проекта: шрифт Geist Mono + стиль виджетов
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);

        Ok(Self {
            window,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            dirty: true,
            state: StrategiesState::default(),
            last_data_sig: 0,
        })
    }

    /// Сверяет сигнатуру данных стратегий/схемы всех ядер: пришёл новый снимок →
    /// перерисовать (отразить новые checked/состав/схему).
    pub fn poll(&mut self, session: &SessionManager) {
        let mut sig = 0u64;
        for s in &session.sessions {
            if let Some(cd) = session.store.core(s.id) {
                sig = sig
                    .wrapping_add(cd.strategies_rev)
                    .wrapping_mul(31)
                    .wrapping_add(cd.schema_rev);
            }
        }
        if sig != self.last_data_sig {
            self.last_data_sig = sig;
            self.dirty = true;
        }
        // Hot-reload правил зависимостей (param_deps.toml) — правка файла видна на лету.
        if self.state.rules.reload_if_changed() {
            self.dirty = true;
        }
    }

    pub fn on_egui_event(&mut self, event: &WindowEvent) -> bool {
        self.dirty = true;
        self.egui_state.on_window_event(&self.window, event).consumed
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.gpu.resize(size);
        self.dirty = true;
    }

    pub fn needs_render(&self) -> bool {
        self.dirty
    }

    /// Кадр окна. Читает данные из `session`; возвращает команды старт/стоп.
    pub fn render(&mut self, session: &SessionManager) -> StrategiesWinOut {
        let mut out = StrategiesWinOut {
            actions: Vec::new(),
        };

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.gpu.surface.configure(&self.gpu.device, &self.gpu.config);
                return out;
            }
            Err(e) => {
                log::warn!("strategies surface error: {e:?}");
                return out;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("strategies-encoder"),
            });

        let cores: Vec<(CoreId, String)> = session
            .sessions
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let state = &mut self.state;
        let store = &session.store;
        let mut panel_out = crate::strategies::StrategiesOut::default();
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            panel_out = state.ui(ctx, &cores, store);
        });
        out.actions = panel_out.actions;

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        for (id, delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        self.egui_renderer
            .update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &tris, &screen);
        {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("strategies-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0745,
                            g: 0.0784,
                            b: 0.0863,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut rpass = rpass.forget_lifetime();
            self.egui_renderer.render(&mut rpass, &tris, &screen);
        }
        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        self.dirty = self.egui_ctx.has_requested_repaint();
        out
    }
}
