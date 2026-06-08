//! Тема оформления чарта (фон/сетка/перекрестие) — ОТДЕЛЬНЫЙ переносимый файл
//! `theme.toml` рядом с exe, чтобы темой можно было делиться (скопировал файл —
//! и оформление перенеслось). Цвета заданы в sRGB (как палитра/egui); в linear
//! их конвертируют шейдеры (см. [[srgb-shader-colors]]).

use serde::{Deserialize, Serialize};

use super::paths;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChartTheme {
    // --- График: фон и сетка ---
    /// Фон чарта (sRGB).
    pub bg: [u8; 3],
    /// Цвет линий сетки (sRGB).
    pub grid: [u8; 3],
    /// Видимость сетки 0..1 (0 — скрыть).
    pub grid_alpha: f32,

    // --- График: перекрестие ---
    /// Цвет перекрестия и ореола (sRGB).
    pub cross: [u8; 3],
    /// Прозрачность линий перекрестия 0..1.
    pub cross_alpha: f32,
    /// Полутолщина линий перекрестия, px.
    pub cross_thickness: f32,
    /// Радиус размытого ореола у курсора, px.
    pub halo_radius: f32,
    /// Яркость ореола 0..1.
    pub halo_intensity: f32,

    // --- Стакан ---
    /// Фон зоны стакана (sRGB).
    pub book_bg: [u8; 3],
    /// Цвет bid-стороны (покупки), sRGB.
    pub book_bid: [u8; 3],
    /// Цвет ask-стороны (продажи), sRGB.
    pub book_ask: [u8; 3],

    // --- Панели (egui-хром: тулбар, панель ордера, док ордеров, статус) ---
    /// Фон панелей (sRGB).
    pub panel_bg: [u8; 3],

    // --- Закрытый график (пустой контейнер без чарта) ---
    /// Фон зоны при закрытом графике (sRGB).
    pub closed_bg: [u8; 3],
}

impl Default for ChartTheme {
    fn default() -> Self {
        Self {
            bg: [0x13, 0x14, 0x16],       // --bg, как панели/тулбары
            grid: [0x17, 0x18, 0x1a],     // едва заметная сетка
            grid_alpha: 1.0,
            cross: [0xff, 0xb3, 0x47],    // --accent (янтарный)
            cross_alpha: 0.5,
            cross_thickness: 1.0,
            halo_radius: 44.0,
            halo_intensity: 0.14,
            book_bg: [0x13, 0x14, 0x16],  // как фон чарта
            book_bid: [0x2f, 0xa8, 0x5c], // --long (зелёный)
            book_ask: [0xff, 0x8e, 0x5a], // --short (оранжевый)
            panel_bg: [0x13, 0x14, 0x16], // --bg
            closed_bg: [0x1a, 0x1c, 0x1f], // --surface-1 (нейтральный контейнер)
        }
    }
}

impl ChartTheme {
    /// Прочитать theme.toml рядом с exe. Нет файла или битый → дефолт (не падаем).
    pub fn load() -> Self {
        let path = paths::theme_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(t) => t,
            Err(e) => {
                log::error!("theme.toml повреждён ({e}); беру дефолт");
                Self::default()
            }
        }
    }

    /// Записать theme.toml (открытый человекочитаемый TOML — можно делиться).
    pub fn save(&self) -> anyhow::Result<()> {
        use anyhow::Context;
        std::fs::write(paths::theme_path(), toml::to_string_pretty(self)?)
            .context("запись theme.toml")?;
        Ok(())
    }
}
