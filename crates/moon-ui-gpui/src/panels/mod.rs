//! Dock-панели окна группы (порт egui `src/dock/*`) как `gpui_component::dock::Panel` —
//! получают вкладки, сплиты, отцепление в окно и персист раскладки от `DockArea`
//! (см. [[prefer-gpui-components]]). По файлу на панель:
//! - [`chart`] — чарт (offscreen wgpu-движок + ввод + оси), центр дока;
//! - [`detects`] — лента детектов группы (откпрепляемая);
//! - [`orders`] — виртуализированная таблица ордеров;
//! - [`order`] — кнопки BUY/SELL/Cancel/Panic;
//! - [`stub`] — заглушки Активы/Лог/Отчёт до подключения данных.

mod chart;
mod detects;
mod order;
mod orders;
mod stub;

pub use chart::ChartPanel;
pub use detects::DetectsPanel;
pub use order::OrderPanel;
pub use orders::OrdersPanel;
pub use stub::StubPanel;
