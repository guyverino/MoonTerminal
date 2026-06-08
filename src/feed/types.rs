//! Доменные типы, которыми backend кормит UI. Не зависят от moonproto,
//! чтобы UI/render-слой ничего не знал о транспорте.

/// Сторона сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Идентификатор биржи ядра — байт `ExchangeCode` из moonproto (спот/фьючи — РАЗНЫЕ
/// коды: Binance=3, FBinance=4, ByBit=7, FBybit=2 …). Ключ дедупа рыночных данных:
/// ядра с одинаковым `ExchangeId` видят идентичный рынок. Держим как голый байт,
/// чтобы доменные типы оставались независимыми от moonproto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExchangeId(pub u8);

/// Один тик (сделка) — семантическая точка графика.
#[derive(Debug, Clone, Copy)]
pub struct Tick {
    /// Unix-время в миллисекундах (из core: row.unix_millis()).
    pub time_ms: f64,
    pub price: f32,
    pub qty: f32,
    pub side: Side,
}

/// Уровень стакана.
#[derive(Debug, Clone, Copy)]
pub struct Level {
    pub price: f32,
    pub qty: f32,
}

/// Снимок верхушки стакана (bids/asks).
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    /// Биды — по убыванию цены.
    pub bids: Vec<Level>,
    /// Аски — по возрастанию цены.
    pub asks: Vec<Level>,
}

/// Открытый ордер (для нижнего дока).
#[derive(Debug, Clone)]
pub struct OrderRow {
    /// Имя рынка (монета).
    pub market: String,
    /// true = Short, false = Long.
    pub is_short: bool,
    /// Размер входной ноги (buy для long / sell для short), в базовой валюте.
    pub size: f64,
    pub sl_on: bool,
    pub ts_on: bool,
    pub vstop_on: bool,
    /// Цена входа (buy_price).
    pub buy_price: f64,
    /// Текущая цена рынка (p_last).
    pub price: f32,
    /// Заполнение входной ноги, %.
    pub fill_pct: f32,
    /// Имя/тип стратегии ордера (вместо числового strat_id).
    pub strat: String,
}

/// Один детект ядра (для тулбара/истории). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct DetectRow {
    /// Монотонный per-core номер (курсор ингеста в ленту детектов).
    pub seq: u64,
    /// Рынок (монета).
    pub market: String,
    /// Id стратегии-источника.
    pub strategy_id: u64,
    pub is_short: bool,
    /// Биты вида: regular / watcher-row / chart-only / alert-fire.
    pub kind_bits: u8,
    /// Текст детекта.
    pub msg: String,
    /// Unix-время приёма, мс.
    pub time_ms: f64,
    /// У стратегии-источника включён звук-алерт (SoundAlert=Yes) — только такие
    /// детекты показываем кнопкой в ленте.
    pub sound_alert: bool,
    /// Сколько секунд держать кнопку (KeepAlert стратегии; дефолт 60).
    pub keep_alert_secs: u32,
}

/// Одна стратегия ядра (для будущего окна стратегий). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct StrategyRow {
    pub id: u64,
    /// Имя стратегии (StrategyName) или fallback.
    pub name: String,
    /// Тип (вид) стратегии — человекочитаемо.
    pub kind: String,
    /// Отмечена (checked) в дереве стратегий.
    pub checked: bool,
    pub is_short: bool,
    /// SoundAlert=Yes.
    pub sound_alert: bool,
    /// KeepAlert, сек.
    pub keep_alert_secs: u32,
}

/// Статус соединения с ядром.
#[derive(Debug, Clone)]
pub enum ConnStatus {
    Connecting,
    /// Промежуточная стадия подключения/инициализации (текст для бейджа).
    Stage(String),
    Ready,
    Failed(String),
    Disconnected,
}

/// Сообщение от backend к UI.
///
/// Делится на два плана. Аккаунтные (Status/Orders/Detects/Strategies) — свои у
/// каждого ядра. Рыночные (Ticks/OrderBook) — общие для биржи, шлёт только
/// ядро-провайдер, и они помечены именем рынка. Identity сообщает биржу ядра.
#[derive(Debug, Clone)]
pub enum FeedMsg {
    Status(ConnStatus),
    /// Биржа ядра (из server_info после BaseCheck). Шлётся один раз.
    Identity(ExchangeId),
    /// Пачка новых тиков рынка (append-only по времени). Только от провайдера.
    Ticks { market: String, ticks: Vec<Tick> },
    /// Свежий снимок стакана рынка. Только от провайдера.
    OrderBook { market: String, book: OrderBook },
    /// Открытые ордера ядра (все рынки).
    Orders(Vec<OrderRow>),
    /// Пачка новых детектов (накопленных за тик дренажа событий).
    Detects(Vec<DetectRow>),
    /// Снимок стратегий ядра (троттлится; под будущее окно стратегий).
    Strategies(Vec<StrategyRow>),
}
