//! Отдельное нативное окно настроек: обычное ОС-окно (системная рамка + крестик),
//! вкладки в ScrollArea, футер с «Сохранить». Свой surface + egui.

use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::config::AppConfig;
use crate::gpu::GpuContext;
use crate::icons::IconSet;
use crate::settings::SettingsState;
use crate::shell::theme;

pub struct SettingsWinOut {
    pub saved: bool,
}

pub struct SettingsWindow {
    pub window: Arc<Window>,
    gpu: GpuContext,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    icons: IconSet,
    dirty: bool,
}

impl SettingsWindow {
    pub fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(t!("settings.window_title").to_string())
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(860.0, 580.0));
        let window = Arc::new(event_loop.create_window(attrs)?);

        let gpu = GpuContext::new(window.clone())?;
        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx);
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
            icons: IconSet::discover(),
            dirty: true,
        })
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

    pub fn render(
        &mut self,
        settings: &mut SettingsState,
        config: &mut AppConfig,
    ) -> SettingsWinOut {
        let mut out = SettingsWinOut { saved: false };

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.gpu.surface.configure(&self.gpu.device, &self.gpu.config);
                return out;
            }
            Err(e) => {
                log::warn!("settings surface error: {e:?}");
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
                label: Some("settings-encoder"),
            });

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let icons = &mut self.icons;
        let mut saved = false;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // Футер с кнопкой Сохранить (вне прокрутки).
            egui::TopBottomPanel::bottom("settings-footer")
                .exact_height(40.0)
                .show(ctx, |ui| {
                    ui.add_space(6.0);
                    if let Some(new_cfg) = settings.footer(ui) {
                        // footer уже провалидировал и записал файлы.
                        *config = new_cfg;
                        saved = true;
                    }
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                settings.body(ui, icons);
            });
        });
        out.saved = saved;

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        for (tex_id, delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *tex_id, delta);
        }
        self.egui_renderer
            .update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &tris, &screen);
        {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings-pass"),
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
        for tex_id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(tex_id);
        }

        self.dirty = self.egui_ctx.has_requested_repaint();
        out
    }
}
