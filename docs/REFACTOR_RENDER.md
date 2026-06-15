# Рефакторинг рисования: было → стало

Что изменилось в рисовании чарта (и в репозитории) против состояния ДО начала работ.
Кратко, по сути. Деталь плана недоделок — `RENDER_PLAN.md`.

Проверка: `cargo check -p moon-ui-gpui` проходит 2026-06-15 через MSVC `vcvars64` после MoonTerminal-правок
и адаптации к PR-grade GPUI hook (`Subscription` + callback `Result`, без `set_present_sync_interval`).
Независимая проверка `cargo check -p moon-core -p moon-chart` после финальной правки стакана — зелёная.

## Суть в одну строку

Чарт рисовался wgpu-движком в offscreen-текстуру с **readback на CPU** каждый кадр (дорого,
GPU↔CPU столл) → теперь его рисует **own-pass DX11 прямо в backbuffer GPUI** через generic-хук,
**без readback и без wgpu**.

## Трасса состояний

**1. ДО (egui-бинарь `moon-terminal`, корневой `src/`)** — удалён.
- Оболочка: `egui` + `winit`, один surface egui+wgpu на окно.
- Чарт: wgpu-движок (`moon-chart` `Chart`) рисует слои (grid/crosses/glass/cursor/order_lines)
  в wgpu `TextureView`; кресты — bitmap-canvas + UV-scroll; темы — `StyleUniform`.
- Оси/текст: egui-painter.

**2. Промежуточное (GPUI-порт, readback-эра)** — пройдено, выпилено (коммиты `edcb5ac`
«extract moon-chart», `d99a0c4` «variant A offscreen→readback», `53c82f9`, `34a2fdb`).
- Оболочка переехала на GPUI, но чарт всё ещё рисовал тот же wgpu-движок `moon-chart` в
  **offscreen-текстуру → readback на CPU → загрузка как GPUI-текстуры** каждый кадр.
- Здесь и «споткнулись»: readback — это синхронный GPU→CPU столл, рывки вплоть до курсора.

**3. ПОСЛЕ (own-pass, текущее)** — «правильные методы».
- Оболочка: GPUI на **MoonPalette** (форк gpui-component, `MoonBackgroundPolicy::NoFill` →
  прозрачный регион чарта).
- Чарт: **НАШ own-pass DX11** (`crates/moon-ui-gpui/src/chartdx/`) рисует слои **прямо в
  backbuffer GPUI** через generic-хук `RawGpuAccess`, фаза `GpuPhase::UnderScene` — **без
  readback, без wgpu**. Слои по природе данных: `combo` (история трейдов, bake+UV-pan) /
  `orderbook` (стакан) / `userdata` (ордера) / `grid` + scissor по зоне панели.
- Оси/текст/крестик: GPUI-оверлей поверх (нативный субпиксельный текст).
- `moon-chart` низведён до **wgpu-free библиотеки** математики/геометрии (view/axes/transform/
  build_order_geometry/инстанс-типы/константы).

## Что выпилено из репозитория (эта чистка)

- **Удалён бинарь `moon-terminal`**: корневой `src/` (egui app: app/chart/dock/shell/settings/…),
  `build.rs` (иконка exe старого бинаря).
- **Корневой `Cargo.toml` → virtual workspace**: без `[package]`, без egui/wgpu/winit/egui-*/
  resvg/rust-i18n/raw-window-handle. Остались `[workspace]` + `[patch]` (GPUI) + `[profile]`.
- **`moon-gpui`**: убраны мёртвые `wgpu`, `pollster` (наследие readback-эры; в коде 0 употреблений).
- **`moon-chart`**: вырезан wgpu-движок — `Chart`, `canvas.rs`, `style.rs`, wgpu-слои
  `layers/{crosses,cursor,glass,grid}.rs`, wgpu-часть `order_lines.rs`/`transform.rs`/`container.rs`/
  `paint.rs`, весь `shaders/*.wgsl`. Крейт стал **wgpu-free** (зависимость на `wgpu` убрана).
- **Единственный бинарь теперь — `moon-gpui`** (`crates/moon-ui-gpui`).
- **Удалён halo/glow вокруг GPUI-крестика**: оставлена только функциональная тонкая вертикаль/горизонталь.
  В own-pass cursor это было визуальным хвостом без понятной пользы; переносить/имитировать в GPUI не стали.

## Что НЕ менялось / что уже пришлось тронуть

- Основная архитектура `moon-core` (session/coordinator/config/db, дедуп маркет-данных через провайдера,
  мультиядро/мультиокно, шифр-конфиг, отчёты SQLite) не менялась.
- Но render-data контур уже расширен: `TickRing`/стакан получили CPU-culling/исправление signed span,
  `feed/live.rs` протаскивает серверную трассу buy/sell и поля зон, `OrderLineStore` хранит server
  trace + zone flags, `orders_rev` участвует в render signature.
- Математика вида (`moon-chart::view::ChartView`) уже изменена под GPUI-only: spatial `Latest`
  вместо 3-сек hold, anchor zoom под курсором, дискретный wheel, Y-scale snap, дефолтный smooth-scale
  под фактический present-rate.

## Незакрытое (см. `RENDER_PLAN.md`)

Открыто/отложено теперь не “базовые баги”, а внешние контракты и поздняя оптимизация:
визуальный live-GUI прогон, phase staggering + UI-toggle fast/slow, per-figure culling для будущих
ChartObj, ChartObj store/API, дополнительные price-series без текущего источника (`avg/closest/wavg/OI/min/max/liq`),
sell-shot zone без явного price-range в read-model, Metal/wgpu renderer самого MoonTerminal. Базовые #1-#3,
data_signature, mouse/follow, серверная трасса buy/sell, zones, shader-extend, Volume, retained
`LastPrice`/`MarkPrice`, background photo layer, own-pass cursor и halo уже закрыты.
