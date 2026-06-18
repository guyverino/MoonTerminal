# Stage Review

Дата: 2026-06-18.
Режим: анализ без правки кода.

Смотрел свежую стадию после оконного рефактора, retained-history интеграции и Mac/Linux тестов. Windows MSVC `cargo check` ранее проходил, но ниже не "сборочные" замечания, а реальные риски поведения.

## P0: retained history может не дойти до chartdx

Главный найденный баг: live-поток больше не переливает тики через `FeedMsg::Ticks`; он шлет легкий сигнал `FeedMsg::MarketDataChanged` (`crates/moon-core/src/feed/live.rs`). UI на этот сигнал вызывает `refresh_market_data_for_open`, а дальше chartdx должен сам дочитать retained history через `MarketDataSource::read_chart_history_into`.

Но в `crates/moon-ui-gpui/src/chartdx/mod.rs` есть опасный ранний выход:

- `source_market_signature` видит `snapshot_revision` MoonProto;
- `refresh_visible_markets` может вернуть `pulled=false`, если в этот момент не пора тянуть стакан;
- старый `market_signature` смотрит только на `MarketView.ticks_rev / price_lines_rev / book_rev`;
- если `market_signature` не изменился, код выходит до `sync_from_market_source`;
- при этом новый `source_sig` уже сохраняется как обработанный.

Итог: retained trade/last/mark данные реально могли поменяться, сигнал пришел, но чтение истории не произошло. Это очень похоже на причину пустых BTC debug chart на macOS: оси есть, окно живое, connection есть, а combo/marks пустые.

Фикс должен быть смысловой: изменение `source_market_signature` для видимого рынка обязано приводить к `sync_from_market_source`, даже если old `market_signature` еще не изменился и orderbook pull не сработал. Нельзя помечать новый retained `source_sig` обработанным до drain retained history.

Тест для закрытия: симулировать `source_sig != last_source_market_sig`, `pulled=false`, `market_signature == last_prepared_market_sig` и проверить, что `read_chart_history_into` все равно вызывается.

## P1: detached/floating окна все еще могут получать неверный parent на Linux

Рефактор `windowing.rs` в целом правильный: появился единый контракт `WindowRelationship` / `WindowTaskbarVisibility`, и новые окна больше не собираются россыпью руками.

Но есть риск в backend-логике Linux:

- `MoonTerminal` для detached chart использует `detached_window_options`, а он делает floating window;
- при restore owner отсутствует, это задумано как independent/fallback;
- в MoonUI Wayland/X11 backend при отсутствии explicit owner все еще есть fallback на keyboard-focused window.

То есть restored independent floating window на Linux может внезапно стать transient/child к текущему focused window. Это ломает ожидаемый контракт "restore independent" и может давать странное поведение: окно без нормальной отдельной управляемости, не там в taskbar, странный stacking.

Фикс лучше делать в MoonUI: fallback на focused parent применять только для legacy dialog-сценария, а не для `WindowRelationship::Independent`. Для owned окон parent должен идти только из explicit owner.

Отдельно надо принять продуктово: detached chart это нормальное отдельное окно или owned tool-window. Если его надо свободно двигать/видеть в taskbar, ему нужен `WindowKind::Normal` и visible taskbar, а не floating hidden по умолчанию.

## P1: device_generation parity не доведена до конца

В MoonUI raw GPU API уже имеет `device_generation`, и Windows/wgpu bump делают на recover. Metal generation, похоже, стабилен на lifetime renderer-а, что само по себе не баг: документация `GpuCanvasRawAccess` говорит, что generation меняется при замене native device/context, а не при каждом resize.

Но в терминальном chartdx верхний слой `PlatformLayers::device_gen()` сейчас фактически Windows-only: для не-Windows возвращается `0`. При этом нижний `RenderState::prepare_gpu` уже читает `gpu.device_generation()` и инвалидирует base cache.

Риск: часть логики `PaneRender` (`last_device_gen`, сброс book/orders rev, force path после device lost) на macOS/Linux не сработает так же, как на Windows. Может не проявляться каждый день, но для PR-grade кроссплатформенности это дырка.

Фикс: либо протащить backend generation через все `PlatformLayers`, либо убрать Windows-only device_gen из orchestration и опираться на generation из raw access единообразно.

## P2: Metal/wgpu cursor-only cache уже не "полный stack", но не идеал

Замечание ревью "base-cache есть только в DX" уже не сходится с текущим кодом:

- `chartdx/metal_backend.rs` имеет `BaseCache`, rebuild и cached base draw;
- `chartdx/wgpu_backend.rs` тоже имеет `BaseCache`, rebuild и cached base draw;
- prepare-фаза отдельно решает `needs_base_cache`.

То есть cursor-only mousemove на macOS/Linux уже не обязан рисовать background/grid/combo/orderbook заново как полный stack. Это хорошая часть.

Но parity еще не идеальная: wgpu/Metal путь все равно стоит проверить профилем на cursor-only, потому что вокруг cached draw остаются uniform/bind-group/frame операции. Это не P0, но для красивого upstream PR лучше иметь цифры: cursor-only должен быть дешевым на всех трех backend-ах.

## P2: retained history reset/capacity может давать тяжелые кадры

В `crates/moon-core/src/market/source.rs` reset retained reader копирует до `reader.capacity()`, а `chartdx/mod.rs` при capacity change может принудительно читать большую историю. Если `chart_memory_percent` высокий и открыто несколько окон, один reset/capacity-change может принести огромный batch в CPU buffers и затем в GPU upload.

Это может объяснять часть Mac bench, где 10 окон проседают сильнее, чем ожидается. Не утверждаю как факт без профиля, но место горячее и должно быть измерено.

Нужно проверить отдельно:

- сколько элементов копируется на initial reset;
- сколько времени занимает `read_chart_history_into`;
- сколько занимает upload/rebuild base cache;
- что происходит при 4-10 видимых графиках в одном окне.

## P2: frame-clock архитектура выглядит правильно, но нужна матрица тестов

В MoonUI направление верное: retained GPU canvas получает frame opportunity, может сказать `Skip` до clear/present, и `RequestPresent` не инвалидирует Rust/GPUI views. Это ровно то, что нужно вместо `request_animation_frame` всего дерева.

Проверенные по коду свойства:

- UI dirty вызывает обычный draw;
- если UI не dirty, сначала спрашиваются GPU canvases;
- если все сказали `Skip`, clear/present не делается;
- chart больше не должен тащить Shell/Orders render на каждый tick.

Что еще надо закрыть тестами:

- Windows: visible chart ticks by monitor/pacer, Orders/Shell не рендерятся 240/s;
- macOS: visible window ticks, inactive/hidden behavior понятен и не ломает live chart;
- Linux Wayland: refresh interval берется от output, fallback 60Hz только когда output неизвестен;
- Linux X11/VNC: visible window не считается `FULLY_OBSCURED`, иначе refresh loop остановится.

## P2: diagnostics местами устарели

В `MarketDataSource::refresh_market` есть диагностические строки вида `readers trades=false last=false mark=false` и `view ring_len/ring_total=0`. После retained-history архитектуры это может быть ложной тревогой: реальные trades/last/mark читаются не через old view ring, а через retained readers.

Для тестов Mac/Linux это вредно: по логам можно решить, что данных нет, хотя проблема в gate/drain.

Нужно обновить diagnostics: печатать snapshot revision, наличие retained readers, reset/drain counts, сколько реально добавлено в combo/price-lines за кадр.

## P3: тестовый plaintext key mode правильный

`MOON_CONFIG_PLAINTEXT=1` уже есть в `crates/moon-core/src/config/mod.rs`, а remote smoke scripts экспортируют plaintext mode. Это правильный путь для автономных тестов без бесконечных Keychain `Allow`.

Публичное приложение может оставаться на encrypted/keychain, но все Mac/Linux smoke scripts должны всегда запускаться через plaintext test config, иначе тесты зависят от ручного клика в GUI.

## Что не считаю багом по текущему коду

- Стабильный `device_generation` на Metal сам по себе не баг, если Metal renderer не пересоздает native device/context.
- Утверждение "Metal/wgpu cursor-only рисует полный stack" устарело: base cache в этих backend-ах уже есть.
- Linux fallback 60Hz не означает жесткий 60Hz всегда: Wayland/X11 пытаются брать refresh от output/mode, fallback нужен для headless/VNC/unknown output.

## Минимальный порядок закрытия

1. Починить P0 retained-history gate в `chartdx/mod.rs`.
2. Добавить тест на retained source signature без book pull.
3. Уточнить Linux parent fallback для `WindowRelationship::Independent`.
4. Привести `device_generation` к единой модели на Windows/macOS/Linux.
5. Прогнать smoke на Windows/macOS/Linux: live connection, BTC chart non-empty, 4 visible charts, detached window move/taskbar, Orders/Shell render rate.
