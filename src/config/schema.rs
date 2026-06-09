//! Форматы файлов конфига на диске (serde). Здесь — ТОЛЬКО структуры данных:
//! без чтения/записи (см. `store`) и без слияния с рантаймом (см. `reconcile`).
//!
//! Forward-compat: каждое новое поле помечаем `#[serde(default = …)]`, тогда старый
//! файл без него читается без ошибки (поле получает дефолт), а `version` ниже
//! позволяет один раз дослоить эти дефолты обратно на диск (см. `AppConfig::load`).

use serde::{Deserialize, Serialize};

use super::groups::GroupConfig;
use super::lang::Language;
use super::secrets::Secret;
use super::servers::{self, FeedFlags};
use crate::market::MarketDataMode;

/// Текущая версия схемы settings.toml. Поднимай на +1, когда добавил новое поле
/// и хочешь, чтобы старые файлы один раз пере-сохранились с его дефолтом.
/// v2: добавлено поле `language`. v3: добавлено `market_mode`.
pub const SCHEMA_VERSION: u32 = 3;

/// Старые файлы без поля `version` читаются как 0 → меньше SCHEMA_VERSION →
/// триггерят досейв с дослоением новых дефолтов.
pub fn default_version() -> u32 {
    0
}

/// Запись сервера в servers.enc (секрет + стабильный uid).
///
/// host/port НЕ храним: они зашиты в самом ключе MoonBot (см. `parse_key_info` в
/// feed/live.rs). Старые servers.enc с полями host/port читаются без ошибки —
/// неизвестные поля serde просто игнорирует, подключение пойдёт по ключу.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Стабильный идентификатор ядра (см. `ServerConfig::uid`). 0 в старых файлах →
    /// присваивается при первой загрузке (см. `reconcile::merge`).
    #[serde(default)]
    pub uid: u64,
    pub name: String,
    #[serde(default)]
    pub key: Secret,
}

#[derive(Default, Serialize, Deserialize)]
pub struct ServersFile {
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
}

/// По-серверная мета в settings.toml (открытая, без секретов).
/// Привязка к серверу — по `uid` (стабильно); для старых файлов без uid
/// один раз привязываемся по `name` (см. `reconcile::merge`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerMeta {
    #[serde(default)]
    pub uid: u64,
    /// Дублируется из servers.enc — для читаемости открытого файла и legacy-привязки.
    pub name: String,
    #[serde(default = "servers::default_true")]
    pub active: bool,
    #[serde(default = "servers::default_true")]
    pub show_window: bool,
    #[serde(default)]
    pub feed: FeedFlags,
    #[serde(default = "servers::default_group")]
    pub group: String,
    #[serde(default = "servers::default_market")]
    pub market: String,
    #[serde(default = "servers::default_color")]
    pub color: [u8; 3],
}

#[derive(Default, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Язык интерфейса. Отсутствует в старых файлах → serde-дефолт = системная локаль.
    #[serde(default)]
    pub language: Language,
    /// Источник рыночных данных (дедуп по провайдеру / по ядрам). Старые файлы → дефолт.
    #[serde(default)]
    pub market_mode: MarketDataMode,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    #[serde(default)]
    pub servers: Vec<ServerMeta>,
}
