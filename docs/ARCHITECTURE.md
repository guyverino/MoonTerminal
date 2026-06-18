# MoonTerminal Architecture

Дата актуализации: 2026-06-18.

Этот документ описывает текущую публичную архитектуру. Старые планы про отдельный `ZedFork`,
`MoonPalette`, upstream PR в Zed и `request_continuous_presentation` больше не являются целевым
путём этого репозитория.

## Состав

- `moon-core` — UI-независимое ядро: подключения, конфиг, сессии, market state, отчёты.
- `moon-chart` — математика чарта: time/price view, phase-clean default scale, pan/zoom, оси.
- `moon-ui-gpui` — бинарь `moonterminal`: GPUI shell, панели, debug-tools, chart integration.
- `Moonbot-Tech/MoonUI` — внешний git dependency: standalone GPUI runtime + Moon UI components.

`MoonPalette` — архивный/legacy путь. Новые зависимости на него добавлять нельзя.

## Рендер

Чарт рисуется через GPU own-pass поверх MoonUI/GPUI:

- Windows: DX11/HLSL.
- macOS: Metal/MSL.
- Linux: нативный GPUI wgpu backend/WGSL.

Это не старый `egui + wgpu offscreen + readback` и не shared-texture bridge между разными
рендерерами. CPU readback для живого чарта не используется.

Ключевой контракт: график сам принимает решение, нужен ли кадр (`gpu_canvas.frame()`), и может
подготовить данные к этому же кадру без top-down `cx.notify()` всего окна. Shell/Orders не должны
перерисовываться на частоте live-scroll, mousemove или present.

## Data Path

Текущий live-path ушёл от старого постоянного polling и top-down переноса chart data:

- MoonProto события приходят через event sink с waker.
- Backend loop ждёт реальные события/команды через waker, а не будится таймером.
- Видимый chart подтягивает market data через `MarketDataSource` внутри `gpu_canvas.frame()`.
- `MarketDataSource` читает `snapshot_versioned()` и двигает consumer cursor только для реально
  видимого chart path.
- `SharedMarketStore` остаётся core-owned совместимым read-model для остальных потребителей; это
  не GPUI entity и не причина top-down render.

Push-события остаются для UI-виджетов и редких уведомлений. Chart data path — pull на frame tick.

## Окна

Терминал использует собственную шапку и borderless/CSD поведение. Проверять отдельно:

- Windows: restore bounds на multi-monitor/DPI.
- macOS: Metal toolchain и `.app` запуск из GUI session.
- Linux X11/Wayland: отсутствие второй системной шапки, Secret Service для encrypted config,
  стабильность surface/present.

## Локальная Разработка

Публичные `Cargo.toml` держат git-зависимости на `Moonbot-Tech/MoonUI` `branch = "master"`.
`Cargo.lock` в терминале не коммитится намеренно: dev-ветка живёт в режиме rolling integration,
чтобы свежий checkout собирался против актуального MoonUI master. Это упрощает синхронную работу
терминала и компонентов, но не является strict reproducible build по дате.

Каждый бинарь пишет в лог build stamp:

```text
build: moonterminal=<git-sha>[+dirty] moonui=<git-sha|local:git-sha>[+dirty]
```

Для release/stabilization можно отдельно зафиксировать tag/rev или вернуть коммитимый lock; сейчас
это сознательно не делается.

Для локальной разработки рядом должны лежать:

```text
workspace/
  MoonTerminal/
  MoonUI/
```

Локальная подмена делается только в ignored `MoonTerminal/.cargo/config.toml`:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }
```

Не использовать top-level `paths`: он меняет форму dependency graph и уже сейчас даёт Cargo warning,
который в будущих версиях Cargo может стать ошибкой.
