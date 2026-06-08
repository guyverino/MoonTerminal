# MoonTerminal

Кросс-десктопный трейдинговый терминал: **график + стакан на `wgpu`**, оболочка на
**`egui`**, поток данных от ядра MoonBot через **MoonProtoBeta**.

Первый срез: окно с графиком тиков (крестики) и стаканом по рынку `BTCUSDT`,
рисование через GPU, фид из ядра в отдельном потоке.

## Запуск

Режим один — **live** (подключение к ядру MoonBot).

```powershell
cargo run
```

Сервера добавляются в приложении: **⚙ Настройки → Подключения** (ключи скрыты,
`config.enc` шифруется при сохранении, ключ — в OS keyring). Можно один раз
подсунуть `config.toml` (см. `config.example.toml`) — он мигрирует в `config.enc`.

> `config.toml` / `config.enc` в `.gitignore` — ключи в git не попадают.

## Архитектура (как в docs проекта)

```
feed (отдельный поток)            UI / render поток
┌──────────────────┐  FeedMsg     ┌───────────────────────────┐
│ live (moonproto) │ ──канал───▶ │ App (winit)               │
│  на ядро          │             │  ├─ GpuContext (wgpu)     │
└──────────────────┘             │  ├─ Chart (слои)          │
                                  │  │   grid/crosses/glass/  │
                                  │  │   cursor               │
                                  │  └─ Shell (egui)          │
                                  └───────────────────────────┘
```

UI **никогда** не зовёт moonproto напрямую — только читает `FeedMsg` из канала
(см. handoff-доки про MoonKernel facade).

## Структура файлов

```
src/
  main.rs                 точка входа: конфиг, backend, event loop
  config.rs               Config (key/host/port/market/source)
  feed/
    mod.rs                канал backend→ui, выбор бэкенда
    types.rs              Tick/Side/Level/OrderBook/ConnStatus/FeedMsg
    live.rs               moonproto-интеграция
  gpu/mod.rs              инициализация wgpu
  chart/
    mod.rs                сборка слоёв + рендер кадра
    view.rs               зум/пан/follow + GPU-uniform
    transform.rs          ChartUniform + bind group
    data/
      tick_ring.rs        semantic ring тиков
      orderbook.rs        модель стакана
    layers/
      mod.rs              хелперы пайплайнов
      grid.rs             слой 1 — фон+сетка
      crosses.rs          слой 2 — крестики
      glass.rs            слой 4 — стакан
      cursor.rs           слой 7 — курсор
  shell/
    mod.rs                egui: header + status
    theme.rs              стили (отдельно)
shaders/                  *.wgsl (common склеивается через concat!)
```

## Что это НЕ (пока)

Первый срез использует **direct-draw** путь (instanced крестики + pan через uniform),
проверенный в стенде. Целевые **bitmap-canvas + UV-scroll + idle-0%-GPU** из
`docs__CHART_RENDERING_TZ.md` — следующий milestone, архитектура слоёв уже под это
заложена (отдельные `Layer`, dirty-флаги, общий uniform).
