# MoonTerminal — архитектура (актуально)

Кросс-десктопный трейдинговый терминал на **Rust + GPUI** (оболочка) с **own-pass DX11**
рендером чарта, поверх **MoonProtoBeta**. Один терминал на несколько ядер MoonBot; ядра
группируются, и каждая группа — это **отдельное ОС-окно** со своей раскладкой.

Режим один — **live** (синтетики нет).

> UI — порт egui-версии на GPUI (модули помечены «порт egui …»). Старый egui-бинарь и
> wgpu-движок чарта удалены; переход — `docs/REFACTOR_RENDER.md`, рендер-план — `docs-internal/RENDER_PLAN.md` (внутр., вне публичного репо).

---

## 0. Крейты

- **`moon-core`** — backend, UI-агностик: feed/session/market/coordinator/config/db/data/
  metrics/symbol/palette. Не зависит ни от GPUI, ни от wgpu.
- **`moon-chart`** — чарт-математика/геометрия, **wgpu-free**: `view::ChartView` (зум/пан/Y/
  follow), `axes` (тик-математика осей), `transform::ChartUniform`, `container` (типы вкладок),
  `build_order_geometry`, типы инстансов линий, константы. Данные рисует own-pass.
- **`moon-ui-gpui`** — единственный бинарь `moon-gpui`: GPUI-оболочка на **MoonPalette** +
  own-pass DX11 рендер чарта (`src/chartdx/`).

---

## 1. Поток данных (backend, moon-core)

```
servers.enc ──decrypt(keyring+AES)──▶ AppConfig { servers, groups, language, market_mode, theme }
        │
        ▼  (поток на ядро, live/moonproto)
SessionManager ─FeedMsg─▶ │ аккаунт-данные  → CoreStore[CoreId]   (status/orders/detects/strategies)
        ▲                 │ маркет-данные    → MarketStore[provider] (трейды/стакан, дедуп)
        │                 └ Identity(биржа)  → coordinator
        │
        └──────────── CoreCmd::SetMarket { provider, markets } ◀── coordinator (выбор провайдера)
                                                                   │
GPUI App ── окно-группа (GPUI Window) = own-pass чарт (chartdx) + MoonPalette-доки
         ├─ окно «Настройки» (3 вкладки)
         └─ окно «Отчёты» / «Стратегии»
```

- Один **backend-поток на ядро** владеет своим `MoonClient` (`feed/live.rs`). UI **никогда**
  не зовёт moonproto напрямую — только читает `FeedMsg` из канала и шлёт `CoreCmd`.
- **Аккаунт-данные** (ордера/детекты/стратегии/статус) — per-core в `CoreStore`.
  **Маркет-данные** (трейды/стакан) дедуплицируются в `MarketStore` по ядру-провайдеру (§6).

### FeedMsg (backend → ui), `moon-core/feed/types.rs`

| Вариант | Что несёт |
|---|---|
| `Status(ConnStatus)` | стадия подключения / ready / failed |
| `Identity(ExchangeId)` | биржа ядра (один раз за сессию) — для выбора провайдера |
| `Ticks { market, ticks }` | новые трейды (append-only, **только провайдер**) |
| `OrderBook { market, book }` | снапшот стакана (**только провайдер**, троттл ~20 Гц) |
| `Orders(Vec<OrderRow>)` | открытые ордера по всем рынкам ядра |
| `Detects(Vec<DetectRow>)` | пачка новых детектов |
| `Strategies(Vec<StrategyRow>)` | снапшот стратегий |

### CoreCmd (ui → backend), `moon-core/feed/mod.rs`

| Вариант | Смысл |
|---|---|
| `SetMarket { provider: bool, markets: Vec<String> }` | роль ядра: `provider=true` → `subscribe_all_trades` по `markets`; `false` → только аккаунт-данные |

---

## 2. Карта модулей `moon-ui-gpui/src/`

| Путь | За что отвечает |
|---|---|
| `main.rs` | GPUI-оболочка (миграция с egui): точка входа, окна-группы, event loop. |
| **`chartdx/`** | **НАШ own-pass DX11 рендер чарта** (§5). Файл на слой. |
| `chartdx/mod.rs` | Оркестратор `ChartEngine`: `prepare` данных per pane + `register_pass` (own-pass под сценой). |
| `chartdx/gpu.rs` | DX11-хелперы, device-lost guard, scissor, шейдеры `include_str!`, `ChartViewGpu`. |
| `chartdx/combo.rs` | История трейдов (кресты): bake в битмап шире экрана + UV-pan + append живого края. |
| `chartdx/orderbook.rs` | Стакан: фон зоны + кумулятивные бары + линии уровней (своя зона справа). |
| `chartdx/userdata.rs` | Ордера юзера: горизонтали/отрезки/маркеры (instanced, из `build_order_geometry`). |
| `chartdx/grid.rs` | Сетка (вертикали по подписям времени, горизонтали по `nice_interval`). |
| `chartdx/cursor.rs` | Крестик own-pass — **избыточен** (дублирует GPUI-крестик `axes.rs`), под удаление. |
| `chartdx/pane.rs` | Контейнер панелей (Fullscreen/Tiled, TTL); панель хранит `ChartView` (без GPU). |
| `chartdx/view.rs` | Маппинг `ChartView` → `ChartViewGpu` (порядок полей под cbuffer), конверт тиков. |
| `chartdx/shaders/*.hlsl` | crosses/bars/grid/order_lines/cursor/blit. |
| `axes.rs` | GPUI-оверлей осей: цена слева, время снизу, крестик + readout'ы (поверх own-pass). |
| `input.rs` | Ввод чарта (порт egui 1:1): колесо/зум/пан/ПКМ/дабл-клик (девайс-px). |
| `panels/mod.rs` | Dock-панели окна-группы как `moon_palette::Panel`. |
| `panels/chart.rs` | Панель чарта (center): own-pass рендер + ввод + GPUI-оверлей осей; отцепляется в окно. |
| `panels/order.rs` | Панель ордера (right): BUY/SELL/Cancel/Panic. |
| `panels/orders.rs` | Таблица открытых ордеров группы (все ядра). |
| `panels/detects.rs` | Лента детектов (открепляемая, TTL, клик → чарт). |
| `panels/report.rs` | Таблица закрытых сделок (читает SQLite). |
| `panels/log.rs` | Просмотр лога с фильтром/выбором источника. |
| `panels/stub.rs` | Заглушка-панель до подключения данных. |
| `chart_tabs.rs` | Таб-стрип чартов: Main + AddToChart-N. |
| `controls.rs` | Торговый тулбар поверх MoonPalette. |
| `detached.rs` | Откреплённые dock-панели в отдельных ОС-окнах (+ персист). |
| `dock_persist.rs` | Персист раскладки доков (→ `docks.json`/`detached.json`). |
| `design.rs` | Design-tokens терминала. |
| `icons.rs` | Иконки групп из `assets/icons/{id}.png` → текстуры GPUI. |
| `settings/` | Окно настроек: `connections` (ядра/группы), `general` (язык), `interface` (тема), `lines` (стиль ордер-линий). |
| `strategies/` | Окно «Стратегии»: `rules` (зависимости полей), `filter` (фильтры дерева). |

Бэкенд-модули (`moon-core/src/`) — без изменений: `feed/`, `session/`, `market/`, `config/`,
`db/`, `data/`, `metrics.rs`, `symbol.rs`, `palette.rs`.

---

## 3. Окна по группам

- Серверы группируются по полю `group` → на каждую группу **своё GPUI-окно** (заголовок = имя
  группы, иконка из `GroupConfig.icon`; на Windows — свой AppUserModelID → раздельные кнопки taskbar).
- Внутри группы с несколькими ядрами активное ядро выбирается для чарта; ордера в панели «Ордера»
  агрегируются по **всем** ядрам группы.
- **Настройки**, **Отчёты**, **Стратегии** — отдельные ОС-окна.
- Dock-панели можно **откреплять** в отдельные окна (`detached.rs`); раскладка доков и откреплённых
  окон персистится (`dock_persist.rs` → `docks.json`/`detached.json`).

---

## 4. Раскладка окна (MoonPalette docks)

Окно-группа — `moon_palette` DockArea: центральная панель чарта + правый док ордера + нижний док
(ордера/отчёт/лог) + лента детектов. Панели = `moon_palette::Panel` (`panels/*`). Чарт-панель
объявляет `MoonBackgroundPolicy::NoFill` — прозрачный регион, под которым own-pass рисует график.

---

## 5. Рендер-модель чарта (own-pass DX11, важно)

- `ChartEngine::register_pass` ставит **один** own-pass через generic-хук GPUI `RawGpuAccess`
  (фаза `GpuPhase::UnderScene`): callback рисует все видимые панели **прямо в backbuffer окна**,
  **без wgpu и без readback**. GPUI-хром/попапы/текст идут сценой ПОВЕРХ.
- Каждый кадр `ChartEngine::prepare` (дёшево): математика вида + конверт новых тиков + сборка
  геометрии слоёв. Тяжёлое (bake/blit/instanced draw) — на GPU в callback.
- **Слои по природе данных** (Z: Grid → Combo → OrderBook → UserData), scissor по зоне панели:
  - **combo** — неизменная история трейдов: кресты пекутся в битмап шире экрана (+запас), движение
    времени = целочисленный UV-pan, новые тики = append; история не перерисовывается.
  - **orderbook** — срез стакана: своя зона справа, бары/линии уровней по видимому ценовому окну.
  - **userdata** — ордера (мутируют): горизонтали/отрезки/маркеры instanced из `build_order_geometry`;
    пан/зум двигает юниформ, буфер не трогаем.
  - **grid** — сетка + фон.
- **Текст/оси/крестик** — GPUI-оверлей поверх (`axes.rs`): нативный субпиксельный текст.
- **Тема** едет в шейдеры cbuffer'ами (sRGB → linear в шейдере).
- **device-lost guard**: при пересоздании device GPUI слои сбрасывают ресурсы и перезаливают историю.
- Перерисовка: фокусный чарт — по vsync (`request_animation_frame`); фоновые/мультичарт — по
  сигнатуре данных (`data_signature`). Детали и недоделки (версий-гейты и пр.) — `docs-internal/RENDER_PLAN.md` (внутр., вне публичного репо).

---

## 6. Маркет-данные и подписки (дедуп через провайдера)

- Ядро **всегда подключено** (статус, ордера, детекты, стратегии). Подписку на **trades/orderbook**
  (`subscribe_all_trades`) держит **одно выбранное ядро на биржу** — провайдер.
- `session/coordinator.rs` каждый кадр:
  - `reconcile_providers()` — в режиме **Dedup** группирует ядра по `ExchangeId` и выбирает один
    здоровый `Ready`-кор провайдером на биржу (с failover); в **PerCore** провайдер — каждое ядро само.
  - `set_open()` — собирает желаемые рынки (объединение открытых чартов на провайдера), добавляет
    новые (сброс view → дочитать retained-историю), снимает закрытые **через 5 c linger**, шлёт
    `CoreCmd::SetMarket` только тем ядрам, у кого роль/список изменились.
- Данные провайдера → `MarketStore` (`market/mod.rs`): CoreId провайдера → рынок → `MarketView`
  (ring тиков + снапшот стакана). UI читает через `SessionManager::market_view()`.

> Дизайн на ~200 ядер: одна подписка на биржу вместо подписки в каждом ядре. Аккаунт-данные — строго per-core.

---

## 7. Конфиг и секреты

- **`servers.enc`** — сервера с ключами, **AES-256-GCM**; 32-байтный ключ в **OS keyring**
  (`service=moon-terminal`). Формат: `nonce(12) ++ ciphertext+tag`.
- **`settings.toml`** — открытые настройки (язык, режим маркет-данных, группы). Битый файл → `.bak`.
- **`theme.toml`** — переносимая тема графика.
- Ключи в UI маскируются (`Secret`), в памяти затираются (`zeroize`). Старые `config.enc`/`config.toml`
  один раз мигрируют (`config/migrate.rs`); `SCHEMA_VERSION` управляет эволюцией формата.

---

## 8. Отчёты (SQLite)

- `feed/live.rs` дренит report-команды ядра; `db/parse.rs` парсит report-SQL (INSERT при открытии /
  UPDATE при закрытии) в `ParsedReport`.
- `db/mod.rs` пишет в локальную **SQLite** (`reports.db`, `rusqlite` bundled) через writer-поток;
  PK `(core_uid, db_id)`, UPSERT. Окно «Отчёты» (`panels/report.rs`) читает БД: фильтры/сортировка/
  top-N/итоги, авто-обновление по счётчику генерации.

---

## 9. Языки

- `moon-core` хранит выбор языка (`config/lang.rs`: `Language::Ru/En/Es`) и режим маркет-данных.
- GPUI-оболочка пока **в коде на русском** (rust-i18n из egui-версии в GPUI-шелл не перенесён).
  Трейдинговые термины и метрики (BUY/SELL/LONG/SHORT/Size/SL/TS, ticks/fps/CPU/RAM) — английские.

---

## 10. Незакрытое

- **Рендер чарта**: версий-гейты, авто-Y по ордерам, зум-к-курсору/аккумуляция колеса, серверная
  трасса ордеров (вместо реконструкции), PriceLines/Volume/Background/ChartObj, удаление избыточного
  own-pass крестика. Полный план и баги — `docs-internal/RENDER_PLAN.md` (внутр., вне публичного репо).
- **Торговля**: кнопки BUY/SELL/Panic/Cancel пока логируют; реальные команды через `client` + guard-ы.
- **i18n** GPUI-оболочки (сейчас строки в коде).
