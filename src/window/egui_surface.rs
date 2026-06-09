//! Общий egui-сурфейс для простых окон (Настройки/Отчёты/Стратегии): wgpu surface
//! + egui-контекст + рендерер + dirty-флаг, и весь цикл кадра одним методом.
//!
//! До этого каждое из трёх окон дословно повторяло ~90 строк бойлерплейта
//! (get_current_texture → encoder → run(ui) → tessellate → render_pass → present).
//! Теперь это здесь один раз; окно лишь отдаёт замыкание с egui-вёрсткой. Окно
//! группы (host) рендерит чарт+оверлей в общий encoder двумя проходами и держит
//! свой собственный egui-конвейер — оно осознанно не использует этот хелпер.

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::gpu::GpuContext;
use crate::shell::theme;

/// Цвет очистки фона окон-утилит (тот же «--surface-1» в линейном sRGB, что был
/// захардкожен в каждом окне отдельно).
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0745,
    g: 0.0784,
    b: 0.0863,
    a: 1.0,
};

pub struct EguiSurface {
    gpu: GpuContext,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    dirty: bool,
}

impl EguiSurface {
    /// Поднимает wgpu-сурфейс окна и egui-конвейер поверх него (с темой проекта).
    pub fn new(window: &std::sync::Arc<Window>) -> anyhow::Result<Self> {
        let gpu = GpuContext::new(window.clone())?;
        let ctx = egui::Context::default();
        theme::apply(&ctx); // тема проекта: шрифт Geist Mono + стиль виджетов
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);
        Ok(Self {
            gpu,
            ctx,
            state,
            renderer,
            dirty: true,
        })
    }

    /// Прокинуть событие в egui; помечает кадр грязным. Возвращает `consumed`.
    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.dirty = true;
        self.state.on_window_event(window, event).consumed
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.gpu.resize(size);
        self.dirty = true;
    }

    pub fn needs_render(&self) -> bool {
        self.dirty
    }

    /// Принудительно перерисовать на следующем кадре (напр. изменился статус ядра
    /// или пришёл новый снимок данных).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Один кадр egui: `run` рисует вёрстку. Весь wgpu-бойлерплейт внутри.
    /// `label` — метка encoder/pass для отладки. Кадр со «слетевшим» сурфейсом
    /// пропускается (dirty остаётся, ретрай на следующем тике).
    pub fn render(&mut self, window: &Window, label: &str, run: impl FnMut(&egui::Context)) {
        let Some((frame, view, mut encoder)) = self.gpu.begin_frame(label) else {
            return;
        };

        let raw_input = self.state.take_egui_input(window);
        let full_output = self.ctx.run(raw_input, run);

        self.state
            .handle_platform_output(window, full_output.platform_output);
        let tris = self
            .ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        for (id, delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        self.renderer
            .update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &tris, &screen);
        {
            let mut rpass = crate::gpu::egui_pass(&mut encoder, &view, label, Some(CLEAR));
            self.renderer.render(&mut rpass, &tris, &screen);
        }
        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        self.dirty = self.ctx.has_requested_repaint();
    }
}
