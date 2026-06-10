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
    /// uid ордера (task id) — монотонен с созданием: больше = новее. Для сортировки
    /// «по созданию / новые-старые первые» в окне ордеров.
    pub uid: u64,
    /// Эмуляторный ордер (не реальный) — для фильтра и пометки «(E)».
    pub emulator: bool,
}

/// Один детект ядра (для тулбара/истории). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct DetectRow {
    /// Монотонный per-core номер (курсор ингеста в ленту детектов).
    pub seq: u64,
    /// Рынок (монета).
    pub market: String,
    /// Unix-время приёма, мс.
    pub time_ms: f64,
    /// У стратегии-источника включён звук-алерт (SoundAlert=Yes) — только такие
    /// детекты показываем кнопкой в ленте.
    pub sound_alert: bool,
    /// Сколько секунд держать кнопку (KeepAlert стратегии; дефолт 60).
    pub keep_alert_secs: u32,
    /// AddToChart у стратегии — НОМЕР чарта-вкладки (1,2,3…), куда авто-добавить
    /// график монеты. 0 = не добавлять (обычный детект-кнопка в ленте).
    pub add_to_chart: u32,
    /// KeepInChart, сек — сколько держать авто-график монеты во вкладке, прежде
    /// чем закрыть (вкладка остаётся). Дефолт 60.
    pub keep_in_chart_secs: u32,
}

/// Одна строка серверного лога ядра (`Event::ServerLog`). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct CoreLogLine {
    /// Unix-время строки, мс (из `ServerLogEvent::unix_millis`).
    pub time_ms: i64,
    pub msg: String,
}

/// Одна стратегия ядра (для окна стратегий). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct StrategyRow {
    pub id: u64,
    /// Имя стратегии (StrategyName) или fallback.
    pub name: String,
    /// Тип (вид) стратегии — человекочитаемо.
    pub kind: String,
    /// Ordinal вида (для связи со схемой при показе секций/полей).
    pub kind_ordinal: u8,
    /// Путь папки в дереве стратегий (например "test cpu/20").
    pub folder_path: String,
    /// Отмечена (checked) = запущена.
    pub checked: bool,
    pub is_short: bool,
    /// Значения полей стратегии (имя → форматированная строка) для read-only плашек.
    pub fields: Vec<(String, String)>,
}

/// Вид виджета поля схемы (из moonproto `StrategyFieldUiKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFieldUi {
    Edit,
    Checkbox,
    Combo,
    Color,
}

/// Описание одного поля схемы стратегий (декаплено от moonproto).
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    /// Имя типа ("Bool"/"Int32"/"Double"/…), для подписи/форматирования.
    /// Пока не читается: нужно этапу полного редактирования полей стратегий.
    #[allow(dead_code)]
    pub type_name: String,
    pub ui: SchemaFieldUi,
    /// Статический список значений (для Combo).
    /// Пока не читается: нужно этапу полного редактирования полей стратегий.
    #[allow(dead_code)]
    pub picklist: Vec<String>,
    /// Значение по умолчанию (форматированное), если есть в схеме.
    pub default: Option<String>,
}

/// Секция (раздел) полей одного вида стратегии (main/filters/…).
#[derive(Debug, Clone)]
pub struct SchemaSection {
    pub title: String,
    pub fields: Vec<SchemaField>,
}

/// Схема одного вида стратегии: его секции.
#[derive(Debug, Clone)]
pub struct SchemaKind {
    pub ordinal: u8,
    /// Имя вида из схемы ядра (авторитетнее хардкода strat_kind_name).
    /// Пока не читается: нужно этапу полного редактирования полей стратегий.
    #[allow(dead_code)]
    pub name: String,
    pub sections: Vec<SchemaSection>,
}

/// Полная схема стратегий ядра (все виды). Шлётся при смене revision схемы.
#[derive(Debug, Clone, Default)]
pub struct StrategySchemaModel {
    pub kinds: Vec<SchemaKind>,
}

/// Статус соединения с ядром.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Пачка новых строк серверного лога ядра (за тик дренажа событий).
    ServerLog(Vec<CoreLogLine>),
    /// Снимок стратегий ядра (шлётся при изменении сигнатуры).
    Strategies(Vec<StrategyRow>),
    /// Схема стратегий ядра (секции/поля по видам). Шлётся при смене revision.
    StrategySchema(StrategySchemaModel),
}
