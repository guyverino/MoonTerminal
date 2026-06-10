//! Связка файловых форматов (`schema`) с рантайм-`AppConfig` в обе стороны.
//!
//! Ключ привязки меты к серверу — стабильный `uid`. Для старых файлов без uid
//! один раз привязываемся по `name` и тут же проставляем свежий uid: после этого
//! переименование сервера больше НЕ теряет его галки (привязка идёт по uid).

use super::groups::GroupConfig;
use super::lang::Language;
use super::schema::{ServerEntry, ServerMeta, ServersFile, SettingsFile, SCHEMA_VERSION};
use super::servers;
use super::ServerConfig;
use crate::market::MarketDataMode;

/// Результат слияния двух файлов в рантайм.
pub struct Merged {
    pub servers: Vec<ServerConfig>,
    pub groups: Vec<GroupConfig>,
    /// Язык интерфейса из settings.toml (или системный дефолт).
    pub language: Language,
    /// Источник рыночных данных из settings.toml (или дефолт Dedup).
    pub market_mode: MarketDataMode,
    /// Отдельная чарт-вкладка на ядро (AddToChart).
    pub charts_split_by_core: bool,
    /// Писать лог в файлы logs/.
    pub log_to_file: bool,
    /// Срок хранения файлов лога (дней; 0 = хранить всё).
    pub log_retention_days: u32,
    /// Нужно пере-сохранить на диск: присвоены новые uid и/или версия схемы
    /// устарела (надо дослоить дефолты новых полей в settings.toml).
    pub dirty: bool,
}

/// servers.enc + settings.toml → рантайм-серверы. Привязка меты по uid,
/// с одноразовым fallback на имя для старых файлов без uid.
pub fn merge(sf: ServersFile, meta: SettingsFile) -> Merged {
    let mut next_uid = next_free_uid(&sf, &meta);
    let mut dirty = meta.version < SCHEMA_VERSION;
    let language = meta.language;
    let market_mode = meta.market_mode;
    let charts_split_by_core = meta.charts_split_by_core;
    let log_to_file = meta.log_to_file;
    let log_retention_days = meta.log_retention_days;

    let servers = sf
        .servers
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            // Привязка меты: по uid, иначе (старый файл) по имени.
            let m = if e.uid != 0 {
                meta.servers.iter().find(|m| m.uid == e.uid)
            } else {
                meta.servers.iter().find(|m| m.name == e.name)
            };
            // Стабильный uid: из файла либо свежий (тогда конфиг «грязный» → досейв).
            let uid = if e.uid != 0 {
                e.uid
            } else {
                dirty = true;
                let u = next_uid;
                next_uid += 1;
                u
            };
            ServerConfig {
                id: (i as u64) + 1,
                uid,
                name: e.name,
                active: m.map(|m| m.active).unwrap_or(true),
                show_window: m.map(|m| m.show_window).unwrap_or(true),
                feed: m.map(|m| m.feed).unwrap_or_default(),
                key: e.key,
                group: m
                    .map(|m| m.group.clone())
                    .unwrap_or_else(servers::default_group),
                market: m
                    .map(|m| m.market.clone())
                    .unwrap_or_else(servers::default_market),
                color: m.map(|m| m.color).unwrap_or_else(servers::default_color),
            }
        })
        .collect();

    Merged {
        servers,
        groups: meta.groups,
        language,
        market_mode,
        charts_split_by_core,
        log_to_file,
        log_retention_days,
        dirty,
    }
}

/// Рантайм-`AppConfig` → два файловых формата (для записи).
#[allow(clippy::too_many_arguments)]
pub fn split(
    servers: &[ServerConfig],
    groups: &[GroupConfig],
    language: Language,
    market_mode: MarketDataMode,
    charts_split_by_core: bool,
    log_to_file: bool,
    log_retention_days: u32,
) -> (ServersFile, SettingsFile) {
    let sf = ServersFile {
        servers: servers
            .iter()
            .map(|s| ServerEntry {
                uid: s.uid,
                name: s.name.clone(),
                key: s.key.clone(),
            })
            .collect(),
    };
    let meta = SettingsFile {
        version: SCHEMA_VERSION,
        language,
        market_mode,
        charts_split_by_core,
        log_to_file,
        log_retention_days,
        groups: groups.to_vec(),
        servers: servers
            .iter()
            .map(|s| ServerMeta {
                uid: s.uid,
                name: s.name.clone(),
                active: s.active,
                show_window: s.show_window,
                feed: s.feed,
                group: s.group.clone(),
                market: s.market.clone(),
                color: s.color,
            })
            .collect(),
    };
    (sf, meta)
}

/// Проставить стабильный uid всем серверам без него (uid == 0). Вызывается перед
/// записью, чтобы у только что добавленных в UI ядер сразу был стабильный id.
pub fn ensure_uids(servers: &mut [ServerConfig]) {
    let mut next = servers.iter().map(|s| s.uid).max().unwrap_or(0) + 1;
    for s in servers.iter_mut() {
        if s.uid == 0 {
            s.uid = next;
            next += 1;
        }
    }
}

/// Первый свободный uid с учётом обоих файлов (чтобы не выдать занятый).
fn next_free_uid(sf: &ServersFile, meta: &SettingsFile) -> u64 {
    let from_entries = sf.servers.iter().map(|e| e.uid).max().unwrap_or(0);
    let from_meta = meta.servers.iter().map(|m| m.uid).max().unwrap_or(0);
    from_entries.max(from_meta) + 1
}
