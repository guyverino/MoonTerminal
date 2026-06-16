# OBSOLETE: GPUI fork -> Zed PR handoff

This document describes the old raw `add_gpu_pass` /
`request_continuous_presentation` direction. Do not use it as the target PR
plan. The current draft path is `docs/GPUI_GPU_CANVAS_FORK_PLAN.md`:
`gpu_canvas`, an immediate-mode GPU drawing element with pre-present frame
decision.

Update 2026-06-15: upstream PR work is intentionally paused. Current team delivery uses
`Moonbot-Tech/ZedFork:master` at `ffa53b323ce6294ea13395cabeb1abe59c0baece` as a clean public
dependency repo, not a GitHub fork-network PR source. For the real Zed PR, create a proper
GitHub fork later and port/cherry-pick the relevant commits again.

Док для следующего чата/агента, который будет доводить форк GPUI до состояния upstream PR в
`zed-industries/zed`.

Цель документа: собрать всё, что уже поняли по стендам, форку, быстрым графикам и PR-рискам.
Код тут не пишем. Код будет писать отдельный агент. Здесь — факты, выводы, критерии качества и
конкретные проблемы текущего патча.

## TL;DR

Нам нужен не "форк ради форка", а upstream-capability:

> GPUI должен позволять приложению встроить свой zero-copy GPU renderer внутрь кадра GPUI:
> под сценой, над сценой или в другой явно описанной фазе, сохранив нормальный UI/text overlay.

Без этого быстрый тиковый график внутри GPUI-окна сваливается в один из плохих вариантов:

- рисовать 100k+ объектов GPUI primitives каждый кадр — слишком дорого;
- offscreen GPU -> CPU readback -> GPUI image — плохая задержка, лишняя шина, плохой scaling на много графиков;
- отдельная child surface (`MTKView`/Vulkan/wgpu surface) — быстро, но ломает композицию, overlay, clipping, input, z-order, HiDPI, popups;
- форк/официальный hook — правильный путь: UI остаётся GPUI, chart renderer рисует напрямую в тот же кадр.

Текущий патч уже доказал идею на Windows/DX11: маленький diff, чарт рисуется через own-pass, без readback.
До зачистки 2026-06-15 это был лабораторный hook. Сейчас hook доведён до cross-backend PR-grade формы:
D3D11 + Metal + wgpu. Минимальный `gpui` example добавлен; перед PR остались GitHub fork-network
переезд и согласованный PR text.

## Актуальный статус 2026-06-15

- PR-worktree: `R:/test/chart-test/stand-gpui/gpui-fork-pr`.
- Branch: `codex/gpui-gpu-pass-hook-pr`.
- Base: `zed-industries/zed` main = `f39cf25c0ba571eaaa7a21f3266c8a356367fa5f`.
- Актуальный pushed commit: `f2f07f3069 Add custom GPU pass example`.
- Сделано в коде форка:
  - `add_gpu_pass` возвращает `Subscription`, drop отписывает pass;
  - callback signature = `FnMut(&RawGpuAccess) -> anyhow::Result<()>`;
  - `RawGpuAccess` получил `device_generation`, `command_buffer`, `command_encoder`,
    `render_target_format`, `render_target_format_ptr`;
  - `GpuBackend::{D3D11, Metal, Wgpu}` соответствуют реальным implementation;
  - D3D11 renderer increment'ит generation после device/resource recreation;
  - wgpu renderer increment'ит generation на `recover()` и сохраняет pass callbacks;
  - вокруг callback добавлен `D3d11StateGuard` для GPUI-owned OM/RS/IA/VS/PS state;
  - Metal callback вызывается в isolated `MTLRenderCommandEncoder` с `Load`;
  - wgpu callback получает native `Device/Queue/CommandEncoder/TextureView/TextureFormat`;
  - MacWindow, Linux X11 и Linux Wayland реально forward'ят `add_gpu_pass`;
  - `set_present_sync_interval` удалён из PR-surface;
  - unsupported platforms без backend implementation возвращают `Err`, а не silent no-op;
  - vendor/русские comments вычищены из изменённых GPUI sources.
  - минимальный example добавлен: `crates/gpui/examples/gpu_pass.rs`.
- Проверки:
  - `rustfmt` на изменённых backend files — OK;
  - `cargo check -p gpui -p gpui_windows -p gpui_wgpu` — OK;
  - `cargo check -p gpui_linux` — OK;
  - `cargo check -p gpui_macos` — OK.
  - `cargo check -p gpui --example gpu_pass` — OK.

## Где лежит контекст

- Локальный стенд GPUI:
  - `R:/test/chart-test/stand-gpui`
- Большой GPUI/Zed fork:
  - PR-source: `R:/test/chart-test/stand-gpui/gpui-fork-pr`
  - remote: `git@github.com:Moonbot-Tech/ZedFork.git`
  - branch: `codex/gpui-gpu-pass-hook-pr`
  - base: `f39cf25c0ba571eaaa7a21f3266c8a356367fa5f`
  - old local prototype `R:/test/chart-test/stand-gpui/gpui-fork` больше не считать PR-source.
- Старый промежуточный fork crates.io `gpui 0.2.2`:
  - `R:/test/chart-test/stand-gpui/gpui-0.2.2-fork`
  - без `.git`, просто локальная копия с наложенным hook;
  - полезен как доказательство, но сейчас не основная линия.
- GPUI chart bench:
  - `R:/test/chart-test/stand-gpui/vendor/gpui-component/examples/chart_bench`
  - главный app-side renderer: `src/chart_gpu.rs`
  - shaders: `crosses.hlsl`, `blit.hlsl`
- Документы стенда:
  - `R:/test/chart-test/stand-gpui/TERMINAL_WORKLOG.md`
  - `R:/test/chart-test/stand-gpui/CHART_BENCH_JOURNAL.md`
  - `R:/test/chart-test/stand-gpui/TERMINAL_RENDER_ARCHITECTURE.md`
  - `R:/test/chart-test/stand-gpui/COMBO_MULTICHART.md`
  - `R:/test/chart-test/stand-gpui/FORK_DIFF.md`
  - `R:/test/chart-test/stand-gpui/FORK_PLAN.md` — устаревший/отвергнутый путь shared texture bridge.
- Текущий терминал:
  - `R:/test/MoonTerminal`
  - ветка на момент аудита: `feat/gpui-shell`
  - текущая архитектура рендера: `docs/RENDER_PLAN.md`
  - текущая общая архитектура: `docs/ARCHITECTURE_MULTICORE.md`
  - важно: `Cargo.toml` и локальный `.cargo/config.toml` уже переключены на
    `ZedFork@f2f07f30692dcc1d3791acc454f4822ac81eaf9c` / `gpui-fork-pr`.
  - отдельный delivery-блокер терминала: `MoonPalette` пока остаётся локальным path dependency, потому
    что `Moonbot-Tech/MoonPalette` не найден через `git ls-remote`.
  - отдельный PR-блокер: `Moonbot-Tech/ZedFork` сейчас `fork=false`; GitHub compare из
    `zed-industries/zed` в `Moonbot-Tech:codex/gpui-gpu-pass-hook-pr` возвращает `404`. Для upstream
    PR нужен настоящий GitHub fork-network repo.

## Что мы тестили

### 1. Наивный GPUI primitives

Идея: каждый кадр строить сцену GPUI из большого числа объектов.

Вывод: мёртвый путь для нашей задачи.

По журналам `CHART_BENCH_JOURNAL.md`:

- `ReEmitAll`: 100k крестов через GPUI primitives даёт порядка 15-19 fps;
- build cross primitives сам по себе дорогой;
- live append stress доходил до сотен миллисекунд paint/build на большой сцене.

Почему плохо:

- GPUI как UI framework прекрасен для UI/text/layout;
- но 100k биржевых маркеров каждый кадр — это data visualization workload, не widget workload;
- UI-layer не должен быть chart renderer.

### 2. CPU canvas / whole texture upload

Идея: рисовать bitmap на CPU/в offscreen, потом отдавать в GPUI как image/texture.

Вывод: лучше наивного primitives, но не годится как главный путь терминала.

Почему:

- появляется постоянная цена копии/заливки;
- на 4K/много окон/много графиков шина и upload растут быстро;
- zoom/pan/rebake превращаются в постоянную работу;
- главное: это архитектурно не тот hot path, который нужен скальперскому терминалу.

### 3. `wgpu offscreen -> readback -> gpui img()` в живом терминале

Это был путь в терминале друга до own-pass.

Вывод: плохой путь.

Симптомы:

- readback дорогой;
- FPS/latency ограничены;
- RAM/буферы раздуваются;
- любые 10/40/100 chart viewport начинают масштабировать боль.

Это важнейший аргумент для PR: нам нужен не "нарисовать картинку и засунуть её в UI", а zero-copy
custom render pass в кадре UI.

### 4. Direct GPU instanced renderer через GPUI fork

Идея: GPUI отдаёт приложению raw access к backbuffer phase; приложение само рисует chart primitives
через D3D11.

Результат:

- 100k/120k видимых крестов: сотни fps uncapped;
- bus ~0;
- cursor cost ~0.01-0.02 ms;
- доказано, что bottleneck readback/primitives уходит.

Вывод:

- направление правильное;
- hook работает;
- доменная логика чарта может жить полностью в приложении, не в GPUI.

### 5. Combo renderer

Идея: для одного графика direct instanced уже хорош, но для 10/100 графиков нельзя каждый vsync
перерастривать всю историю. Поэтому:

- статичная история печётся в offscreen texture;
- live edge/appends дорисовываются инкрементально;
- pan делается UV-сдвигом;
- GPUI text/UI overlay остаётся сверху.

Результат из журнала:

- Combo даёт примерно в 3 раза ниже GPU load даже на одном графике;
- для многооконного/многографикового терминала это главный путь;
- это не "bitmap вместо данных", потому что данные всё равно семантические, а bitmap — cache слоя истории.

## Почему потребовался форк

Потому что GPUI renderer владеет native GPU frame.

На Windows:

- GPUI владеет `IDXGISwapChain`;
- создаёт/держит `ID3D11Device`, `ID3D11DeviceContext`, `ID3D11RenderTargetView`;
- сам делает `pre_draw`, scene batches, `Present`.

Без hook приложение не может встроить свой chart renderer в правильную фазу кадра.

На macOS:

- GPUI владеет `CAMetalLayer`;
- берёт drawable;
- создаёт `MTLCommandBuffer`;
- создаёт `MTLRenderCommandEncoder`;
- рисует scene;
- делает `present_drawable/commit`.

Без hook приложение не может zero-copy рисовать под GPUI UI/text в тот же drawable.

На Linux:

- GPUI использует `gpui_wgpu`;
- владеет `wgpu::Surface`, `Device`, `Queue`, surface texture, command encoder/pass;
- Wayland/X11 window просто вызывает `renderer.draw(scene)`.

Без hook приложение не получает `Device/Queue/Encoder/TextureView` нужного кадра.

Итого:

- быстрый график как standalone можно сделать без форка на всех платформах;
- быстрый график внутри GPUI-композиции без readback требует либо fork, либо upstream API;
- child native surface технически возможна, но это плохая продуктовая архитектура для нашего UI.

## Что сейчас есть в форке

Diff backend-scoped: public API + D3D11/Metal/wgpu hook plumbing, без chart-specific кода.

Изменённые места:

- `crates/gpui/src/platform.rs`
  - `GpuPhase`
  - `GpuBackend`
  - `RawGpuAccess`
  - default `add_gpu_pass` возвращает explicit unsupported error
- `crates/gpui/src/window.rs`
  - `Window::add_gpu_pass`
- `crates/gpui_windows/src/directx_renderer.rs`
  - хранение `under_scene_passes`, `over_scene_passes`
  - lifecycle через `Subscription`
  - `raw_gpu_access`
  - `run_gpu_passes`
  - `D3d11StateGuard`
  - вызов `UnderScene` перед GPUI scene batches
  - вызов `OverScene` после GPUI scene batches
- `crates/gpui_windows/src/window.rs`
  - forwarding в renderer.
- `crates/gpui_macos/src/metal_renderer.rs`
  - хранение `under_scene_passes`, `over_scene_passes`
  - lifecycle через `Subscription`
  - isolated `MTLRenderCommandEncoder` для UnderScene/OverScene
  - raw Metal handles в `RawGpuAccess`
- `crates/gpui_macos/src/window.rs`
  - forwarding в renderer.
- `crates/gpui_wgpu/src/wgpu_renderer.rs`
  - хранение pass callbacks
  - wgpu-native raw context
  - `device_generation` на `recover()`
- `crates/gpui_linux/src/linux/x11/window.rs`
  - forwarding в `WgpuRenderer`.
- `crates/gpui_linux/src/linux/wayland/window.rs`
  - forwarding в `WgpuRenderer`.

Важно: доменной chart-логики в форке уже нет. Это хорошо.

App-specific chart renderer живёт в bench/app:

- `ChartCross`
- `ChartView`
- `crosses.hlsl`
- `blit.hlsl`
- ring buffer;
- combo texture;
- upload/append/bake/blit.

Это правильное разделение:

- GPUI fork = generic mechanism;
- терминал/app = chart specifics.

## Что в текущем форке уже хорошо

1. Маленький diff.

Это очень важно для upstream. У мейнтейнера должен быть шанс прочитать PR за один заход.

2. Доменной логики в GPUI нет.

Первый вариант форка был "толстым", с chart-specific code. Это было бы мёртвым для PR. Текущий
generic hook намного ближе к тому, что можно предложить Zed.

3. Правильные фазы кадра.

- `UnderScene`: custom renderer рисует под GPUI scene.
- `OverScene`: custom renderer может рисовать поверх GPUI scene.

Для терминала основной путь — `UnderScene`: chart/background/grid/data под UI/text/cursor.

4. На Windows путь уже доказан живым бенчем.

Hook не теория. Он рисовал 100k+ крестов, combo, live data, UI поверх.

## Что в текущем форке плохо для PR

Ниже — исторический список finding-ов ревью. По состоянию 2026-06-15 пункты P0-1/P0-2/P0-3/P0-4/P0-5
и P1-1/P1-2/P1-3/P1-4/P1-5/P2-1/P2-2/P2-3 закрыты в PR-worktree. Остаётся финальный review diff
и согласованный PR description.

### P0. Нет lifecycle/unregister

Сейчас `Window::add_gpu_pass(...)` просто кладёт callback в `Vec` навсегда.

Проблема:

- view/panel/chart может закрыться;
- pass останется жить до закрытия окна;
- callback может держать `Rc/RefCell`/ресурсы;
- для Zed это unacceptable API.

Как должно быть:

- метод должен возвращать guard/subscription/handle;
- при drop handle pass удаляется;
- стиль желательно близкий к GPUI `Subscription`.

### P0. Device lost lifecycle не покрыт для custom pass

GPUI сам после device lost восстанавливает свой renderer.

Windows path:

- VSync thread делает `GetDeviceRemovedReason`;
- при ошибке ждёт ~350ms;
- пересоздаёт `DirectXDevices`;
- шлёт `WM_GPUI_GPU_DEVICE_LOST`;
- renderer выкидывает старые resources/devices;
- пересоздаёт resources/globals/pipelines;
- ставит `skip_draws = true`;
- следующий forced render делает `mark_drawable`.

Но custom pass остаётся зарегистрированным, а app-side renderer может держать старые D3D resources:

- shaders;
- buffers;
- SRV/RTV;
- combo texture;
- constant buffers.

Если после device lost callback получит новый `RawGpuAccess`, а внутри у него старый `pipe`, будет
использование ресурсов старого device с новым context. Это может дать blank/no-op/D3D error/panic.

Как должно быть:

- `GpuPassContext` должен иметь `device_generation: u64`;
- custom renderer должен сбрасывать свои GPU resources при смене generation;
- либо API должен иметь event `DeviceLost/DeviceRecreated`;
- минимум — строгий safety contract в docs, но для хорошего PR лучше generation/event.

### P0. D3D state восстанавливается неполно

Сейчас после callback форк возвращает только:

- render target;
- viewport.

Но callback может поменять:

- rasterizer state;
- scissor;
- depth/stencil;
- input layout;
- vertex/index buffers;
- samplers;
- SRV/UAV slots;
- blend state;
- topology;
- shaders;
- constant buffers.

GPUI перед каждым batch выставляет часть state, но не весь state. Например rasterizer state ставится
один раз при resources init.

Как должно быть:

- либо full D3D11 state guard вокруг callback;
- либо строгий unsafe contract "callback must restore all state";
- для upstream лучше state isolation/guard, хотя он имеет цену.

Компромисс:

- UnderScene before GPUI batches может быть дешевле: после UnderScene GPUI сам выставляет многое.
  Но всё равно rasterizer/scissor/depth остаются риском.
- OverScene после GPUI batches менее опасен для GPUI, но может ломать present/debug/следующий frame,
  если state не восстановлен.

### P1. Callback не возвращает `Result`

Сейчас signature примерно:

```rust
Box<dyn FnMut(&RawGpuAccess)>
```

Плохо:

- custom renderer не может штатно вернуть ошибку;
- остаются panic/log-only/no-op;
- ошибка custom pass не попадает в `DirectXRenderer::draw -> Result`.

Лучше:

```rust
FnMut(&mut GpuPassContext) -> anyhow::Result<()>
```

или GPUI-специфичный `Result`.

### P1. Public API притворялся cross-platform, но реально был Windows-only

Было до правки 2026-06-15:

- Windows реализует;
- macOS no-op;
- Linux no-op;
- `GpuBackend::Metal/Other` есть, но реально не заполнены.

Для PR это плохой smell.

Лучше один из вариантов:

1. Честно Windows-only первый PR:
   - `try_add_gpu_pass` возвращает `Unsupported` на других платформах;
   - docs честно говорят "currently implemented for Windows/D3D11".

2. Сразу backend-aware cross-platform API:
   - Windows: D3D11 context;
   - macOS: Metal context;
   - Linux: wgpu context.

Итоговая правка пошла по варианту 2: D3D11 + Metal + wgpu реализованы реально.

### P1. `set_present_sync_interval` лучше вынести из первого PR

Это отдельная capability, не обязательная для custom GPU pass.

Проблемы:

- Windows/DXGI-specific;
- no-op на других платформах;
- меняет frame pacing semantics;
- добавляет ревью-поверхность.

Для PR лучше убрать из первого patch. Если нужен upstream — отдельный PR с отдельной мотивацией.

### P1. Raw pointer contract недостаточно описан

`RawGpuAccess` отдаёт `*mut c_void`:

- `device`;
- `context`;
- `render_target`;
- width/height.

Сейчас не зафиксировано:

- указатели borrowed или owned;
- можно ли хранить;
- кто делает AddRef/Release;
- что происходит при resize/device lost;
- на каком thread вызывается callback;
- можно ли блокировать;
- можно ли вызывать GPUI APIs внутри callback;
- какие D3D state obligations;
- можно ли submit/present самому.

Для upstream это надо прописать прямо. Иначе это просто unsafe hole.

### P1. Vendor/russian comments

В публичном API и renderer comments сейчас есть:

- "MoonKernel fork";
- русский текст;
- "чарт";
- внутренние пояснения под нашу задачу.

Для стенда нормально. Для PR — стоп-фактор.

Нужно переписать нейтрально:

- custom render pass;
- external renderer integration;
- render phase;
- backend-specific pass context.

Никаких MoonBot/MoonKernel/terminal/chart в GPUI source.

### P1. Missing docs warnings

`gpui` включает `#![warn(missing_docs)]`.

Build logs уже показывали 6 warnings по public fields `RawGpuAccess`.

Для PR должно быть:

- 0 warnings;
- все public types/fields documented;
- docs не отписка, а контракт.

### P2. rustfmt сейчас не проходит

`rustfmt --check` показывал diff в `crates/gpui_windows/src/directx_renderer.rs`:

- длинная сигнатура `add_gpu_pass`;
- лишняя пустая строка.

Перед PR: `cargo fmt --all` / `rustfmt --check`.

### P2. В diff есть unrelated cleanup

В `directx_renderer.rs` было изменение:

```rust
if matches!(entry, ShaderModule::EmojiRasterization) { ... } else { ... }
```

на `match`.

Функционально ок, но к PR не относится. Убрать, чтобы diff был хирургический.

### P2. Upstream-visible example/test

Исторически в PR-source сам API нигде не использовался, а `chart_bench` лежал снаружи в
`stand-gpui/vendor`. Закрыто 2026-06-15: добавлен `crates/gpui/examples/gpu_pass.rs`, проверка
`cargo check -p gpui --example gpu_pass` проходит.

Для PR reviewer должен видеть:

- зачем API нужен;
- как им пользоваться;
- почему это не private hack.

Для PR reviewer теперь есть минимум:

- маленький example в Zed/GPUI repo;
- или хорошо оформленный PR description с ссылкой на public benchmark/repro;
- желательно test на registration/unregistration lifecycle, если архитектурно возможно.

## Что будет на macOS

GPUI hook сделан:

- `Window::add_gpu_pass` на macOS больше не no-op;
- `MacWindow` forward'ит регистрацию в `MetalRenderer`;
- `MetalRenderer` хранит UnderScene/OverScene callbacks и возвращает `Subscription`;
- `RawGpuAccess` отдаёт Metal-native handles: `MTLDevice*`, `MTLCommandBuffer*`,
  `MTLRenderCommandEncoder*`, `MTLTexture*`, `MTLPixelFormat`.

Особенность Metal:

- нельзя иметь два активных render encoders одновременно;
- реализовано через isolated custom encoder: clear frame → UnderScene encoder с `Load` → GPUI scene
  encoder с `Load` → OverScene encoder с `Load` → present.

Для нашего терминала на Mac нужен отдельный Metal renderer чарта:

- MSL shaders;
- Metal buffers;
- Metal pipeline states;
- combo texture;
- same semantic model, но другой backend.

## Что будет на Linux

GPUI hook сделан. Linux GPUI path использует `gpui_wgpu`:

- Wayland/X11 создают `WgpuRenderer`;
- renderer владеет `wgpu::Surface`, `Device`, `Queue`;
- получает current surface texture;
- делает command encoder/render pass;
- present.

Реализованный Linux hook — wgpu-native через `RawGpuAccess`: `wgpu::Device*`, `wgpu::Queue*`,
`wgpu::CommandEncoder*`, `wgpu::TextureView*`, `wgpu::TextureFormat*`, size, `device_generation`.
X11/Wayland forwarders возвращают настоящий `Subscription`.

Плюс lifecycle:

- Linux renderer уже умеет `device_lost()` и `recover(raw_window)`;
- `WgpuRenderer` сохраняет callbacks через `recover()` и increment'ит `device_generation`;
- custom renderer должен видеть generation/recover и пересоздавать свои wgpu resources.

Для нашего терминала Linux может быть самым удобным cross-platform backend, потому что chart renderer
на wgpu/WGSL естественно ложится в `gpui_wgpu`. Но Windows/Mac в GPUI сейчас не wgpu, поэтому одного
wgpu renderer для всех платформ через GPUI hook пока не получается.

## Каким должен быть upstream-grade API

Не навязываем точную сигнатуру, но свойства должны быть такими.

### 1. Registration возвращает handle

Пример формы:

```rust
pub fn add_gpu_pass(
    &self,
    phase: GpuPassPhase,
    callback: impl FnMut(&mut GpuPassContext) -> Result<()> + 'static,
) -> GpuPassHandle;
```

`GpuPassHandle`/`Subscription` при drop удаляет callback.

### 2. Explicit unsupported behavior

Не silent no-op.

Варианты:

```rust
pub fn try_add_gpu_pass(...) -> Result<GpuPassHandle, GpuPassUnsupported>;
```

или:

```rust
pub fn supports_gpu_passes(&self) -> bool;
```

Лучше `Result`, потому что caller сразу видит проблему.

### 3. Backend-specific context

Плохой вариант:

```rust
struct RawGpuAccess {
    backend: GpuBackend,
    device: *mut c_void,
    context: *mut c_void,
    render_target: *mut c_void,
}
```

Это слишком raw для upstream без safety-contract.

Лучше:

```rust
enum GpuPassContext<'a> {
    #[cfg(windows)]
    D3d11(D3d11PassContext<'a>),
    #[cfg(target_os = "macos")]
    Metal(MetalPassContext<'a>),
    #[cfg(target_os = "linux")]
    Wgpu(WgpuPassContext<'a>),
}
```

Если Zed не хочет тянуть platform types в public gpui crate, можно оставить opaque/raw, но docs должны
быть очень сильные.

### 4. Device generation

Обязательно:

```rust
device_generation: u64
```

или event callbacks:

```rust
GpuPassEvent::Render(...)
GpuPassEvent::DeviceLost
GpuPassEvent::DeviceRestored { generation }
```

Generation проще и универсальнее.

### 5. State isolation

Варианты:

- GPUI делает state guard вокруг callback;
- API contract требует restore state;
- для D3D11 в debug build можно assert/check наиболее опасные state changes.

Для 99% принятия лучше хотя бы:

- четкий documented contract;
- GPUI восстанавливает минимум, который сам ожидает;
- объяснить, почему это безопасно.

### 6. Фазы кадра

Текущие `UnderScene`/`OverScene` нормальны, но naming можно сделать более GPUI-neutral:

- `BeforeScene`;
- `AfterScene`;
- maybe `BeforePresent`.

Для нашего терминала нужен `BeforeScene/UnderScene`: chart под UI.

### 7. Никакого present внутри callback

Contract:

- callback не делает present/swapchain acquire;
- callback не block-ит frame loop;
- callback не вызывает GPUI APIs, которые мутируют scene/window;
- callback рисует только в предоставленный target/encoder или свои offscreen resources.

## Почему PR важен для Zed, не только для нас

PR нельзя продавать как "нам для MoonBot надо". Нужно показать общий value.

Use cases:

- high-density charts/time-series visualization;
- video preview / media surface;
- game/scene preview inside editor;
- GPU profiler/debug visualizers;
- terminal-like views with custom renderer;
- CAD/map/large canvas panes.

Главный тезис:

> GPUI отлично рисует UI/text/layout, но есть классы viewport-ов, где приложение уже имеет свой
> GPU renderer. Сейчас для них нет zero-copy integration path. Custom render pass даёт такой путь,
> сохраняя GPUI scene/text/composition.

Важно:

- не просить Zed принять chart renderer;
- не просить принять MoonBot-specific code;
- просить принять маленький generic mechanism.

## Как должен выглядеть PR description

Структура:

1. Problem

   GPUI apps sometimes need to embed high-throughput GPU-rendered viewports. Current options are:

   - re-emit thousands of GPUI primitives;
   - render offscreen and copy/readback;
   - create native child surfaces with compositor/input/z-order problems.

2. Solution

   Add a generic custom GPU pass registration point to Window/PlatformWindow. The pass is invoked
   during the renderer frame at a documented phase and receives backend-specific render context.

3. Safety/lifecycle

   - registration handle;
   - callback lifetime;
   - unsupported platform behavior;
   - device generation/device lost;
   - render state contract.

4. Platform support

   Be honest.

   Current PR can say:

   - Windows/D3D11 implemented;
   - macOS/Metal implemented;
   - Linux/wgpu implemented;
   - platforms without backend implementation return explicit Unsupported.

5. Performance evidence

   Include benchmark summary:

   - GPUI primitives path too slow for 100k markers;
   - readback path has extra copies and latency;
   - direct pass removes readback and keeps UI text overlay;
   - combo/static texture cache reduces GPU load for multi-chart cases.

6. Non-goals

   - no chart-specific code;
   - no new layout/widget abstraction;
   - no change to existing GPUI rendering unless no passes registered;
   - no forced frame pacing change.

## Что убрать из текущего PR до открытия

- ✅ `set_present_sync_interval` — удалён из PR-surface 2026-06-15.
- ✅ `MoonKernel`/русские comments — вычищены из изменённых GPUI sources.
- ✅ fake `GpuBackend::Metal/Other` — удалены; `GpuBackend::{D3D11, Metal, Wgpu}` теперь реально implemented.
- ✅ unrelated shader `if -> match` cleanup — в текущем diff не найден.
- ✅ any chart-specific language in GPUI source — в текущем diff не найдено.
- ✅ silent no-op semantics — заменено на real Mac/Linux implementation + explicit `Err` для unsupported platforms.

## Что добавить до PR

- ✅ unregister handle;
- ✅ `Result` from callback;
- ✅ explicit unsupported result;
- ✅ docs for safety/lifecycle/thread/state;
- ✅ device generation;
- ✅ D3D11 state restore for GPUI-owned OM/RS/IA/VS/PS state;
- ✅ field docs for all public fields;
- ✅ rustfmt clean on touched files;
- ✅ minimal example in `crates/gpui/examples/gpu_pass.rs`;
- ⏭️ PR description with proof and non-goals.

## Acceptance checklist: "99% шанс"

Перед PR должно быть так:

- [x] Branch rebased/cherry-picked on current `zed-industries/zed` main (`f39cf25c0ba571eaaa7a21f3266c8a356367fa5f` checked 2026-06-15).
- [x] Diff only touches required backend hook files.
- [x] No vendor words: MoonBot/MoonKernel/chart/terminal in touched GPUI sources.
- [x] No Russian comments in touched GPUI/Zed source.
- [x] `rustfmt --check` clean on touched files.
- [x] `cargo check -p gpui -p gpui_windows -p gpui_wgpu` clean.
- [x] `cargo check -p gpui_linux` clean.
- [x] `cargo check -p gpui_macos` clean.
- [x] No new warnings from `missing_docs` observed in touched crates check.
- [x] Callback registration is removable.
- [x] Callback can report error.
- [x] Mac/Linux are implemented; unsupported platforms are explicit, not silent.
- [x] Device lost/recovery story exists via `device_generation`.
- [x] Resize story exists: frame-scoped `render_target`/size/format are refreshed each callback.
- [x] State/isolation contract exists: D3D via state guard, Metal via isolated encoders, wgpu via pass-boundary isolation.
- [x] No `set_present_sync_interval` in this PR.
- [x] Existing GPUI behavior unchanged when no custom pass is registered.
- [x] Example included: `cargo check -p gpui --example gpu_pass` passes.
- [ ] PR has clear non-goals.

## Recommended implementation order for the coding agent

1. ✅ Remove unrelated changes.
2. ✅ Rename API/comments to neutral upstream names.
3. ✅ Replace permanent Vec registration with removable `Subscription`.
4. ✅ Change callback to return `Result`.
5. ✅ Add explicit unsupported behavior.
6. ✅ Add device generation.
7. ✅ Add docs/safety contract.
8. ✅ Add D3D11 state guard.
9. ✅ Add Metal backend hook.
10. ✅ Add wgpu/Linux backend hook.
11. ✅ Rebase/cherry-pick fork on fresh Zed main.
12. ✅ Add a tiny example/test or benchmark proof.
13. ⏭️ Prepare PR description with benchmark evidence.

## The core argument

Форк нужен не потому что "у нас особый график".

Форк нужен потому что GPUI сейчас не имеет официальной точки расширения для zero-copy native GPU
viewport внутри своего frame. Для высоконагруженного терминала это не optimization nicety, а
архитектурный requirement.

Правильный PR должен выглядеть не как "мы засунули raw pointer ради своего чарта", а как:

> Add a small, well-scoped custom GPU pass integration point for applications that need to compose
> backend-native GPU content with GPUI-rendered UI.

Если это будет маленько, чисто, честно по platform support, с lifecycle и safety contract — шанс
принятия высокий. По состоянию 2026-06-15 blocker-косяки по backend hook закрыты; настоящий оставшийся
риск — example/proof, финальный review diff и вкус мейнтейнеров Zed.
