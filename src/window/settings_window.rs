//! Отдельное нативное окно настроек: обычное ОС-окно (системная рамка + крестик),
//! вкладки в ScrollArea, футер с «Сохранить». egui-конвейер — общий [`EguiSurface`].

use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::config::AppConfig;
use crate::icons::IconSet;
use crate::settings::{CoreStatuses, SettingsState};
use crate::window::{AuxWindow, EguiSurface};

pub struct SettingsWinOut {
    pub saved: bool,
}

pub struct SettingsWindow {
    pub window: Arc<Window>,
    egui: EguiSurface,
    icons: IconSet,
}

impl SettingsWindow {
    pub fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(t!("settings.window_title").to_string())
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(860.0, 580.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        // Иконка окна/taskbar — общая «лунная» 0.png (как у окна отчётов). Вшита в
        // бинарь, поэтому показывается и в release-запуске без assets/ рядом с exe.
        window.set_window_icon(crate::icons::brand_winit_icon());

        let egui = EguiSurface::new(&window)?;
        Ok(Self {
            window,
            egui,
            icons: IconSet::discover(),
        })
    }

    pub fn needs_render(&self) -> bool {
        self.egui.needs_render()
    }

    /// Принудительно перерисовать на следующем кадре (напр. изменился статус ядра).
    pub fn mark_dirty(&mut self) {
        self.egui.mark_dirty();
    }

    pub fn render(
        &mut self,
        settings: &mut SettingsState,
        config: &mut AppConfig,
        status: &CoreStatuses,
    ) -> SettingsWinOut {
        let mut saved = false;
        let icons = &mut self.icons;
        self.egui.render(&self.window, "settings-pass", |ctx| {
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
                settings.body(ui, icons, status);
            });
        });
        SettingsWinOut { saved }
    }
}

impl AuxWindow for SettingsWindow {
    fn on_event(&mut self, event: &WindowEvent) -> bool {
        self.egui.on_event(&self.window, event)
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.egui.resize(size);
    }
}
