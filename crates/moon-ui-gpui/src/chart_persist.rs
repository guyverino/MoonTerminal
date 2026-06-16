//! Персист чарт-вкладок (порт идеи `detached.rs`, но для чарт-вкладок — у них своя
//! сериализация). Хранит ПО ВКЛАДКЕ (ключ = группа/номер/ядро): масштаб цены и, если вкладка
//! откреплена в своё ОС-окно, геометрию этого окна. Файл `charts.json` рядом с exe.
//!
//! На старте откреп-вкладки восстанавливаются ПУСТЫМИ (только брендовое лого) в том же месте и
//! ждут детект — `ChartTabs::ingest` наполнит их по (номер, ядро), как обычные AddToChart.
//! Положение/зум самого чарта НЕ персистим: при загрузке вкладка пуста (нечего восстанавливать),
//! а появившиеся монеты идут в live-follow.

use moon_core::config::paths;
use moon_core::session::CoreId;
use serde::{Deserialize, Serialize};

/// Геометрия окна откреплённой вкладки.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WinGeom {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Состояние одной чарт-вкладки. `num == 0` — Main; `num >= 1` — AddToChart-N.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChartTabSpec {
    pub group: String,
    pub num: u32,
    #[serde(default)]
    pub core: Option<CoreId>,
    #[serde(default)]
    pub scale: Option<f32>,
    /// Some → вкладка откреплена в своё окно с этой геометрией; None → во вкладочном стрипе.
    #[serde(default)]
    pub detached: Option<WinGeom>,
}

/// Загрузить из `charts.json` (нет/битый → пусто).
pub fn load_all() -> Vec<ChartTabSpec> {
    match std::fs::read_to_string(paths::charts_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("charts.json битый ({e}) → без сохранённых вкладок");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Записать в `charts.json` (не фатально).
pub fn save_all(list: &[ChartTabSpec]) {
    moon_core::detect_diag::line(&format!(
        "[save] charts.json: {} спек, detached(окна)={}",
        list.len(),
        list.iter().filter(|s| s.detached.is_some()).count()
    ));
    match serde_json::to_string_pretty(list) {
        Ok(s) => {
            if let Err(e) = std::fs::write(paths::charts_path(), s) {
                log::warn!("не записал charts.json: {e}");
            }
        }
        Err(e) => log::warn!("не сериализовал charts.json: {e}"),
    }
}
