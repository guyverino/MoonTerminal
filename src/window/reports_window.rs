//! Откреплённое нативное окно «Отчёты» — тонкая обёртка: общий [`EguiSurface`] +
//! [`ReportView`] (всё состояние и рендер таблицы живут в нём, переиспользуются и
//! вкладкой «Отчёт» нижнего дока). Автообновление по счётчику-генерации writer'а.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::dock::ReportView;
use crate::window::{AuxWindow, EguiSurface};

pub struct ReportsWindow {
    pub window: Arc<Window>,
    egui: EguiSurface,
    view: ReportView,
}

impl ReportsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        generation: Option<Arc<AtomicU64>>,
    ) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("Отчёты — MoonTerminal")
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 660.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        // Иконка окна/taskbar — общая «лунная» 0.png, вшитая в бинарь (не зависит от
        // наличия assets/ рядом с exe в release).
        window.set_window_icon(crate::icons::brand_winit_icon());

        let egui = EguiSurface::new(&window)?;
        let view = ReportView::new(generation);
        Ok(Self { window, egui, view })
    }

    pub fn needs_render(&self) -> bool {
        self.egui.needs_render()
    }

    /// Новые/изменённые отчёты от writer → перерисовать (перезапрос делает view).
    pub fn poll(&mut self) {
        if self.view.poll() {
            self.egui.mark_dirty();
        }
    }

    pub fn render(&mut self) {
        let view = &mut self.view;
        self.egui.render(&self.window, "reports-pass", |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| view.ui(ui));
        });
    }
}

impl AuxWindow for ReportsWindow {
    fn on_event(&mut self, event: &WindowEvent) -> bool {
        self.egui.on_event(&self.window, event)
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.egui.resize(size);
    }
}
