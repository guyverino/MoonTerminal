//! Окна терминала. Окно группы (WindowHost) рендерит чарт+док+шелл. Простые
//! окна-утилиты (Настройки/Отчёты/Стратегии) делят egui-конвейер через
//! [`EguiSurface`] и общий контракт [`AuxWindow`].

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;

pub mod chart_window;
pub mod egui_surface;
pub mod host;
pub mod settings_window;
pub mod strategies_window;

pub use chart_window::ChartWindow;
pub use egui_surface::EguiSurface;
pub use host::WindowHost;
pub use settings_window::SettingsWindow;
pub use strategies_window::StrategiesWindow;

/// Общий контракт окна-утилиты (Настройки/Отчёты/Стратегии): ввод и ресайз.
/// Рендер у каждого свой (разные данные/выходы) и в трейт не входит.
pub trait AuxWindow {
    /// Прокинуть событие в egui окна; возвращает `consumed`.
    fn on_event(&mut self, event: &WindowEvent) -> bool;
    fn resize(&mut self, size: PhysicalSize<u32>);
}

/// Единая обработка события окна-утилиты: отдать egui, ресайзнуть при Resized,
/// сообщить о запросе закрытия. Раньше этот блок был трижды скопирован в App.
/// Возвращает `true`, если окно нужно закрыть (CloseRequested).
pub fn handle_aux_event<W: AuxWindow>(w: &mut W, event: &WindowEvent) -> bool {
    w.on_event(event);
    if let WindowEvent::Resized(size) = event {
        w.resize(*size);
    }
    matches!(event, WindowEvent::CloseRequested)
}
