//! Раскладка окон — отдельный переносимый `layout.toml` рядом с exe (как
//! `theme.toml`). Хранит позиции/размеры окон групп, свёрнут ли док и активную
//! вкладку, а также список откреплённых окон (какая вкладка, из какой группы,
//! геометрия). Общая на всех (один файл). Битый/отсутствующий файл → дефолт.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paths;

/// Геометрия+состояние окна группы (ключ карты — имя группы).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct GroupLayout {
    /// Внешняя позиция окна (физ. пиксели десктопа).
    pub x: i32,
    pub y: i32,
    /// Внутренний размер (физ. пиксели).
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub collapsed: bool,
    /// Индекс активной вкладки дока (см. `DockTab::idx`).
    #[serde(default)]
    pub tab: u8,
}

/// Прямоугольник окна (внешняя позиция + внутренний размер, физ. пиксели).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct GeomRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Одно откреплённое окно вкладки.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedLayout {
    /// Индекс вкладки (см. `DockTab::idx`).
    pub tab: u8,
    /// Имя группы-владельца (для Orders — чьи ордера; для глобальных — откуда открыт).
    pub owner_group: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Полная раскладка окон.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct WindowLayout {
    /// Окна групп по имени группы.
    #[serde(default)]
    pub groups: HashMap<String, GroupLayout>,
    /// Открытые откреплённые окна вкладок.
    #[serde(default)]
    pub detached: Vec<DetachedLayout>,
    /// Запомненная геометрия окон открепления по ключу (даже после закрытия) —
    /// чтобы повторное открепление той же вкладки вставало на прежнее место.
    /// Ключ: `g:<idx>` для глобальных, `o:<idx>:<группа>` для Orders (см. App).
    #[serde(default)]
    pub detached_geom: HashMap<String, GeomRect>,
}

impl WindowLayout {
    /// Загрузить layout.toml. Нет файла → дефолт; битый → лог + дефолт.
    pub fn load() -> Self {
        let path = paths::layout_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("layout.toml повреждён ({e}); начинаю с дефолта");
                Self::default()
            }
        }
    }

    /// Записать layout.toml (не фатально: при ошибке только лог).
    pub fn save(&self) {
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(paths::layout_path(), s) {
                    log::warn!("запись layout.toml: {e}");
                }
            }
            Err(e) => log::warn!("сериализация layout.toml: {e}"),
        }
    }
}
