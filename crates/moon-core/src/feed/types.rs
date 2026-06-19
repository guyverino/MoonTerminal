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
    /// Абсолютный объём сделки в базовой валюте.
    pub qty: f32,
    pub side: Side,
}

/// Retained price-line source kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceLineKind {
    Last,
    Mark,
}

/// Точка retained price-line (LastPrice / MarkPrice), уже в unix ms.
#[derive(Debug, Clone, Copy)]
pub struct PricePoint {
    pub time_ms: f64,
    pub price: f32,
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

/// Точка серверной ордерной трассы для чарта, уже в unix ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrderTracePoint {
    pub time_ms: f64,
    pub price: f32,
}

/// Серверная polyline-трасса buy/sell линии ордера. Moonproto остаётся внутри
/// feed-слоя; UI получает только доменную структуру.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrderTrace {
    pub points: Vec<OrderTracePoint>,
    pub tmp_point: Option<OrderTracePoint>,
    pub stop_price: Option<f32>,
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
    /// Цена продажи (sell_price); 0 = не выставлена.
    pub sell_price: f64,
    /// Время создания ордера, unix мс (начало линии). 0 = неизвестно.
    pub create_time_ms: f64,
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
    /// Ордер терминальный (`job_is_done` у ядра) — исполнен/отменён, ждёт deferred-removal.
    /// АВТОРИТЕТНЫЙ флаг закрытия (как MoonBot `o.IsClosed`): стор помечает линию закрытой
    /// по нему СРАЗУ, пока ордер ещё в снимке, а не по исчезновению+грейс (см. ЕБАНИНА Пример 4).
    pub job_is_done: bool,

    // --- Цены линий на чарте (категория C: горизонтали по цене) ---
    // Считаются в feed-слое (live.rs) из StopSettings/buy_price/market-liq: проценты
    // приводятся к абсолютной цене ТАМ, а рендер получает готовые цены и только
    // маппит их в пиксели через shader-uniform. `None` = линия не активна.
    /// Ордер ещё не исполнен (pending) — линию входа рисуем пунктиром.
    pub pending: bool,
    /// Входная нога исполнена (позиция открыта) — гейт для стоп/трейлинг/liq линий.
    pub filled: bool,
    /// Стоп-лосс (абсолютная цена).
    pub stop_loss: Option<f64>,
    /// Трейлинг-стоп (абсолютная цена; для %-режима — оценка от входа).
    pub trailing: Option<f64>,
    /// Тейк-профит (абсолютная цена).
    pub take_profit: Option<f64>,
    /// VStop (абсолютная цена уровня).
    pub vstop: Option<f64>,
    /// Цена условия pending-ордера (BuyCondPrice).
    pub pending_cond: Option<f64>,
    /// Цена ликвидации позиции (из рынка, по стороне).
    pub liq: Option<f64>,
    /// Локальный/серверный PanicSell флаг.
    pub panic_sell: bool,
    /// Moon-shot corridor active marker.
    pub is_moon_shot: bool,
    /// Corridor price band from server, 0/NaN means absent.
    pub corridor_price_down: f32,
    pub corridor_price_up: f32,
    /// Серверная трасса buy-линии (если ядро её уже построило).
    pub buy_trace: Option<OrderTrace>,
    /// Серверная трасса sell-линии (если ядро её уже построило).
    pub sell_trace: Option<OrderTrace>,
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
    /// Имя типа ("Bool"/"Int32"/"Double"/"String"/…) из схемы ядра. Используется в UI,
    /// чтобы числовые поля не рисовались как многострочный memo (см. `is_memo_field`).
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
    Ticks {
        market: String,
        ticks: Vec<Tick>,
    },
    /// Новые точки retained price-line рынка. Только от провайдера.
    PriceLine {
        market: String,
        kind: PriceLineKind,
        points: Vec<PricePoint>,
    },
    /// Свежий снимок стакана рынка. Только от провайдера.
    OrderBook {
        market: String,
        book: OrderBook,
    },
    /// Рыночный read-model изменился. Это лёгкий пинок consumer-side pull:
    /// `SessionManager` перечитает provider snapshot для видимых графиков.
    /// Сами тики/стакан через UI-channel не едут.
    MarketDataChanged,
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
