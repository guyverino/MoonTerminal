# MoonTerminal — архитектура (актуально)

Кросс-десктопный трейдинговый терминал на **Rust + winit + wgpu + egui** поверх
**MoonProtoBeta**. Один терминал на несколько ядер MoonBot; ядра группируются, и
каждая группа — это **отдельное ОС-окно** со своей раскладкой.

Режим один — **live** (синтетики нет).

---

## 1. Поток данных

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
WindowManager(App) ── WindowHost(группа) = Chart(wgpu) + Shell(egui) + Dock(egui) + Workspace
                   ├─ SettingsWindow (нативное окно)
                   └─ ReportsWindow  (нативное окно, SQLite)
```

- Один **backend-поток на ядро** владеет своим `MoonClient` (`feed/live.rs`). UI
  **никогда** не зовёт moonproto напрямую — только читает `FeedMsg` из канала и
  шлёт `CoreCmd`.
- **Аккаунт-данные** (ордера/детекты/стратегии/статус) живут per-core в
  `CoreStore`. **Маркет-данные** (трейды/стакан) дедуплицируются в `MarketStore`
  по ядру-провайдеру (см. §6). GPU-работа — только для окон, которым реально
  нужно перерисоваться (dirty + skip-present).

### FeedMsg (backend → ui), `feed/types.rs`

| Вариант | Что несёт |
|---|---|
| `Status(ConnStatus)` | стадия подключения / ready / failed |
| `Identity(ExchangeId)` | биржа ядра (один раз за сессию) — для выбора провайдера |
| `Ticks { market, ticks }` | новые трейды (append-only, **только провайдер**) |
| `OrderBook { market, book }` | снапшот стакана (**только провайдер**, троттл ~20 Гц) |
| `Orders(Vec<OrderRow>)` | открытые ордера по всем рынкам ядра |
| `Detects(Vec<DetectRow>)` | пачка новых детектов |
| `Strategies(Vec<StrategyRow>)` | снапшот стратегий |

### CoreCmd (ui → backend), `feed/mod.rs`

| Вариант | Смысл |
|---|---|
| `SetMarket { provider: bool, markets: Vec<String> }` | роль ядра: `provider=true` → `subscribe_all_trades` по списку `markets`; `false` → только аккаунт-данные |

---

## 2. Карта модулей (`src/`)

| Путь | За что отвечает |
|---|---|
| `main.rs` | `env_logger`, `AppConfig::load`, `rust_i18n::set_locale`, taskbar-id, `EventLoop`, запуск `App`. |
| `applog.rs` | Файловый лог сырых report-SQL команд от ядра (потокобезопасный, write-once). |
| `icons.rs` | `IconSet`: PNG-иконки из `assets/icons` → текстуры egui + `winit::Icon`. |
| `metrics.rs` | `Metrics`: CPU/RAM процесса и системы для статус-бара (sysinfo, опрос ≤1 c, дельта RAM за 5 c). |
| `symbol.rs` | Разбор тикера: котируемая валюта (BTC**USDT**→USDT) и базовый символ для отображения. |
| `win_taskbar.rs` | **Windows**: AppUserModelID на окно (раздельные кнопки taskbar). На macOS/Linux — no-op. |
| **config/** | Конфиг, секреты, темы. |
| `config/mod.rs` | `AppConfig { servers, groups, language, market_mode, theme }` + `load()/save()`, миграция, `structural_sig()`. |
| `config/schema.rs` | Дисковый формат (serde): `ServersFile`/`SettingsFile`, `SCHEMA_VERSION`, forward-compat через `#[serde(default)]`. |
| `config/store.rs` | Низкоуровневый I/O: шифр `servers.enc`, открытый `settings.toml`; порченый settings → `.bak`. |
| `config/servers.rs` | `ServerConfig { id, uid, name, active, show_window, feed, host, port, key, group, market, color }`, `FeedFlags`. |
| `config/secrets.rs` | `Secret` — маска в Debug/UI (`masked`), затирание (`zeroize`), `buffer_mut` для ввода. |
| `config/crypto.rs` | `encrypt/decrypt` (AES-256-GCM), 32-байтный ключ в OS keyring (`service=moon-terminal`). |
| `config/paths.rs` | пути `servers.enc`/`settings.toml`/`theme.toml`/`reports.db`/логов рядом с exe + legacy. |
| `config/migrate.rs` | Разовая миграция legacy `config.enc` / `config.toml` → `servers.enc` + `settings.toml`. |
| `config/reconcile.rs` | Слияние `ServersFile` + `SettingsFile` → runtime `ServerConfig`, стабильные `uid`. |
| `config/groups.rs` | `GroupConfig { name, active, icon }` — свойства окна-группы. |
| `config/lang.rs` | `Language::Ru/En/Es`, `code()/label()`, дефолт по системной локали. |
| `config/theme.rs` | `ChartTheme` (цвета bg/grid/cross/halo/book_*/panel_* в sRGB) ↔ переносимый `theme.toml`. |
| **feed/** | Граница с ядром. |
| `feed/types.rs` | `Tick/Side/Level/OrderBook/OrderRow/DetectRow/StrategyRow/ConnStatus/ExchangeId` + `FeedMsg`. |
| `feed/mod.rs` | `FeedHandle` (канал данных + `cmd_tx`), `CoreCmd::SetMarket`, `spawn(server)`. |
| `feed/live.rs` | Единственный, кто знает moonproto: connect, lifecycle→статус, динамические подписки, чтение trades(курсор)/orderbook/orders/detects/reports. |
| **session/** | Мульти-ядра. |
| `session/store.rs` | `CoreId`, `CoreData { status, orders, detects(≤2000), strategies, *_rev }`, `CoreStore`. |
| `session/mod.rs` | `SessionManager` (поток на ядро), `CoreSession`, `drain()`, `market_view()`, `set_market_mode()`. |
| `session/coordinator.rs` | Выбор провайдера на биржу (Dedup/PerCore), `reconcile_providers()`, `set_open()`, linger-задержка. |
| `market/mod.rs` | `MarketStore`/`MarketView`: дедуп трейдов/стакана по провайдеру; режим `MarketDataMode::{Dedup,PerCore}`. |
| **gpu/** | `GpuContext` — wgpu instance/adapter/device/queue/surface, resize. |
| **chart/** | Низкоуровневый рендер графика. |
| `chart/mod.rs` | `Chart`: слои + `canvas` + `view`, `render(rect, data)`, перезалив GPU-буферов по `*_rev`, scissor-зоны. |
| `chart/canvas.rs` | Offscreen-битмап крестиков (Stage 2c): `bake/append/rebake/composite`, UV-scroll, `MARGIN_PX=1024`. |
| `chart/view.rs` | `ChartView` — follow (hold 3 c)/fit_price/zoom/pan + сборка `ChartUniform`. |
| `chart/transform.rs` | `ChartUniform` + общий bind group `(time,price)→px→clip`. |
| `chart/style.rs` | `StyleUniform` — тема как GPU-uniform (group 1): цвета + толщина/halo; `from_theme()`. |
| `chart/data/tick_ring.rs` | semantic ring тиков (SoA-инстансы), `visible_range()`, `price_range_in()`, `dropped()`. |
| `chart/data/orderbook.rs` | модель стакана: кумулятивная глубина + линии уровней (bid/ask × fill/line). |
| `chart/layers/` | `grid`/`crosses`/`glass`/`cursor` + `mod.rs` (хелперы пайплайнов, `InstanceBuf`). |
| **shell/** | egui-хром окна. |
| `shell/mod.rs` | header (рынок/цена/статус/⚙) + статус-бар с метриками; `HEADER_H=46`, `STATUS_H=24`. |
| `shell/theme.rs` | стиль egui, шрифт Geist Mono, кнопки/пилюли/градиенты. |
| `shell/brand.rs` | SVG-лого MoonBot → текстура egui (resvg+tiny_skia, ×2 supersample). |
| **dock/** | Раскладка панелей окна. |
| `dock/mod.rs` | `Dock`: тулбар + правая панель ордера + нижний док + лента детектов + центральный rect. |
| `dock/toolbar.rs` | размеры/масштаб(Auto/S1..)/live-follow/детекты/стратегии/справка/отчёты. |
| `dock/controls.rs` | `OrderControls` (5 размеров, пресеты масштаба). |
| `dock/order.rs` | панель: BUY/SELL/CancelBuy/PanicSell/Настройки (пока **лог**, без торговли). |
| `dock/orders_panel.rs` | нижний док: таблица открытых ордеров группы (11 колонок). |
| `dock/detects.rs` | `DetectRibbon`: лента детектов справа, TTL по KeepAlert, клик → чарт рынка/ядра. |
| `dock/close_btn.rs` | кнопка закрытия чарта (X). |
| **settings/** | Содержимое окна настроек (3 вкладки). |
| `settings/mod.rs` | `SettingsState` + трейт `SettingsTab` (draft-конфиг, валидация, Save). |
| `settings/connections.rs` | `ConnectionsTab`: таблица серверов (имя/биржа/host/port/ключ•••/группа/цвет/фид-фильтры) + группы с иконками. |
| `settings/general.rs` | `GeneralTab`: выбор языка (применяется после Save, пересоздаёт окна). |
| `settings/interface.rs` | `InterfaceTab`: живой редактор темы графика (цвета/прозрачность/толщина/halo). |
| **window/** | Нативные ОС-окна. |
| `window/host.rs` | `WindowHost` — одно окно = группа: surface+egui+`Chart`+`Shell`+`Dock`+`Workspace`, dirty/skip-present, кэш egui-меша. |
| `window/settings_window.rs` | `SettingsWindow` — **отдельное нативное окно** настроек (свой surface+egui, владеет `SettingsState`). |
| `window/reports_window.rs` | `ReportsWindow` — **отдельное нативное окно** отчётов: читает SQLite, фильтры/сортировка/top-N/итоги, авто-обновление по генерации. |
| **workspace/** | `Workspace { group, cores, open_chart, dock }`, `build_all(config)` (группировка по `group`). |
| `app/mod.rs` | `App` — менеджер окон: маршрутизация событий по `WindowId`, рендер-цикл, пересоздание окон при Save, владелец Settings/Reports. |
| **db/** | Локальная БД отчётов. |
| `db/mod.rs` | SQLite-зеркало закрытых ордеров (PK `core_uid+db_id`, UPSERT), writer-поток через mpsc, запросы. |
| `db/parse.rs` | Парсер report-SQL (INSERT/UPDATE) → `ParsedReport`; учитывает кавычки/экранирование. |
| **shaders/** | `common.wgsl` (склеивается) + `grid/crosses/canvas/glass/glass_bg/cursor.wgsl`. |
| **locales/** | `app.yml` — RU/EN/ES строки интерфейса (rust-i18n, fallback `en`). |

---

## 3. Окна по группам

- `Workspace::build_all(config)` группирует серверы по полю `group` → список
  workspace (= групп).
- `App::build_windows` создаёт **`WindowHost` на каждую группу** (свой
  `winit::Window` + surface + egui + `Chart` + `Dock`). Нет серверов → одно пустое
  окно (чтобы открыть Настройки).
- Заголовок окна = имя группы; иконка из `GroupConfig.icon`. На Windows каждому
  окну ставится свой AppUserModelID → раздельные кнопки в taskbar.
- Закрыли все окна → выход. Save в настройках → сессии и окна пересоздаются по
  новым группам.
- Внутри группы с несколькими ядрами активное ядро выбирается (для чарта); ордера
  в нижнем доке агрегируются по **всем** ядрам группы.
- **Settings** и **Reports** — собственные нативные окна поверх группы-владельца,
  а не `egui::Window` внутри (старый долг закрыт).

---

## 4. Раскладка окна (Dock)

```
[ header: рынок · цена · статус · метрики · ⚙ ]
[ toolbar: размеры ордера · масштаб(Auto/S1..) · live-follow · детекты · стратегии/справка/отчёты ]
[  график + стакан (wgpu)        | панель ордера: BUY/SELL/Cancel/Panic | лента детектов ]
[ нижний док: открытые ордера группы (resizable, на всю ширину) ]
[ status bar: ticks/fps · CPU/RAM ]
```

- Центральный прямоугольник под график берётся из `CentralPanel` egui (точки →
  пиксели), поэтому панели реально «отъедают» место.
- Нижний док — таблица: **Ядро · Сторона(LONG/SHORT) · Токен · Size · SL · TS ·
  VStop · Buy · Цена · Fill · Strat** (Strat = тип стратегии, не число).
- Лента детектов справа — живая (см. §6): кнопки с обратным отсчётом TTL, клик
  открывает чарт нужного рынка/ядра.

---

## 5. Рендер-модель (важно для GPU)

- Все окна обслуживает **один winit event loop**; рендер вызывается напрямую из
  рендер-цикла (не зависим от фокуса/`RedrawRequested`).
- **Dirty + skip-present**: окно рисуется и презентится **только если** у его
  ядра изменились тики/стакан/ордера (`*_rev`) или был ввод/UI-событие. Иначе
  кадр пропускается → idle ≈ 0% GPU.
- **Bitmap-canvas + UV-scroll** (`chart/canvas.rs`, Stage 2c) — основной путь
  крестиков: тики **пекутся в offscreen-текстуру** шире вьюпорта (+`MARGIN_PX`),
  плавное движение по времени даёт целочисленный UV-сдвиг (`composite`), без
  перерисовки. **Rebake** — только при смене Y/зума или выходе скролла за
  пределы; **append** — на дозалив новых тиков. Композит блитится
  nearest-neighbor (без блюра).
- График **tick-driven**: правый край следует за временем последнего тика
  (live-follow с удержанием 3 c), а не за стенными часами. Нет тиков → нет
  движения → нет GPU.
- **Слои** (порядок рендера): `grid` (фон+сетка 10×60) → `canvas.composite`
  (крестики) → `glass` (фон стакана `glass_bg.wgsl` + бары/линии `glass.wgsl`) →
  `cursor` (перекрестие + halo). scissor по зонам график/стакан.
- **Тема** едет в шейдеры как `StyleUniform` (group 1); цвета хранятся в sRGB, а
  **sRGB→linear конвертится в шейдере** (swapchain sRGB ждёт linear — иначе серый
  фон).

---

## 6. Маркет-данные и подписки (дедуп через провайдера)

- Ядро **всегда подключено** (статус, ордера, детекты, стратегии). Подписку на
  **trades/orderbook** (`subscribe_all_trades`) держит **одно выбранное ядро на
  биржу** — провайдер.
- `session/coordinator.rs` каждый кадр:
  - `reconcile_providers()` — в режиме **Dedup** группирует ядра по `ExchangeId`
    (из `Identity`) и выбирает один здоровый `Ready`-кор провайдером на биржу (с
    failover); в режиме **PerCore** провайдер — каждое ядро само.
  - `set_open()` — собирает желаемые рынки (объединение открытых чартов на
    провайдера), добавляет новые (сброс view → провайдер дочитает retained-историю),
    снимает закрытые **через 5 c linger** (антидребезг при быстром реоткрытии), и
    шлёт `CoreCmd::SetMarket` только тем ядрам, у кого роль/список изменились.
- Данные провайдера складываются в `MarketStore` (`market/mod.rs`), ключ — CoreId
  провайдера → рынок → `MarketView` (ring тиков + снапшот стакана). Окна читают
  через `SessionManager::market_view()`.
- Закрыли окно → рынок уходит из «открытых» → после linger ядро отписывается.

> Дизайн рассчитан на ~200 ядер: одна подписка на биржу вместо подписки в каждом
> ядре. Аккаунт-данные при этом остаются строго per-core.

---

## 7. Конфиг и секреты

- Конфиг разбит на три файла рядом с exe:
  - **`servers.enc`** — сервера с ключами, шифр **AES-256-GCM**; 32-байтный ключ
    случайный, в **OS keyring** (Win Credential Manager / macOS Keychain),
    `service=moon-terminal`. Формат: `nonce(12) ++ ciphertext+tag`.
  - **`settings.toml`** — открытые настройки (язык, режим маркет-данных, группы).
    Битый файл бэкапится в `.bak`.
  - **`theme.toml`** — переносимая тема графика (можно шарить между людьми).
- Ключи в UI маскируются (`Secret`), в памяти затираются (`zeroize`).
- Старые `config.enc` / `config.toml` (если есть) один раз мигрируют в новый
  формат (`config/migrate.rs`). `SCHEMA_VERSION` управляет эволюцией формата.

---

## 8. Отчёты (SQLite)

- `feed/live.rs` дренит report-команды ядра; `db/parse.rs` парсит report-SQL
  (INSERT при открытии / UPDATE при закрытии) в `ParsedReport`.
- `db/mod.rs` пишет в локальную **SQLite** (`reports.db`, `rusqlite` bundled —
  без системных зависимостей) через writer-поток; PK `(core_uid, db_id)`, UPSERT.
  Хранит зеркало ордеров биржи: монета/сторона/цены/даты/qty/profit/leverage/
  стратегия/статус/причина/комментарий + аналитика.
- `window/reports_window.rs` — отдельное окно: фильтры (ядро/даты/монета/сторона),
  сортировка по клику, top-N, итоги; авто-обновление по счётчику генерации от
  writer'а.

---

## 9. Локализация

- **RU / EN / ES** через `rust-i18n` (`i18n!("locales", fallback = "en")`), строки
  в `locales/app.yml` (~100 ключей). Язык по умолчанию — системная локаль
  (`sys-locale`), переключается в Настройки → Общие (применяется после Save).
- Намеренно **не переводятся**: трейдинговые термины (BUY/SELL/Cancel Buy/PANIC
  SELL/LONG/SHORT/Size/SL/TS/VStop) и метрики (ticks/fps/CPU/RAM).

---

## 10. Что НЕ сделано / отклонения от UI-ТЗ

(см. `docs/docs__TERMINAL_UI_TZ.md`)

- **Торговля не подключена**: кнопки BUY/SELL/Panic/Cancel **логируют**; реальные
  команды через `client.trade()/orders()` + guard-ы — следующий шаг (расширить
  `CoreCmd` торговыми намерениями).
- **Нет GPU effects-слоя** (ripple/glow/flash) — `ui_effects` не реализован.
- **Нет окна `UI Kit`** (демо-таблицы, date/time picker, context menu, popup,
  loading, progress).
- **Нет cell-flash** в таблицах.
- Окна с **системной рамкой** (а не свой тонкий borderless shell).

> Закрытые ранее долги: native-окно Settings, мультиядро/мультиокно, дедуп
> маркет-данных, bitmap-canvas + UV-scroll, темы графика, i18n, локальные отчёты.

---

## 11. Дальнейшие шаги (ориентир)

1. **Торговая точка входа**: расширить `CoreCmd` (NewOrder/Cancel/Move/Panic/…),
   исполнять в backend через `client.trade()/orders()`, guard + dry-run, `uid` в
   `OrderRow`, результат назад в UI.
2. **GPU effects layer** (ripple/glow/flash) и cell-flash в таблицах.
3. **UI Kit** демо и borderless-shell окна.
4. Тюнинг рендера под ~200 ядер (профиль dirty/present, бюджеты перезаливки).
