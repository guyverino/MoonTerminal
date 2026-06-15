//! Общие мелочи времени для UI-оболочки. wgpu-рендер панелей (`render_panes`) и
//! сигнатура видимых панелей (`panes_visible_sig`) удалены вместе с egui-движком —
//! own-pass (chartdx) имеет свой рендер и `data_signature`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Текущее unix-время в мс (та же шкала, что time_ms тиков).
pub fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
