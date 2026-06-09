//! Описание одного ядра MoonBot (сервера) и группы.

use serde::{Deserialize, Serialize};

use super::secrets::Secret;

/// Флаги приёма данных от ядра — чисто клиентский фильтр.
///
/// ВАЖНО: ядро всё равно шлёт эти доменные события всегда. Сброшенный флаг
/// означает «не читаем / не складываем / не рисуем» (экономим CPU, БД и окна),
/// но НЕ экономит сетевой трафик — серверного opt-out у этих категорий нет.
/// Стакан/лента сюда не входят: они chart-only и живут только при открытом окне.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FeedFlags {
    /// Открытые ордера ядра (нижний док).
    #[serde(default = "default_true")]
    pub orders: bool,
    /// Детекты / watcher-строки / chart-only / alert-fire (`DetectEvent`).
    #[serde(default = "default_true")]
    pub detects: bool,
    /// Отчёты по закрытым sell-ордерам (`ClosedSellOrderReport`) → SQLite.
    #[serde(default = "default_true")]
    pub reports: bool,
    /// Балансы и метаданные аккаунта.
    #[serde(default = "default_true")]
    pub balance: bool,
    /// Состояние стратегий (`Strat`).
    #[serde(default = "default_true")]
    pub strategies: bool,
    /// Серверный лог (`ServerLog`).
    #[serde(default = "default_true")]
    pub log: bool,
    /// Chart-алерты и chart-текст.
    #[serde(default = "default_true")]
    pub alerts: bool,
    /// Арбитраж (`Arb`).
    #[serde(default = "default_true")]
    pub arb: bool,
}

impl Default for FeedFlags {
    /// Дефолт = принимать всё (поведение как до введения флагов).
    fn default() -> Self {
        Self {
            orders: true,
            detects: true,
            reports: true,
            balance: true,
            strategies: true,
            log: true,
            alerts: true,
            arb: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Рантайм-id ядра (CoreId): позиционный, переназначается при каждой загрузке.
    /// Используется для привязки панелей/данных/БД в пределах сессии.
    pub id: u64,
    /// Стабильный идентификатор ядра. Переживает переименование и перепорядок —
    /// по нему мета из settings.toml привязывается к серверу из servers.enc.
    /// 0 = ещё не присвоен (старый файл / только что добавлен) → проставится при save.
    #[serde(default)]
    pub uid: u64,
    #[serde(default)]
    pub name: String,
    /// Активно ли ядро (галка в настройках). Неактивные не подключаются.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Рисовать ли окно/чарт ядра. Off + active = headless: тянем отчёты/детекты
    /// в БД/store без окна. Окно показываем только при active && show_window.
    #[serde(default = "default_true")]
    pub show_window: bool,
    /// Что принимаем от ядра (клиентский фильтр).
    #[serde(default)]
    pub feed: FeedFlags,
    /// Base64-ключ MoonBot. Внутри зашиты host/port/transport — отдельных полей нет.
    #[serde(default)]
    pub key: Secret,
    /// Группа = имя окна, куда попадает ядро. Цвет/иконка — на группе (GroupConfig).
    #[serde(default = "default_group")]
    pub group: String,
    /// Рынок по умолчанию (временно, до мульти-рынков на ядро).
    #[serde(default = "default_market")]
    pub market: String,
    /// Цвет сервера (RGB) — цвет детекта (используется позже).
    #[serde(default = "default_color")]
    pub color: [u8; 3],
}

pub fn default_color() -> [u8; 3] {
    [0xff, 0xb3, 0x47]
}

pub fn default_group() -> String {
    "default".to_string()
}

pub fn default_market() -> String {
    "BTCUSDT".to_string()
}

pub fn default_true() -> bool {
    true
}

impl ServerConfig {
    /// Заглушка для отображения, когда серверов в конфиге ещё нет (не подключается).
    pub fn placeholder() -> Self {
        Self {
            id: 0,
            uid: 0,
            name: "—".to_string(),
            active: true,
            show_window: true,
            feed: FeedFlags::default(),
            key: Secret::default(),
            group: default_group(),
            market: default_market(),
            color: default_color(),
        }
    }
}
