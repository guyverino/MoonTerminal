//! Окна терминала. Сейчас — по ОС-окну на группу (WindowHost).

pub mod host;
pub mod reports_window;
pub mod settings_window;

pub use host::{HostRender, WindowHost};
pub use reports_window::ReportsWindow;
pub use settings_window::SettingsWindow;
