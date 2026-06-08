//! Состояние контролов дока: размеры ордера и пресеты масштаба.

pub struct OrderControls {
    /// 5 настраиваемых размеров ордера.
    pub sizes: [f64; 5],
    /// Активный размер (индекс).
    pub active_size: usize,
    /// Активный пресет масштаба (индекс в SCALES).
    pub scale_idx: usize,
}

impl Default for OrderControls {
    fn default() -> Self {
        Self {
            sizes: [10.0, 25.0, 50.0, 100.0, 250.0],
            active_size: 0,
            scale_idx: 0,
        }
    }
}

impl OrderControls {
    pub fn active_size(&self) -> f64 {
        self.sizes[self.active_size]
    }
}

/// Действие смены масштаба цены (Y), как ZoomBar в moonweb.
#[derive(Clone, Copy, Debug)]
pub enum ScaleAction {
    /// Динамический подгон под видимый диапазон.
    Auto,
    /// Видимый диапазон цены = текущая цена * доля.
    Percent(f32),
}

/// Пресеты масштаба ЦЕНЫ: (подпись, действие). idx 0 = «Авто».
/// Доли — как в moonweb ZoomBar: 50/20/10/5/2 % от цены.
pub const SCALES: [(&str, ScaleAction); 6] = [
    ("Авто", ScaleAction::Auto),
    ("50%", ScaleAction::Percent(0.50)),
    ("20%", ScaleAction::Percent(0.20)),
    ("10%", ScaleAction::Percent(0.10)),
    ("5%", ScaleAction::Percent(0.05)),
    ("2%", ScaleAction::Percent(0.02)),
];
