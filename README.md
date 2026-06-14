# MoonTerminal

Кросс-десктопный трейдинговый терминал для ядер **MoonBot**.

Текущая рабочая оболочка разработки — `crates/moon-ui-gpui`: **GPUI +
MoonPalette + chartdx own-pass renderer**. Корневой пакет `moon-terminal` со
старой `egui`/`winit` оболочкой остаётся в workspace как legacy-ветка кода, но
не является тем бинарём, который сейчас приводится к MoonBot-дизайну.

Один терминал обслуживает **несколько ядер сразу**. Ядра группируются, и каждая
группа — это **отдельное ОС-окно** со своей раскладкой. Маркет-данные (трейды +
стакан) дедуплицируются: на каждую биржу подписку держит **одно выбранное ядро**,
а аккаунт-данные (ордера/детекты/стратегии) читаются по каждому ядру отдельно.

Режим один — **live** (синтетики нет).

## Запуск текущей GPUI-оболочки

**Windows toolchain: MSVC, не GNU.** Собираем таргетом
`x86_64-pc-windows-msvc`: его ожидают GPUI, DirectX/DWrite/DComp и наш
`chartdx` GPU-pass. GNU-таргет (`*-windows-gnu`) для текущей GPUI-оболочки не
используем.

Требования:
- **Rust** с MSVC standard library для `x86_64-pc-windows-msvc`.
- **Visual Studio Build Tools 2022**, компонент *«Разработка на C++ для настольных систем»*
  — даёт `link.exe`, `lib.exe`, `ml64.exe` и Windows SDK. Полная Visual Studio
  не нужна, хватает Build Tools.

```powershell
cd R:\test\MoonTerminal

$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && `"C:\files\utils\rust\cargo\bin\cargo.exe`" build -p moon-ui-gpui --bin moon-gpui --target x86_64-pc-windows-msvc"

.\target\x86_64-pc-windows-msvc\debug\moon-gpui.exe
```

Можно запускать одной командой:

```powershell
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && `"C:\files\utils\rust\cargo\bin\cargo.exe`" run -p moon-ui-gpui --bin moon-gpui --target x86_64-pc-windows-msvc"
```

Важно: `target\debug\moon-gpui.exe` и
`target\x86_64-pc-windows-msvc\debug\moon-gpui.exe` — разные output-директории.
Если собирали с явным `--target x86_64-pc-windows-msvc`, запускать нужно именно
`target\x86_64-pc-windows-msvc\debug\moon-gpui.exe`; другой exe может быть
старым.

### Runtime на чистой Windows

MSVC-сборка не вшивает системные DLL в exe. На Windows 10/11 обычно уже есть
UCRT и графический стек (`d3d11.dll`, `dxgi.dll`, `dwrite.dll`, `dcomp.dll`), но
`dumpbin /dependents` для текущего `moon-gpui.exe` также показывает
`VCRUNTIME140.dll`. Для чистой машины ставим или кладём рядом с инсталлятором
**Microsoft Visual C++ Redistributable 2015-2022 x64**.

Корневой legacy-бинарь собирается отдельно:

```powershell
cargo run -p moon-terminal
```

Сервера (ядра) добавляются прямо в приложении: **⚙ Настройки → Подключения**
(имя/биржа/host/port, ключ скрыт, фид-фильтры, группа, цвет). Конфиг с ключами
шифруется при сохранении (`servers.enc`, AES-256-GCM, ключ — в OS keyring);
открытые настройки (язык, режим маркет-данных) лежат в `settings.toml`, тема
графика — в переносимом `theme.toml`.

> `servers.enc` / `settings.toml` в `.gitignore` — ключи и приватные настройки в
> git не попадают. Старые `config.enc` / `config.toml` при наличии один раз
> мигрируют в новый формат.

## Языки

Интерфейс на **RU / EN / ES** (`rust-i18n`, строки в `locales/app.yml`).
По умолчанию — язык системы; переключается в **Настройки → Общие**. Трейдинговые
термины и метрики (BUY/SELL/LONG/SHORT/Size/SL/TS, ticks/fps/CPU/RAM) намеренно
остаются английскими.

## Архитектура (кратко)

```
servers.enc ─decrypt(keyring+AES)→ AppConfig.servers ─group→ окна по группам
        │
        ▼  (поток на ядро, live/moonproto)
SessionManager ──FeedMsg──▶ CoreStore (аккаунт) + MarketStore (дедуп трейды/стакан)
        ▲                                    │
        └─────── CoreCmd::SetMarket ◀────────┤  (coordinator: выбор провайдера)
                                             ▼
WindowManager(App) ── WindowHost(группа) = Chart(wgpu) + Shell + Dock + Workspace
                   ├─ SettingsWindow (нативное окно, 3 вкладки)
                   └─ ReportsWindow  (нативное окно, SQLite-отчёты)
```

UI **никогда** не зовёт moonproto напрямую — только читает `FeedMsg` из канала и
шлёт `CoreCmd`. Подробности — в [docs/ARCHITECTURE_MULTICORE.md](docs/ARCHITECTURE_MULTICORE.md).

## Структура файлов

```
src/
  main.rs               точка входа: логгер, конфиг, i18n-локаль, taskbar, event loop
  applog.rs             файловый лог сырых SQL-команд отчётов от ядра
  icons.rs              загрузка PNG-иконок из assets/icons (egui + winit)
  metrics.rs            CPU/RAM процесса и системы для статус-бара (sysinfo)
  symbol.rs             разбор тикера (котируемая валюта, базовый символ)
  win_taskbar.rs        Windows: AppUserModelID на окно (раздельные кнопки taskbar)
  config/
    mod.rs              AppConfig { servers, groups, language, market_mode, theme }
    schema.rs           дисковый формат (serde), SCHEMA_VERSION
    store.rs            I/O: шифр servers.enc, открытый settings.toml, .bak при порче
    servers.rs          ServerConfig + FeedFlags (orders/detects/reports/…)
    secrets.rs          Secret: маска в UI, zeroize в памяти
    crypto.rs           AES-256-GCM, ключ в OS keyring
    paths.rs            пути servers.enc/settings.toml/theme.toml/reports.db рядом с exe
    migrate.rs          разовая миграция legacy config.enc/config.toml
    reconcile.rs        слияние ServersFile+SettingsFile → runtime, стабильные uid
    groups.rs           GroupConfig { name, active, icon }
    lang.rs             Language Ru/En/Es + дефолт по системе
    theme.rs            ChartTheme (цвета графика) ↔ переносимый theme.toml
  feed/
    mod.rs              FeedHandle (канал + cmd_tx), CoreCmd, spawn(server)
    types.rs            FeedMsg, Tick/Side/Level/OrderBook/OrderRow/DetectRow/StrategyRow
    live.rs             единственный, кто знает moonproto: connect, подписки, чтение
  session/
    mod.rs              SessionManager: поток на ядро, drain(), market_view()
    store.rs            CoreStore/CoreData (status/orders/detects/strategies + *_rev)
    coordinator.rs      выбор провайдера на биржу (Dedup/PerCore), set_open(), linger
  market/mod.rs         MarketStore: дедуп трейды/стакан по провайдеру (Dedup/PerCore)
  gpu/mod.rs            GpuContext (wgpu instance/adapter/device/queue/surface)
  chart/
    mod.rs              Chart: слои + canvas, render(rect, data), перезалив по *_rev
    canvas.rs           offscreen-битмап крестиков + UV-scroll (bake/append/composite)
    view.rs             ChartView: follow/fit_price/zoom/pan + ChartUniform
    transform.rs        ChartUniform + bind group (time,price)→px→clip
    style.rs            StyleUniform (тема как GPU-uniform, group 1)
    data/
      tick_ring.rs      semantic ring тиков (SoA)
      orderbook.rs      модель стакана: кумулятивная глубина + линии уровней
    layers/
      mod.rs            хелперы пайплайнов, InstanceBuf
      grid.rs           слой 1 — фон+сетка
      crosses.rs        слой 2 — крестики (инстансы, пекутся в canvas)
      glass.rs          слой 3 — стакан (фон + бары/линии)
      cursor.rs         слой 4 — курсор-перекрестие + halo
  shell/
    mod.rs              egui: header (рынок/цена/статус/⚙) + статус-бар с метриками
    theme.rs            стиль egui, шрифт Geist Mono, кнопки/пилюли
    brand.rs            SVG-лого MoonBot → текстура egui (resvg)
  dock/
    mod.rs              Dock: тулбар + панель ордера + нижний док + лента детектов
    toolbar.rs          размеры/масштаб/live-follow/детекты/стратегии/справка/отчёты
    controls.rs         OrderControls (5 размеров, пресеты масштаба)
    order.rs            панель BUY/SELL/Cancel/Panic (пока лог, без торговли)
    orders_panel.rs     нижний док: таблица открытых ордеров группы
    detects.rs          лента детектов (live, TTL, клик → чарт рынка)
    close_btn.rs        кнопка закрытия чарта
  settings/
    mod.rs              SettingsState + трейт SettingsTab (draft, Save)
    connections.rs      вкладка Подключения: сервера + группы + иконки
    general.rs          вкладка Общие: язык
    interface.rs        вкладка Интерфейс: живой редактор темы графика
  window/
    mod.rs              реэкспорт WindowHost/SettingsWindow/ReportsWindow
    host.rs             WindowHost — одно ОС-окно = группа (surface+egui+chart+dock)
    settings_window.rs  отдельное нативное окно настроек
    reports_window.rs   отдельное нативное окно отчётов (SQLite, фильтры/сорт)
  workspace/mod.rs      Workspace { group, cores, open_chart, dock }, build_all(config)
  db/
    mod.rs              SQLite-отчёты по закрытым ордерам (writer-поток, UPSERT)
    parse.rs            парсер report-SQL (INSERT/UPDATE) → структуру
  app/mod.rs            App — менеджер окон: маршрутизация по WindowId, рендер-цикл
shaders/                grid/crosses/canvas/glass/glass_bg/cursor.wgsl (+ common.wgsl)
locales/app.yml         RU/EN/ES строки интерфейса (rust-i18n)
assets/                 icons/*.png, brand/moonbot-logo.svg
```

## Статус

Сделано: мультиядро/мультиокно, дедуп маркет-данных, шифр-конфиг, i18n, темы
графика (живой редактор), bitmap-canvas + UV-scroll рендер, метрики CPU/RAM,
живые детекты, локальная БД отчётов с отдельным окном.

Не сделано: **торговля не подключена** (кнопки BUY/SELL/Panic/Cancel логируют);
GPU effects-слой (ripple/glow/flash); borderless-shell окна; cell-flash в
таблицах. Полный список и долги — в §8 архитектурного доку.
