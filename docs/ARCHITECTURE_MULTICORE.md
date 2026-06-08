# MoonTerminal — архитектура (актуально)

Кросс-десктопный трейдинговый терминал на **Rust + winit + wgpu + egui** поверх
**MoonProtoBeta**. Один терминал на несколько ядер MoonBot; ядра группируются, и
каждая группа — это **отдельное ОС-окно** со своей раскладкой.

Режим один — **live** (синтетики нет).

---

## 1. Поток данных

```
config.enc ──decrypt(keyring+AES)──▶ AppConfig.servers
        │
        ▼  (по ядру)
SessionManager ── feed thread (live, moonproto) ──FeedMsg──▶ CoreStore[CoreId]
        ▲                                                          │
        └────────────── CoreCmd (Subscribe/Unsubscribe/…) ◀───────┤ (читают окна)
                                                                   ▼
WindowManager(App) ── WindowHost(группа) ── Chart(wgpu) + Dock(egui) + Workspace
```

- Один **backend-поток на ядро** владеет своим `MoonClient`. UI **никогда** не
  зовёт moonproto напрямую — только читает `FeedMsg` из канала и шлёт `CoreCmd`.
- Все данные ядер живут в общем `CoreStore` (CPU). GPU-работа — только для окон,
  которым реально нужно перерисоваться (dirty + skip-present).

---

## 2. Карта модулей (`src/`)

| Путь | За что отвечает |
|---|---|
| `main.rs` | Логгер, `AppConfig::load`, создание `EventLoop`, запуск `App`. |
| **config/** | Конфиг и секреты. |
| `config/mod.rs` | `AppConfig { servers }` + `load()/save()` + миграция старого `config.toml` → `config.enc`. |
| `config/servers.rs` | `ServerConfig { id, name, exchange, host, port, key, group, market }`, `Exchange`, `placeholder()`. |
| `config/secrets.rs` | `Secret` — маска в Debug/UI (`masked`), затирание (`zeroize`), `buffer_mut` для ввода. |
| `config/crypto.rs` | `encrypt/decrypt` (AES-256-GCM), ключ — случайный, в OS keyring. |
| `config/paths.rs` | путь `config.enc` рядом с exe + legacy `config.toml`. |
| **feed/** | Граница с ядром. |
| `feed/types.rs` | `Tick/Side/Level/OrderBook/OrderRow/ConnStatus` + `FeedMsg` (backend→ui). |
| `feed/mod.rs` | `FeedHandle` (канал данных + `cmd_tx`), `CoreCmd` (ui→backend), `spawn(server)`. |
| `feed/live.rs` | Единственный, кто знает moonproto: connect, lifecycle→статус, **динамические подписки**, чтение trades (курсор)/orderbook/orders, тип стратегии. |
| **session/** | Мульти-ядра. |
| `session/store.rs` | `CoreId`, `CoreData { ring, book, orders, last_price, last_tick_ms, status, *_rev }`, `CoreStore`. |
| `session/mod.rs` | `SessionManager` (поток на ядро), `CoreSession`, `drain()`, `set_charted()` (подписки по открытым чартам). |
| **gpu/** | `GpuContext` — wgpu instance/adapter/device/queue/surface, resize. |
| **chart/** | Низкоуровневый рендер графика. |
| `chart/mod.rs` | `Chart`: слои + `view`, `render(rect, data)`, перезалив GPU-буферов по `*_rev`, scissor-зоны (график/стакан). |
| `chart/view.rs` | `ChartView` — follow/fit_price/zoom + сборка `ChartUniform`. |
| `chart/transform.rs` | `ChartUniform` + общий bind group `(time,price)→px→clip`. |
| `chart/data/tick_ring.rs` | semantic ring тиков (SoA-инстансы). |
| `chart/data/orderbook.rs` | модель стакана: кумулятивная глубина + линии уровней. |
| `chart/layers/` | `grid`/`crosses`/`glass`/`cursor` + `mod.rs` (хелперы пайплайнов, `InstanceBuf`). |
| **shell/** | egui-хром окна. |
| `shell/mod.rs` | header (рынок/цена/статус/⚙) + статус-бар. |
| `shell/theme.rs` | палитра/стиль (из стенда). |
| **dock/** | Раскладка панелей окна. |
| `dock/mod.rs` | `Dock`: тулбар + правая панель ордера + нижний док + центральный rect под график. |
| `dock/controls.rs` | `OrderControls` (5 размеров, пресеты масштаба). |
| `dock/toolbar.rs` | верхний тулбар: размеры/масштаб/детекты(плейсхолдер)/стратегии/справка/отчёты. |
| `dock/order.rs` | правая панель: BUY/SELL/Cancel/Panic/Настройки (пока лог, без торговли). |
| `dock/orders_panel.rs` | нижний док: таблица открытых ордеров группы. |
| **settings/** | Окно настроек. |
| `settings/mod.rs` | `SettingsState` + трейт `SettingsTab` + egui-окно (draft конфига, Save). |
| `settings/connections.rs` | `ConnectionsTab`: таблица серверов (имя/биржа/host/port/ключ•••/группа). |
| **workspace/** | `Workspace { group, cores, active_core, dock }`, `build_all(config)` (группировка по `group`). |
| **window/** | `WindowHost` — одно ОС-окно = одна группа: свой surface+egui+chart+dock+workspace, dirty/skip-present, `needs_render`. |
| `app/mod.rs` | `App` — менеджер окон: маршрутизация событий по `WindowId`, рендер-цикл, пересоздание окон при Save, владелец Settings. |
| **shaders/** | `common.wgsl` (склеивается) + `grid/crosses/glass/cursor.wgsl`. |

---

## 3. Окна по группам

- `Workspace::build_all(config)` группирует серверы по полю `group` → список
  workspace (= групп).
- `App::build_windows` создаёт **`WindowHost` на каждую группу** (свой
  `winit::Window` + surface + egui + `Chart` + `Dock`). Нет серверов → одно пустое
  окно (чтобы открыть Настройки).
- Заголовок окна = имя группы. Закрыли все окна → выход. Save в настройках →
  сессии и окна пересоздаются по новым группам.
- Внутри группы с несколькими ядрами активное ядро выбирается (для чарта); ордера
  в нижнем доке агрегируются по **всем** ядрам группы.

---

## 4. Раскладка окна (Dock)

```
[ header: рынок · цена · статус · ⚙ ]
[ toolbar: размеры ордера · масштаб(Auto/S1..) · детекты · стратегии/справка/отчёты ]
[  график + стакан (wgpu)        | панель ордера: BUY/SELL/Cancel/Panic ]
[ нижний док: открытые ордера группы (resizable, на всю ширину) ]
[ status bar ]
```

- Центральный прямоугольник под график берётся из `CentralPanel` egui (точки →
  пиксели), поэтому панели реально «отъедают» место.
- Нижний док — таблица: **Ядро · Сторона(LONG/SHORT) · Токен · Size · SL · TS ·
  VStop · Buy · Цена · Fill · Strat** (Strat = тип стратегии, не число).

---

## 5. Рендер-модель (важно для GPU)

- Все окна обслуживает **один winit event loop**; рендер вызывается напрямую из
  `about_to_wait` (не зависим от фокуса/`RedrawRequested`).
- **Dirty + skip-present**: окно рисуется и презентится **только если** у его
  ядра изменились тики/стакан/ордера (`*_rev`) или был ввод/UI-событие. Иначе
  кадр пропускается → idle ≈ 0% GPU.
- График **tick-driven**: правый край следует за временем последнего тика
  (`last_tick_ms`), а не за стенными часами. Нет тиков → нет движения → нет GPU.
- Цикл усыплён `ControlFlow::WaitUntil(~8 мс)` (нет busy-spin), пробуждается по
  событию.
- Слои графика: фон+сетка / крестики (instanced) / стакан / курсор, scissor по
  зонам. Сейчас это **direct-draw** (рисуется видимый набор каждый грязный кадр) —
  bitmap-canvas + UV-scroll (idle/cheap-frame) пока НЕ сделаны (см. §8).

---

## 6. Подписки (следуют за открытым чартом)

- Ядро всегда подключено (статус, ордера). Подписка на **trades/orderbook**
  включается, только когда чарт ядра открыт.
- `App` каждый цикл считает набор «ядер с открытым чартом» (активное ядро каждого
  окна) → `SessionManager::set_charted` → шлёт `CoreCmd::Subscribe/Unsubscribe`.
- Закрыли окно → ядро отписывается, поток данных с него прекращается.

---

## 7. Конфиг и секреты

- `config.enc` рядом с exe. Содержимое (toml) шифруется **AES-256-GCM**; ключ —
  случайный, хранится в **OS keyring** (Win Credential Manager / macOS Keychain).
  Без пароля, кросс-платформенно, читается на старте.
- Ключи в UI маскируются (`Secret`), в памяти затираются (`zeroize`).
- Старый `config.toml` (если есть) один раз мигрирует в `config.enc`.

---

## 8. Что НЕ сделано / отклонения от UI-ТЗ

(см. `docs/docs__TERMINAL_UI_TZ.md`)

- **Settings — `egui::Window` в окне, а не отдельное native ОС-окно** (ТЗ требует
  отдельное окно). Долг.
- **Нет GPU effects-слоя** (ripple/glow/flash) — `ui_effects` не реализован.
- **Нет окна `UI Kit`** (таблица маркетов с cell-flash, отчёты 10k virtualized,
  date/time picker, context menu, popup, loading, progress).
- **Нет cell-flash** в таблицах.
- Окна с **системной рамкой** (а не свой тонкий borderless shell).
- **Bitmap-canvas + UV-scroll + 7-слойная dirty-архитектура** из
  `docs__CHART_ARCHITECTURE.md` — пока direct-draw; крестики рисуются набором, не
  кэш-битмапом.
- **Торговля не подключена**: кнопки BUY/SELL/Panic/Cancel логируют; реальные
  команды через `client.trade()/orders()` + guard-ы — следующий шаг (расширить
  `CoreCmd` торговыми намерениями).
- **Детекты** в тулбаре — плейсхолдер (не читаем `DetectEvent`).

---

## 9. Дальнейшие шаги (ориентир)

1. Торговая точка входа: расширить `CoreCmd` (NewOrder/Cancel/Move/Panic/…),
   исполнять в backend через `client.trade()/orders()`, guard + dry-run, `uid` в
   `OrderRow`, результат назад в UI.
2. Детекты live: читать `DetectEvent`, кнопки детектов, клик → рынок/ядро.
3. Чарт П2: рисовать только видимое / bitmap-canvas + UV-scroll (cheap frame).
4. UI-долги по ТЗ: native Settings-окно, GPU effects layer, UI Kit demo,
   cell-flash, borderless shell.
