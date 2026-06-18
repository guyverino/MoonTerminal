# Аудит cap/limit: MoonTerminal vs MoonProto vs Delphi MoonBot

Дата: 2026-06-18.

Цель: найти все важные cap/limit вокруг графика, понять что они реально
ограничивают, и зафиксировать одно правильное решение. Delphi-доки использовались
только как карта. Delphi-соответствие ниже считается доказанным только если есть
ссылка на код `X:\proj-X\MoonBot\src`.

## Базовая модель

Правильная модель памяти выводится из трёх простых правил.

1. Сколько хранить исторических рыночных данных в RAM — это продуктовая policy,
   зависящая от физической RAM и желания пользователя. Единственная допустимая
   магия здесь — одна формула/таблица базового бюджета от RAM. Поверх неё нужен
   пользовательский множитель памяти графиков, как Delphi `UseMemForCharts`
   (`UseMemForTrades`) с clamp-ом.

2. Одинаковые исторические данные не должны жить в двух RAM-хранилищах. Если
   MoonProto уже держит retained history, терминал не должен держать такую же
   long-lived копию под другим cap-ом. Разрешены cursor-ы, версии, маленькое
   состояние вида и переиспользуемые scratch-буферы кадра.

3. VRAM — исключение, потому что GPU не может бесплатно рисовать из RAM. GPU
   combo — это не новая история, а resident ring-кэш/проекция MoonProto history
   в GPU-формате. В live он должен работать так же просто, как source ring:
   MoonProto получил новый узел, chart дочитал его cursor-ом, GPU ring записал
   его в следующий слот. Отличие только в байтах: MoonProto хранит source rows,
   GPU хранит готовые draw-instance-ы.

## Решение сверху

1. Историей рынка владеет MoonProto. Его `MarketHistoryConfig` должен стать
   единственным источником предметных лимитов: trades, last/mark price, candles,
   MM. В эту policy надо добавить пользовательский множитель памяти графиков.

2. Терминальный `MarketView` с `TickRing`/`PriceLineRing` сейчас является
   вторым RAM-хранилищем той же retained history. Это архитектурный долг. Цель:
   убрать long-lived дубль и заменить его общим retained-history reader API из
   MoonProto плюс переиспользуемые frame scratch buffers. Не запрещается короткая
   копия/конвертация в scratch перед GPU upload; запрещается вторая
   долгоживущая история в RAM.

3. MoonProto — общая библиотека для терминала, консольных ботов и других
   потребителей, а не "движок графики". Поэтому API надо формулировать не как
   chart-specific helper, а как универсальные retained-history primitives:
   read visible/time range, read tail, price/range scan, bounded drain by cursor
   с результатом `copied/clipped/caught_up`. График терминала просто один из
   потребителей этого API.

4. `MARKET_PULL_BATCH = 8192` удалить. Это не product-функция, а искусственный
   cap одной порции drain-а, который может оставить retained хвост недочитанным.
   Правильное место для bounded drain — MoonProto retained-history API:
   он владеет cursor/seq/clipped/caught_up semantics, а терминал только
   вызывает этот API и реагирует на результат. В текущем коде это уже закрыто:
   локальной `MARKET_PULL_BATCH` в терминале нет.

5. GPU combo оставить как backend ring-кэш MoonProto history. Он как раз нужен,
   чтобы не гонять одну и ту же историю RAM→VRAM каждый кадр. Исправлять надо не
   саму идею combo, а левую самостоятельную capacity policy:
   `COMBO_RING_CAP` / `COMBO_HISTORY_CAP` не должны быть отдельными числами
   renderer-а. Capacity GPU combo должна соответствовать MoonProto history
   capacity для этого stream-а/market-а, а DX/Metal/wgpu должны иметь одну
   одинаковую ring-модель append/reset.

6. `MAX_WINDOW_MS = 1h` заменить на Delphi parity: `21_600_000 ms` (6 часов).

7. Для стакана строить draw-instance-ы только для уровней, которые реально
   попадают в видимое ценовое окно панели. Если в RAM 9000 уровней, а уровни
   начиная с 5000-го выше/ниже видимой области графика, они не должны попадать
   в GPU buffer вообще. Никакого отдельного cap-а "по высоте экрана" и никакого
   pixel-binning как обязательной политики: видимый уровень рисуем, невидимый
   отсекаем до построения `LevelInstance`.
   
8. `INITIAL_*_CAP` для стакана/readout/userdata — не лимиты данных. Это стартовая
   ёмкость массива draw-instance-ов перед отправкой в GPU. Оставить их как
   стартовые capacity и переименовать в `INITIAL_*_BUFFER_CAPACITY`, чтобы не
   путать с cap-ом рынка/стакана/ордеров.

9. В Delphi нет рабочего cap-а "5000 уровней стакана". Код содержит
   `n := Min(5000, N)`, но потом выделяет и копирует `N`, не `n`. Использовать
   это как эталон нельзя.

## Почему текущий CPU render-cache плох как архитектура

Сейчас терминал не рисует напрямую из MoonProto. Он копирует retained history в
свой `MarketView`:

- `R:\test\MoonTerminal\crates\moon-core\src\market\mod.rs:75-86` —
  `MarketView` содержит `TickRing`, `PriceLineRing`, `OrderBookModel`.
- `R:\test\MoonTerminal\crates\moon-core\src\market\mod.rs:91-93` —
  `TickRing::new(..., TICK_CAP)`, `PriceLineRing::new(..., PRICE_LINE_CAP)`.

Эта копия реально используется:

- `TickRing::visible_range` делает бинарный поиск видимых тиков:
  `R:\test\MoonTerminal\crates\moon-core\src\data\tick_ring.rs:90-99`.
- `TickRing::price_range_in` считает auto-Y по видимому окну:
  `tick_ring.rs:124-142`.
- `chartdx` использует это при подготовке pane:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\mod.rs:780-789`,
  `933-977`.

Итого: текущая копия не случайная — она закрывает реальные операции графика.
Но как целевая архитектура это неправильно: одни и те же trades/price живут и
в MoonProto, и в терминальном `MarketView`.

Правильное исправление:

1. Вынести операции чтения retained history в общий MoonProto/source API:
   tail read для resident данных, time-range/range scan для видимого анализа,
   bounded cursor drain status. Это не
   graphics API; график использует его как один из потребителей.
2. В терминале оставить cursor/version/view state и переиспользуемые scratch
   vectors кадра.
3. В GPU держать resident buffers только под текущую визуальную задачу.

## Delphi: реальная политика истории

В Delphi нет одной константы "сколько истории держать". Размер вычисляется от
RAM, режима VDS, платформы и `UseMemForTrades`.

База `MaxDataLen` от RAM:

- `X:\proj-X\MoonBot\src\Unit1.pas:6299-6303`
- `<3 GB -> 1200`
- `<4 GB -> 1600`
- `<6 GB -> 2400`
- `<9 GB -> 4000`
- `<18 GB -> 5200`
- `>=18 GB -> 6200`

Потом база меняется платформой:

- Gate: VDS `1600`, иначе `round(MaxDataLen * 1.5)`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6312-6316`.
- Не Gate: VDS `2000`, иначе `round(MaxDataLen * 2.2) * 2`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6318-6322`.

Price/candles:

- `MaxDataLenCandles := MaxDataLen`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6326`.
- non-VDS при `TotalMemGB > 4`: `MaxDataLenCandles` масштабируется через
  `UseMemForTrades`, но не выше `25000`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6355-6357`.

Trades/orders history:

- базово `MaxDataLenOrdersH := Min(MaxDataLen * 3, 44000)`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6329`.
- QBinance не market-server: `Min(MaxDataLen * 4, 58000)`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6330-6331`.
- FBinance не market-server: `Min(MaxDataLen * 3, 48000)`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6332-6333`.
- Gate не market-server: `Min(MaxDataLen * 2, 28000)`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6334-6335`.
- VDS режет историю: FBinance до `4800`, Gate до `3200`, остальные до `4200`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6337-6342`.
- non-VDS при `TotalMemGB > 4`: `MaxDataLenOrdersH` масштабируется через
  `UseMemForTrades`, но не выше `98000`.
  Код: `X:\proj-X\MoonBot\src\Unit1.pas:6355-6356`.

`UseMemForTrades`:

- default `100`, читается из INI как `UseMemForCharts`.
  Код: `X:\proj-X\MoonBot\src\Vars.pas:1549`,
  `X:\proj-X\MoonBot\src\Vars.pas:2267`.
- clamp `100..800`.
  Код: `X:\proj-X\MoonBot\src\Vars.pas:2272`.

`ResizeOrdersHistory` не задаёт размер истории. Он чистит старую голову, когда
count перелез порог:

- порог: `OrdersHCount > round(MaxDataLenOrdersH * MaxDataLenCoeffOrdersH)`;
  код: `X:\proj-X\MoonBot\src\MarketsU.pas:8697`.
- удаление головы через `move(OrdersH[n], OrdersH[0], ...)`;
  код: `X:\proj-X\MoonBot\src\MarketsU.pas:8732-8733`.
- коэффициент `0.8/0.85/0.9/0.95` зависит от размера:
  `X:\proj-X\MoonBot\src\Unit1.pas:6361-6367`.

Вывод для терминала: Delphi подтверждает не конкретную цифру `200000`, а подход:
одна memory/platform policy, а не россыпь независимых чисел.

## MoonProto: источник предметной policy

Отдельное ТЗ для разработчика MoonProto:
`R:\test\MoonTerminal\docs\MOONPROTO_RETAINED_HISTORY_TZ.md`.

MoonProto уже содержит memory-based policy:

- `MarketHistoryConfig`:
  `C:\Users\Mike\.cargo\git\checkouts\moonprotobeta-20f0f9a560480e03\a4ce5ec\src\state\history_store\config.rs:65-76`.
- default:
  futures trades `10_000`, spot `5_000`, liquidation `2_000`,
  MM orders `2_000`, last price `5_000`, mini/candles `5_000`.
  Код: `config.rs:110-120`.
- auto sizing:
  `config.rs:124-169`.
- auto max:
  futures trades `200_000`, spot `150_000`, liquidation `50_000`,
  MM orders `50_000`, last price `80_000`, mini candles `50_000`,
  5m candles `20_000`.
  Код: `config.rs:136-158`.
- budget: `<8 GiB -> total/4`, иначе `total/5`.
  Код: `config.rs:171-177`.

Это и есть единственное место допустимой магии: какую долю RAM отдаём под
рыночную историю. Все остальные capacity должны быть производными от этого
budget-а и пользовательского множителя, а не самостоятельными числами в
терминале или renderer-е.

Чего не хватает относительно Delphi-подхода:

- пользовательского множителя memory budget для графиков. В Delphi это
  `UseMemForCharts` / `UseMemForTrades`, default `100`, clamp `100..800`.
  В терминале нужен такой же смысл: base budget от RAM умножается на настройку
  пользователя, и только потом раскладывается по trade/price/candle/MM history.

`SeqRing`:

- `copy_new_since` двигает cursor на реально скопированные строки:
  `C:\Users\Mike\.cargo\git\checkouts\moonprotobeta-20f0f9a560480e03\a4ce5ec\src\state\seq_ring.rs:335-343`.
- `SeqRingReadMeta.next_seq` и `SeqRingBounds.next_seq` сейчас публичны только
  под `test/diagnostics`:
  `seq_ring.rs:53-67`, `seq_ring.rs:38-49`.

Практический вывод: MoonProto должен открыть универсальный bounded cursor-drain
API, пригодный не только для UI:

`SeqRingReader::drain_new_bounded(cursor, limit, out) -> SeqRingDrainMeta`.

`SeqRingDrainMeta` должен содержать `copied`, `clipped`, `caught_up`. Любой
потребитель — терминал, консольный бот, аналитика — может понять, догнал ли он
retained history или нужен повторный drain/full range read. Терминал не должен
лезть к приватным полям и не должен строить костыль вокруг `8192`.

## Конкретные cap-ы терминала

### `TICK_CAP = 200_000`

Код:

- `R:\test\MoonTerminal\crates\moon-core\src\market\mod.rs:28`.
- используется в `TickRing::new`:
  `R:\test\MoonTerminal\crates\moon-core\src\market\mod.rs:91`.

Факт:

Это capacity второго RAM-хранилища trades внутри терминала. Цифра совпадает с
MoonProto auto max для futures trades (`200_000`), но это плохое совпадение:
одни и те же trades уже хранятся в MoonProto retained history.

Решение:

Удалить отдельную константу `TICK_CAP` и убрать long-lived terminal
`TickRing` как вторую историю. График должен получать trades из MoonProto через
универсальный retained-history reader API:

- tail read нужен для resident GPU cache;
- time-range query/scan нужен только для auto-Y, layout и другой аналитики
  видимой области;
- bounded drain для live append с результатом `copied/clipped/caught_up`.

### `PRICE_LINE_CAP = 200_000`

Код:

- `R:\test\MoonTerminal\crates\moon-core\src\market\mod.rs:29`.
- используется в `PriceLineRing::new`:
  `R:\test\MoonTerminal\crates\moon-core\src\market\mod.rs:92-93`.

Факт:

Это capacity второго RAM-хранилища last/mark price внутри терминала. Она не
совпадает с MoonProto: MoonProto auto max для last price — `80_000`
(`config.rs:153-154`).

Решение:

Удалить отдельную константу `PRICE_LINE_CAP` и убрать long-lived terminal
`PriceLineRing` как вторую историю. Last/mark line должны читаться из MoonProto
source history в scratch/GPU resident buffer. Last и mark используют одну
MoonProto price-history policy (`config.rs:185-186`), без терминального
магического cap-а.

### `MARKET_PULL_BATCH = 8192`

Код:

- `R:\test\MoonTerminal\crates\moon-core\src\market\source.rs:14`.
- применяется к trades/last/mark:
  `source.rs:261`, `source.rs:279`, `source.rs:295`.

Зачем это появилось:

Это попытка не копировать слишком много retained history за один `refresh_market`.
То есть это не функция продукта, а ограничитель работы на кадре.

Почему текущая форма плохая:

- initial retained history может быть больше `8192`;
- после одного pull-а `chartdx` записывает `last_source_market_sig`:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\mod.rs:618-620`;
- следующий кадр может выйти через fast-skip, если `snapshot_revision` не
  изменился:
  `chartdx\mod.rs:604-607`;
- значит старый retained хвост может остаться недочитанным.

Решение:

Удалить `MARKET_PULL_BATCH`.

Новый алгоритм:

1. При создании chart / смене provider / смене market:
   - запросить из MoonProto tail до effective capacity для resident GPU combo;
   - отдельно читать visible time range только для auto-Y/аналитики вида;
   - загрузить resident tail в GPU buffers;
   - cursor после initial fill ставить на now через `cursor_from_now`.
2. На live incremental кадрах:
   - bounded drain из MoonProto в scratch buffer;
   - append в GPU resident buffers;
   - если `clipped = true`, сделать full visible/tail reupload из MoonProto.
3. Добавить публичный MoonProto bounded-drain API с результатом
   `copied/clipped/caught_up`, чтобы terminal мог проверить догонку без доступа
   к приватным `SeqRing` полям.

Так нет отдельного batch-cap-а, и нет медленной догонки retained history через
случайные present ticks.

### `COMBO_RING_CAP = 1 << 17` / `COMBO_HISTORY_CAP = 1 << 17`

Код:

- DX:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\combo.rs:25`.
- DX обрезает reset/append/price-lines:
  `combo.rs:397-418`, `combo.rs:584-592`.
- Metal:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\metal_backend.rs:23`,
  `metal_backend.rs:284-303`.
- wgpu:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\wgpu_backend.rs:27`,
  `wgpu_backend.rs:334-353`.

Факт:

Это capacity GPU combo ring. Идея ring-а правильная: combo должен держать в
VRAM тот же логический хвост истории, который MoonProto держит в RAM, но уже в
GPU draw-формате. Если capacity = `1000`, и приходит 1001-й узел, MoonProto ring
дешево перезаписывает старый source slot, а GPU combo ring должен так же дешево
перезаписать старый GPU slot.

Проблема не в combo как таковом, а в том, что `1 << 17` — самостоятельная левая
policy renderer-а. Сейчас он меньше terminal trades CPU cache (`131072` против
`200000`) и больше MoonProto price auto max (`131072` против `80000`). То есть
один и тот же смысл разошёлся по разным цифрам.

По целевой модели MoonProto остаётся источником истории, а GPU combo — его
resident projection/cache. Это не те же байты и не тот же объект: MoonProto
хранит source rows, GPU хранит `ChartCross`/`PriceLinePoint` и прочие
draw-instance-ы. Но capacity и поведение хвоста должны быть согласованы с
MoonProto, а не придуманы заново.

Текущий backend status:

- DX уже близок к правильной ring-модели для crosses: есть `head`, `count`,
  `ring_write_no_overwrite` и incremental bake новых тиков.
- DX price lines пока заливаются отдельным массивом, не полноценным ring-append.
- Metal/wgpu сейчас хуже: `Vec + drain + full buffer write`, то есть не такая же
  дешёвая ring-модель, как DX/MoonProto.

Решение:

Удалить `COMBO_RING_CAP` и `COMBO_HISTORY_CAP` именно как самостоятельные
renderer policy. Заменить их на единую `combo_capacity`, полученную из
MoonProto/source history policy для соответствующего stream-а/market-а.

Live path должен быть простым:

1. MoonProto ring append source row.
2. Chart cursor дочитывает новые rows.
3. Terminal конвертирует rows в GPU draw-instance-ы в scratch buffer.
4. GPU combo ring пишет append в `head`.
5. `head/count` обновляются; старый хвост вытесняется тем же правилом capacity.

Если пользователь ушёл pan/zoom в диапазон, которого уже нет в resident GPU
ring, это обычный cache miss: GPU combo reset/reupload из MoonProto visible/tail
range. Это не отдельная history policy, а восстановление GPU-кэша из source of
truth.

DX/Metal/wgpu должны использовать одну и ту же модель: ring capacity от
MoonProto policy, cheap append по live-данным, reset/reupload только на смену
source/range/device или cache miss.

### `DEFAULT_WINDOW_MS = 60_000`

Код:

- `R:\test\MoonTerminal\crates\moon-chart\src\view.rs:39`.
- phase-clean formula:
  `view.rs:118-128`.

Delphi:

- `DefaultTimeRange = 60`;
  `X:\proj-X\MoonBot\src\Vars.pas:539`.
- обычный zoom-in не уходит ниже 60 секунд:
  `X:\proj-X\MoonBot\src\ChartFrameUnit.pas:3482-3484`.

Решение:

Оставить. У нас это не строго 60 секунд, а фазо-чистое окно рядом с 60 секундами.
Это соответствует ранее принятому правилу плавного live-scroll.

### `MAX_WINDOW_MS = 3_600_000`

Код:

- `R:\test\MoonTerminal\crates\moon-chart\src\view.rs:37`.
- clamp zoom-out:
  `view.rs:266-274`.

Delphi:

- `DefaultTimeRange = 60`, `MaxTimeRange = 360`;
  `X:\proj-X\MoonBot\src\Vars.pas:539-540`.
- zoom-out clamp:
  `X:\proj-X\MoonBot\src\ChartFrameUnit.pas:3484`.
- другие clamp-места:
  `ChartFrameUnit.pas:1697`, `4586`, `4640`, `5767`.

Решение:

Исправить на `MAX_WINDOW_MS = 21_600_000.0`.

## Стакан

### `INITIAL_LEVEL_CAP = 256`

Код:

- `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\orderbook.rs:25`.
- initial GPU buffer:
  `orderbook.rs:111-113`.
- рост buffer-а до фактической длины:
  `orderbook.rs:116-120`.
- записываются все `levels`, count = `levels.len()`:
  `orderbook.rs:121-125`.

Простыми словами:

GPU draw call не получает "книгу стакана" напрямую. Сначала CPU строит список
маленьких записей `LevelInstance`: где прямоугольник, какой объём, bid/ask,
fill/line. Этот список надо положить в GPU buffer. `256` — это стартовый размер
этого buffer-а. Если записей больше, buffer пересоздаётся больше.

Текущий `OrderBookModel::build_instances` делает две записи на каждый видимый
уровень: fill и line.

- fill push:
  `R:\test\MoonTerminal\crates\moon-core\src\data\orderbook.rs:89-100`.
- line push:
  `orderbook.rs:101-111`.

Если видимо 5000 уровней, получится около 10000 draw-instance-ов. Код не режет
это до 256. Он создаст buffer больше.

Решение:

`INITIAL_LEVEL_CAP` переименовать в `INITIAL_LEVEL_BUFFER_CAPACITY`.

Отдельно важно: стакан не должен отправлять в GPU уровни, которые лежат вне
видимого ценового окна панели. Это не "оптимизация по вкусу", а простая
геометрия:

- raw book в MoonProto/RAM может иметь тысячи уровней;
- `build_instances(lo, hi, out)` должен смотреть пересечение уровня с `[lo, hi]`;
- если уровень выше `hi` или ниже `lo`, он не создаёт ни fill, ни line instance;
- если уровень попадает в `[lo, hi]`, он рисуется как обычный прямоугольник;
- если внутри `[lo, hi]` физически оказалось 5000 уровней, это 5000 видимых
  уровней, а не повод придумывать отдельный cap.

То есть задача стакана — cull невидимое по price range до построения
`LevelInstance`, а не сжимать видимое по высоте экрана.

### Delphi и "5000 уровней стакана"

Код Delphi:

- `X:\proj-X\MoonBot\src\EngineBase.pas:2216`:
  `n := Min(5000, N);`
- `X:\proj-X\MoonBot\src\EngineBase.pas:2218-2219`:
  `SetLength(NewBook, N); SetLength(ABook, N);`
- `X:\proj-X\MoonBot\src\EngineBase.pas:2221-2222`:
  копируется `N * SizeOf(TOrderGlass)`.

Решение:

Не использовать эту строку как аргумент для cap-а стакана. Рабочего Delphi
лимита 5000 тут нет.

### `ORDERBOOK_PULL_PERIOD_MS = 200`

Код:

- `R:\test\MoonTerminal\crates\moon-core\src\market\source.rs:15`.
- use:
  `source.rs:302-329`.

Факт:

Это частота чтения snapshot стакана из MoonProto в terminal render-cache. Это не
cap уровней. Это throttle обновления данных стакана.

Delphi похожий throttle:

- futures:
  `X:\proj-X\MoonBot\src\EngineBase.pas:8736-8738`.
- spot:
  `X:\proj-X\MoonBot\src\EngineBase.pas:8682-8688`.
- формула Delphi: active fullscreen сразу, остальные
  `Min(5000, 500 + GlobalOpenedCharts * 500)`.

Решение:

Оставить `200ms` как текущую terminal policy для data snapshot, потому что
визуальный pan/zoom стакана уже не должен ждать эти 200ms: texture rebake на
изменение transform делается сразу.

Код instant rebake:

- `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\orderbook.rs:142-160`.

## Readout / userdata / orders GPU capacities

Эти cap-ы не ограничивают предметные данные. Они стартуют GPU buffer для
draw-instance-ов.

### Readout

Код:

- `INITIAL_RECT_CAP = 4`, `INITIAL_GLYPH_CAP = 64`;
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\readout.rs:18-20`.
- рост до `next_buffer_cap(rects.len(), floor)`:
  `readout.rs:66-77`.
- draw по фактической длине:
  `readout.rs:96`, `readout.rs:102`.

Смысл:

Readout — это маленькие плашки и символы около курсора. Один символ = один
glyph-instance. Если символов больше 64, buffer увеличится.

Решение:

Переименовать в `INITIAL_READOUT_RECT_BUFFER_CAPACITY` и
`INITIAL_READOUT_GLYPH_BUFFER_CAPACITY`.

### Userdata/orders

Код:

- `INITIAL_ZONE_CAP = 64`, `INITIAL_HLINE_CAP = 256`,
  `INITIAL_SEG_CAP = 512`, `INITIAL_MARKER_CAP = 512`;
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\userdata.rs:21-25`.
- рост до фактической длины:
  `userdata.rs:153-166`.
- draw по фактическим count:
  `userdata.rs:199-221`.

Смысл:

Ордера и пользовательские линии превращаются в draw-instance-ы: зоны,
горизонтали, сегменты, маркеры. Эти числа — стартовые размеры GPU buffer-а, не
лимит количества ордеров.

Решение:

Переименовать в `INITIAL_ZONE_BUFFER_CAPACITY`,
`INITIAL_HLINE_BUFFER_CAPACITY`, `INITIAL_SEG_BUFFER_CAPACITY`,
`INITIAL_MARKER_BUFFER_CAPACITY`.

### Metal/wgpu

Metal/wgpu уже не имеют таких локальных initial constants для level/zone/etc.
`BufferSlot::write` пересоздаёт buffer до `need.next_power_of_two()`.

Код:

- Metal:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\metal_backend.rs:61-84`.
- wgpu:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\chartdx\wgpu_backend.rs:65-90`.

Решение:

Для Metal/wgpu ничего по initial cap-ам чинить не надо. Чинить надо только
общую combo history policy.

## MoonProto protocol caps

Это не renderer cap-ы терминала. Это внутренняя защита протокола orderbook в
MoonProtoBeta:

- `BOOK_FULL_REQUEST_THROTTLE = 5000`;
- `BOOK_CACHE_MAX_PACKETS = 64`.

MoonProto Rust:

- `C:\Users\Mike\.cargo\git\checkouts\moonprotobeta-20f0f9a560480e03\a4ce5ec\src\state\order_books.rs:40-45`.
- use:
  `order_books.rs:211-214`, `order_books.rs:263-267`.

Delphi:

- `X:\proj-X\MoonBot\src\MoonProto\MoonProtoOrderBook.pas:11-12`.
- full request throttle:
  `X:\proj-X\MoonBot\src\MoonProto\MoonProtoOrderBook.pas:536`.
- cache count/corruption:
  `X:\proj-X\MoonBot\src\MoonProto\MoonProtoEngine.pas:2051`,
  `MoonProtoEngine.pas:2088`.

Решение:

Оставить в MoonProto. Не переносить эти цифры в chart renderer.

## Closed orders / order overlays

### `max_closed_orders = 500`, slider max `5000`

Код:

- config field/default:
  `R:\test\MoonTerminal\crates\moon-core\src\config\orders.rs:124`,
  `orders.rs:157`.
- slider max:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\settings\lines.rs:206-212`.
- chart берёт открытые + newest closed:
  `R:\test\MoonTerminal\crates\moon-chart\src\lib.rs:76-79`.

### `CLOSED_RING_CAP = 5000`

Код:

- `R:\test\MoonTerminal\crates\moon-core\src\session\order_lines.rs:31-34`.
- удаление старейших закрытых:
  `order_lines.rs:352-365`.

Факт:

Это terminal UX policy: сколько закрытых order-line можно хранить/рисовать.
Открытые ордера не cap-аются.

Решение:

Оставить `CLOSED_RING_CAP = 5000`, потому что он совпадает с max slider и
ограничивает только закрытые overlay orders.

## UI/diagnostic limits

Эти лимиты не относятся к market history и не должны смешиваться с chart caps.
Они нормальны как UI/diagnostic limits:

- app log `RING_CAP = 5000`:
  `R:\test\MoonTerminal\crates\moon-core\src\applog.rs:22`,
  use `applog.rs:115`.
- session detects/log `MAX_DETECTS = 2000`, `MAX_LOG = 5000`:
  `R:\test\MoonTerminal\crates\moon-core\src\session\store.rs:15-19`,
  use `store.rs:97-119`.
- detects buttons `MAX_DETECT_BTNS = 48`:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\panels\detects.rs:58`.
- log panel `VIEW_LIMIT = 5000`:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\panels\log.rs:24`,
  use `log.rs:157-167`, `log.rs:293-294`.
- report rows `MAX_REPORT_ROWS = 100_000`:
  `R:\test\MoonTerminal\crates\moon-ui-gpui\src\panels\report.rs:27`,
  use `report.rs:194`.
- feed reconnect backoff `BACKOFF_MAX = 30s`:
  `R:\test\MoonTerminal\crates\moon-core\src\feed\mod.rs:129`.
- metrics memory window `MEM_WINDOW = 5s`:
  `R:\test\MoonTerminal\crates\moon-core\src\metrics.rs:13`.

## Итоговый список правок

1. В MoonProto memory policy добавить пользовательский множитель памяти
   графиков. Единственная магия остаётся в одном месте: базовый бюджет от RAM.
2. Добавить в MoonProto универсальные retained-history reader primitives:
   time-range/tail read, range query/scan, bounded cursor drain с
   `copied/clipped/caught_up`.
3. Удалить независимые `TICK_CAP` и `PRICE_LINE_CAP`; terminal не должен держать
   long-lived дубль MoonProto history в RAM.
4. Удалить `MARKET_PULL_BATCH = 8192`; live-drain делать через MoonProto
   bounded API, а clipped backlog лечить full visible/tail reupload.
5. Оставить GPU combo как resident ring-кэш MoonProto history, но удалить
   независимые `COMBO_RING_CAP` и `COMBO_HISTORY_CAP` как самостоятельную
   renderer policy. Capacity брать из MoonProto/source history policy для
   соответствующего stream-а/market-а.
6. Привести DX/Metal/wgpu к одной combo ring-модели: cheap append в `head`,
   reset/reupload из MoonProto только при смене source/range/device или cache
   miss при pan/zoom.
7. Исправить `MAX_WINDOW_MS` на `21_600_000.0`.
8. Переименовать `INITIAL_*_CAP` в `INITIAL_*_BUFFER_CAPACITY`.
9. Для стакана гарантировать price-window culling до построения GPU instance-ов:
   невидимые уровни не должны попадать в GPU buffer.
10. Не вводить cap 5000 уровней стакана: Delphi-код его не доказывает.
11. Не трогать UI/diagnostic limits в рамках этой задачи.

## Статус реализации

Дата статуса: 2026-06-18.

1. MoonProto retained-history API готов в публичном `Moonbot-Tech/MoonProtoBeta`
   (`f54e086`): `SeqRingDrainMeta`, `drain_new_bounded`,
   `scan_from_cursor`, `copy_time_range_ms`, `MarketHistorySizing` с
   `auto_with_budget_percent`.
2. Терминал использует `MarketHistorySizing::auto_with_budget_percent` и
   передаёт пользовательский `chart_memory_percent` в live client config.
3. Runtime-код терминала больше не содержит `TICK_CAP`, `PRICE_LINE_CAP`,
   `MARKET_PULL_BATCH`, `TickRing`, `PriceLineRing`, `TickInstance`.
4. `MarketView` больше не владеет второй trade/price history; chart history
   читается из MoonProto readers через per-pane cursor и scratch buffers.
5. GPU combo capacity больше не задаётся локальными `COMBO_RING_CAP` /
   `COMBO_HISTORY_CAP` в renderer-е. Capacity приходит из MoonProto reader
   capacity и прокидывается в `PlatformLayers`.
6. DX11 combo остаётся настоящим resident GPU ring: reset/append пишут в ring по
   runtime capacity от MoonProto.
7. Metal/wgpu больше не имеют левого `1 << 17` cap-а; они держат resident tail с
   capacity от MoonProto и не переливают большие storage buffers на каждый
   camera/base prepare, а только при dirty data/capacity change. Их shader path
   остаётся dense-buffer path, не DX-style write-at-head ring.
8. Стакан делает price-window culling до построения `LevelInstance`: невидимые
   уровни не попадают в GPU buffer. Pixel-binning/merge не является задачей.
9. `MAX_WINDOW_MS` выставлен на `21_600_000.0` (6 часов).
10. `INITIAL_*_CAP` для GPU buffers переименованы в
    `INITIAL_*_BUFFER_CAPACITY`.
11. Windows MSVC проверка и сборка проходят:
    `cargo check -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc`
    и
    `cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc`.
12. MoonProto `f54e086` добавил diagnostics fixture
    `diag_fill_market_history_to_capacity`: он заполняет все retained history
    ring-и рынка до effective capacity синтетикой за последний час через
    правильный retained-history path. В терминале debug-кнопка вызывает этот
    hook и затем делает resident history reupload обычным chart path-ом.
    Viewport при этом НЕ меняется: 60 секунд — только видимое окно, а не cap
    данных. Fake GPU-only injection не нужен.
