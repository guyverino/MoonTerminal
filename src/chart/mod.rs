//! Chart-фасад egui-оболочки. Движок (GPU-слои + render_panes) живёт в крейте
//! `moon_chart` (egui-free); здесь — ре-экспорт под прежними путями `crate::chart::*`
//! (чтобы остальной UI-код не править) + egui-специфика: подписи осей (`axes`),
//! egui-оверлей шкал/перекрестия (`overlay`) и обработка ввода winit (`input`).

// Движок (1:1, общий с GPUI-оболочкой). Ре-экспортим модули → `crate::chart::paint`,
// `crate::chart::container`, `crate::chart::view`, … резолвятся как раньше.
// allow(unused): часть имён — совместимый фасад, может не использоваться в bin.
#[allow(unused_imports)]
pub use moon_chart::{
    canvas, container, data, layers, paint, style, transform, view, Chart, GLASS_ZONE_PX,
    PRICE_AXIS_W, TIME_AXIS_H,
};

// egui-специфика оболочки (НЕ в движке: тянут egui/winit/shell::theme).
pub mod axes;
pub mod input;
pub mod overlay;
