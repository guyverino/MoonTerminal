# ТЗ: универсальный retained-history API в MoonProto

Дата: 2026-06-18.

Цель: дать MoonProto аккуратный общий API чтения retained history, пригодный для
терминала, консольных ботов, аналитики и тестов. Это не API графика и не API
renderer-а. График MoonTerminal является только одним из потребителей.

## Зачем

Сейчас терминал вынужден держать вторую long-lived RAM-историю (`TickRing`,
`PriceLineRing`) поверх истории, которая уже есть в MoonProto. Это создаёт
рассинхрон policy, лишнюю память и локальные cap-ы (`TICK_CAP`,
`PRICE_LINE_CAP`, `MARKET_PULL_BATCH`).

Правильная граница владения:

1. MoonProto владеет source history в RAM.
2. Потребители держат только cursor/version/view state и переиспользуемые
   scratch buffers.
3. GPU может держать свою resident projection/cache в VRAM, но source of truth
   остаётся MoonProto.

## Не цель

- Не делать MoonProto зависимым от UI, GPUI, GPU, chartdx или MoonTerminal.
- Не добавлять в MoonProto `ChartCross`, `PriceLinePoint`, screen pixels,
  viewport, color, scale или renderer-specific типы.
- Не переносить политику GPU combo в MoonProto.
- Не заставлять консольных ботов платить за графический API.

## API-смыслы

Нужны универсальные primitives для dense retained rings:

1. **Tail read**  
   Прочитать последние `limit` записей в последовательном порядке.

2. **Time/range read**  
   Прочитать записи в диапазоне domain-time `[from, to]` с лимитом результата.
   Для trade/price/candle rows domain-time берётся из row timestamp.

3. **Cursor drain**  
   Прочитать новые записи после per-consumer cursor и продвинуть cursor ровно на
   реально скопированное количество.

4. **Bounded drain status**  
   Результат drain должен честно говорить:
   - `copied` — сколько записей скопировано;
   - `clipped` — cursor был старше retention и чтение началось с oldest;
   - `caught_up` — потребитель догнал `next_seq`, или надо вызвать drain ещё;
   - `concurrent_miss` — reserved-флаг для backend-ов без полной блокировки.

5. **Range scan**  
   Возможность просканировать retained range без построения второй истории у
   потребителя. Минимально нужен price min/max для rows с ценой; лучше сделать
   это как generic scan/visitor, чтобы API не был chart-specific.

6. **Capacity/policy visibility**  
   Потребитель должен иметь возможность узнать effective capacity retained
   history для конкретного stream-а/market-а. Это нужно не для владения policy, а
   для согласования внешних cache-ов, например GPU resident ring.

## Предлагаемые типы

Имена можно поменять при реализации, важен контракт.

```rust
pub struct SeqRingDrainMeta {
    pub copied: usize,
    pub clipped: bool,
    pub caught_up: bool,
    pub concurrent_miss: bool,
}
```

```rust
impl<T: SeqRingRow> SeqRingReader<T> {
    pub fn drain_new_bounded(
        &self,
        cursor: &mut SeqRingCursor,
        limit: usize,
        out: &mut Vec<T>,
    ) -> SeqRingDrainMeta;
}
```

Для rows с domain-time:

```rust
pub trait SeqRingTimedRow: SeqRingRow {
    fn seq_ring_time_ms(&self) -> i64;
}

impl<T: SeqRingTimedRow> SeqRingReader<T> {
    pub fn copy_time_range(
        &self,
        from_time: MoonTime,
        to_time: MoonTime,
        limit: usize,
        out: &mut Vec<T>,
    ) -> SeqRingReadMeta;

    pub fn copy_time_range_ms(
        &self,
        from_ms: i64,
        to_ms: i64,
        limit: usize,
        out: &mut Vec<T>,
    ) -> SeqRingReadMeta;
}
```

Для scan лучше предпочесть generic visitor, чтобы не пришить MoonProto к
графику:

```rust
impl<T: SeqRingRow> SeqRingReader<T> {
    pub fn scan_from_cursor<R, F>(
        &self,
        cursor: SeqRingCursor,
        limit: usize,
        init: R,
        f: F,
    ) -> (R, SeqRingReadMeta)
    where
        F: FnMut(R, &T) -> R;
}
```

### Контракт `scan_from_cursor`

`scan_from_cursor(...)` даёт потребителю не копию массива, а временный доступ к
внутренним строкам retained ring. Поэтому реализация держит read-lock ring-а на
всё время выполнения user closure. Это нормально для коротких агрегатов без
аллокаций, но является footgun-ом, если контракт не записан в rustdoc.

В rustdoc к `scan_from_cursor` нужно явно написать:

- closure выполняется под внутренним read-lock конкретного retained-history ring;
- closure must be short and non-blocking;
- внутри closure нельзя делать UI render, логирование в hot path, sleep, сеть,
  ожидания, доступ к чужому client state или попытки взять другие потенциально
  спорные lock-и;
- `limit` ограничивает число строк, но не спасает от тяжёлой работы внутри
  closure.

Хороший паттерн:

```rust
let (range, meta) = reader.price_range_from_cursor(cursor, 10_000);

// Для редкого custom aggregate допустим короткий scan без I/O/рендера/ожиданий.
let (count, meta) = reader.scan_from_cursor(cursor, 10_000, 0usize, |count, _row| {
    count + 1
});
```

Плохой паттерн:

```rust
reader.scan_from_cursor(cursor, 10_000, (), |_, row| {
    ui.draw(row);
    client.snapshot();
    std::thread::sleep(std::time::Duration::from_millis(1));
});
```

Сам API полезный: он позволяет делать min/max/sum без второй истории и без
аллокаций. Опасность не в том, что есть read-lock, а в том, что произвольный
user callback легко случайно превращает короткий scan в долгий stop-the-writer
для конкретного retained ring.

### Safe aggregate API поверх scan

Чтобы обычный код реже использовал произвольный closure под read-lock, MoonProto
должен дать готовые aggregate/query helpers, реализованные внутри библиотеки.
Имена не принципиальны, важен смысл:

```rust
pub struct PriceRange {
    pub min: f32,
    pub max: f32,
    pub count: usize,
}

impl<T: SeqRingPriceRow> SeqRingReader<T> {
    pub fn price_range_from_cursor(
        &self,
        cursor: SeqRingCursor,
        limit: usize,
    ) -> (Option<PriceRange>, SeqRingReadMeta);

    pub fn price_range_time_ms(
        &self,
        from_ms: i64,
        to_ms: i64,
        limit: usize,
    ) -> (Option<PriceRange>, SeqRingReadMeta);
}
```

Для объёмов/количеств аналогично:

```rust
pub struct QtySum {
    pub sum: f64,
    pub count: usize,
}

impl<T: SeqRingQtyRow> SeqRingReader<T> {
    pub fn qty_sum_time_ms(
        &self,
        from_ms: i64,
        to_ms: i64,
        limit: usize,
    ) -> (QtySum, SeqRingReadMeta);
}
```

Если generic trait-ы `SeqRingTimedPriceRow` / `SeqRingTimedQtyRow` получаются
слишком тяжёлыми, допустимы typed helpers на уровне history-store конкретного
стрима: trades price range, candles OHLC range, price-line range. Главное:
агрегат считается внутри MoonProto коротким tight-loop-ом, наружу возвращается
готовый результат, а не произвольная пользовательская работа под lock.

Для тяжёлой пользовательской обработки остаётся owned-copy путь:
`copy_time_range(...)` / `drain_new_bounded(...)` в caller-provided
buffer. После копии пользователь может рендерить, логировать и ходить в другие
state-и без удержания lock-а MoonProto.

Если generic scan получится громоздким, допустим отдельный typed helper в
history-store layer, но он всё равно должен быть phrased as retained-history
query, а не chart helper.

### Diagnostics: full-capacity retained fixture

Для проверки GPU combo и хвоста retained history нужен debug-only метод в
MoonProto, доступный только под `feature = "diagnostics"`:

```rust
client.diag_fill_market_history_to_capacity(
    market_name,
    now_ms,
    moonproto::client::DIAG_MARKET_HISTORY_FILL_SPAN_MS, // 3_600_000
)?;
```

Смысл метода не "добавь N трейдов", а "сделай для market полный
синтетический retained-history стенд":

- MoonProto сам берёт effective capacity каждого retained ring из текущей
  memory policy;
- futures trades, spot trades, liquidations, MM orders + companion,
  LastPrice, MarkPrice, mini-candles и 5m candles заполняются каждый до своего
  capacity;
- если в ring уже были live rows, они остаются самым новым хвостом, а
  синтетика генерируется хронологически перед самым старым существующим row;
- если ring пустой, синтетика распределяется по `span_ms` до `now_ms`;
- после возврата `len == capacity`, `oldest_seq/next_seq/revision` корректны,
  а обычный `drain_new_bounded` видит эти rows как реальные.

Это стенд для терминала: он читает полный cap из MoonProto, загружает GPU cache,
потом новые live rows двигают голову и вытесняют хвост обычной политикой
MoonProto. В продовом build метода нет.

## Memory policy

MoonProto остаётся единственным местом предметной policy по объёму истории.

Нужно добавить пользовательский множитель budget-а:

- default: `100`;
- clamp: `100..800`;
- смысл как Delphi `UseMemForCharts` / `UseMemForTrades`;
- применяется к base memory budget от физической RAM до раскладки по
  trade/price/candle/MM capacities.

Терминал передаёт значение настройки в `ClientConfig::with_market_history(...)`.
Другие потребители могут оставить default.

## Контракт для MoonTerminal

Терминал после этого:

1. Не держит long-lived `TickRing` / `PriceLineRing` поверх MoonProto history.
2. На reset/provider change/device lost читает tail/time range из MoonProto.
3. На live читает новые rows через bounded cursor drain.
4. Если `clipped = true`, делает full range/tail read.
5. Если `caught_up = false`, имеет право продолжить drain без локального
   `MARKET_PULL_BATCH`.
6. Конвертирует source rows в GPU structs только в scratch/GPU upload path.

## Проверка готовности

- В MoonProto есть публичный bounded drain без `cfg(test/diagnostics)` доступа к
  приватным seq-полям.
- Cursor после drain двигается на фактически скопированные rows.
- `caught_up=false` воспроизводится тестом, когда `limit` меньше backlog.
- `clipped=true` воспроизводится тестом, когда cursor старше retained oldest.
- Time-range read возвращает rows в sequence order.
- Rustdoc `scan_from_cursor` явно говорит, что closure выполняется под
  внутренним read-lock и должна быть короткой/non-blocking.
- Для типовых запросов есть готовый aggregate API (`price_range_*`,
  `qty_sum_*` или typed equivalents), чтобы потребители не тащили тяжёлую
  работу в произвольный closure.
- Под `feature = "diagnostics"` есть full-capacity fixture hook, который
  заполняет retained rings до текущих effective capacities без отдельной
  fake-history логики в терминале.
- API не импортирует и не упоминает MoonTerminal, GPUI, chartdx, GPU-типы.
