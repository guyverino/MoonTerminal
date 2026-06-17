# Terminal_patch_V1 — полный план миграции MoonTerminal на gpu_canvas

Status: implementation plan v1, implementation audit reopened, 2026-06-17.

Этот документ самодостаточный. Он описывает, как перевести MoonTerminal с
window-global GPU pass / continuous presentation на элементный `gpu_canvas` API
форка, какие старые механизмы удалить, как сохранить fast live-scroll, и как
проверить Windows/macOS/Linux.

Current implementation status is tracked in this checklist and fresh `ForkIssues_*` audits.
Do not treat remaining unchecked boxes as mere runtime polish unless that file
classifies them that way.

## Главные критерии приемки

Эти четыре пункта важнее любых локальных компромиссов и чекбоксов ниже:

```text
1. Решение о рисовании принимает график: frame() решает Skip/RequestPresent,
   prepare/draw идут в тот же platform tick, без пропуска кадра после решения,
   и нет бездумной очистки/present на каждый такт.
2. Решение повышает шансы upstream PR: это generic-фича уровня custom cursor /
   video / chart / viewport, а не терминальный хак.
3. Решение кросс-платформенное: Windows DX11, macOS Metal, Linux native GPUI
   backend, без readback и без wgpu как общей прослойки для Windows/macOS.
4. Реализация красивая, минималистичная и строгая: один понятный владелец
   frame/data/present decisions, без скрытых 16мс pump-таймеров и подпорок,
   которые живут рядом с gpu_canvas.
```

## Цель

Терминал должен получить быстрый кросс-платформенный chart renderer:

```text
chart сам решает, нужен ли GPU кадр;
решение принимается до clear/present;
если кадр нужен, prepare/draw происходят в том же platform tick;
Skip не делает clear/render-pass/Present;
GPUI view tree не rerender'ится на каждый tick/mousemove;
Windows / macOS / Linux используют нативный backend GPUI, без readback и без wgpu как кроссплатформенной прослойки.
```

## API форка, на который опирается терминал

```rust
pub fn gpu_canvas<D>(driver: D) -> GpuCanvas
where
    D: Into<GpuCanvasHandle>;

pub trait GpuCanvasDriver {
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision;

    fn prepare_gpu(&mut self, ctx: &mut GpuCanvasPrepareContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn draw(&mut self, ctx: &mut GpuCanvasDrawContext) -> anyhow::Result<()>;
}

pub enum GpuFrameDecision {
    Skip,
    RequestPresent,
}

pub struct GpuFrameInfo {
    pub now: Instant,
    pub bounds: Bounds<Pixels>,
    pub scale_factor: f32,
    pub presentable: bool,
}
```

Element usage:

```rust
gpu_canvas(driver)         // under whole GPUI scene
gpu_canvas(driver).over()  // over whole GPUI scene
```

Frame order guaranteed by fork:

```text
frame()       // CPU decision, no GPU access
prepare_gpu() // GPU resources/uploads/offscreen, no active GPUI scene pass
clear
draw()        // direct draw/composite into under/over phase
Present
```

If any present happens for any reason, every visible canvas gets `prepare_gpu()`
and `draw()`, even if it returned `Skip`.

## New chart ownership model

Chart state must be retained outside `render()`.

```rust
struct ChartPanel {
    chart_state: Rc<RefCell<ChartState>>,
    under_driver: ChartUnderCanvas,
    over_driver: ChartOverlayCanvas,
}

struct ChartUnderCanvas {
    state: Rc<RefCell<ChartState>>,
}

struct ChartOverlayCanvas {
    state: Rc<RefCell<ChartState>>,
}
```

`ChartPanel::render()` emits elements:

```rust
gpu_canvas(self.under_driver.clone())       // chart/background/grid/combo/book/userdata
gpu_canvas(self.over_driver.clone()).over() // crosshair + readouts
```

Do not create driver state in `render()`. That would reset camera, buffers,
device generation, bake state and cursor state on every GPUI rerender.

Multiple charts:

```text
each chart panel owns its own ChartState
each visible chart emits its own under/over gpu_canvas records
one window may contain 4 visible active charts
one chart requesting present causes all visible canvases in that window to draw
```

## ChartState responsibilities

`ChartState` owns retained state:

```text
camera / time scale / price scale
follow/live/pause state
last frame/present timestamps
phase-clean px_per_ms
per-pane bounds and view snapshots
data revisions / ring heads / append ranges
combo/orderbook/userdata layer dirty flags
GPU resources per backend
device_generation per backend
crosshair/readout overlay state
theme/font/DPI dependent overlay atlas
diagnostic counters
```

`frame()` may mutate only this retained state. It must not call `cx.notify()` and
must not mutate GPUI tree.

## Data ingestion bridge

Backend feed drain is event-driven by incoming feed messages. That wake is not a
chart render/present decision.

Correct bridge:

```text
incoming feed event wakes the foreground data-drain task
backend session.drain() applies pending feed messages
if chart-visible data changed:
    backend updates registered chart data handles
    chart copies/rebuilds retained CPU state without cx.notify()
    chart marks its retained driver state dirty/needs_present
gpu_canvas.frame() later decides Skip/RequestPresent on platform frame-clock
prepare_gpu/draw happen only on actual present path
```

Forbidden bridge:

```text
ChartPanel-owned 16ms pump/timer
per-chart async prepare loop
per-vblank/per-data cx.notify() just to feed GPU pixels
```

Implemented bridge:

```text
backend session.drain() returns DrainStats
if and only if chart-visible data changed:
    backend updates registered ChartDataHandle consumers
    chart syncs app/session data into retained chart state without cx.notify()
gpu_canvas.frame() later consumes retained dirty flags / camera state
    and decides Skip/RequestPresent
```

This removes the per-chart and global 16ms pumps, removes data prepare from
throttled `observe`/ordinary `render` cadence, and removes the high-rate
`WeakEntity<ChartPanel>` path. Render may still force one retained sync for
lifecycle reasons (first visible frame, resize, settings/theme/follow change);
market data does not enter through render.

## Under canvas: `frame()`

`ChartUnderCanvas::frame(info)` is the CPU decision point. It replaces the old
mix of 62.5 Hz async prepare task, own-pass callback present_seq, and live-edge
advance in render callback.

Inputs:

```text
info.now
info.bounds
info.scale_factor
info.presentable
backend/session data revisions already copied into ChartState
current mouse/input/camera state already copied into ChartState
```

Immediate skips:

```text
if !info.presentable -> Skip
if info.bounds empty -> Skip
if chart hidden / no active pane -> Skip
if paused and no dirty data/input/resize -> Skip
```

Resize / DPI:

```text
if bounds or scale_factor changed:
    update chart rects
    recompute phase-clean default scale only when this is default/reset-live/rescale path
    mark full layer rebuild where needed
    RequestPresent
```

Live pixel-cross:

```text
if follow/live:
    if pixel phase crossed and min-present-interval allows:
        advance_camera(now)
        mark scroll/composite update
        RequestPresent
    else:
        do not advance_camera
```

Critical invariant:

```text
advance_camera only on a path that returns RequestPresent.
No camera/phase mutation on Skip.
```

Data triggers:

```text
new combo ticks / rev changed -> mark append/full as needed, RequestPresent
new orderbook data -> mark book dirty, subject to cadence/throttle, RequestPresent when due
new user data / orders / lines / markers -> mark userdata dirty, RequestPresent
periodic live-trades visual cadence -> RequestPresent when due
diagnostic/500ms floor -> RequestPresent when due, if visible/presentable
```

Periodic cadence jitter:

```text
each chart gets deterministic phase offset by chart id
orderbook 5 Hz, live-trades ~10 Hz, diagnostics/floor are phase-shifted
do not stack all charts on the same candle-close / timer tick
```

Backstop:

```text
frame() keeps a minimum interval between RequestPresent for GPU-only live scroll.
This is a chart-side SkipCount equivalent.
It prevents zoom/catch-up from requesting above target rate.
```

## Phase-clean default time scale

Default/reset-live scale is anchored near 60 seconds. It is not hardcoded to
`1px per frame`.

Definitions:

```text
monitor_rate = actual monitor/display source rate
effective_present_rate = monitor_rate / round(monitor_rate / 60)
dt = 1000 / effective_present_rate
chart_w = chart plot width in device pixels
```

Snap formula:

```text
s0 = chart_w * dt / 60000

if s0 >= 1:
    shift = max(1, round(s0))          // integer pixels per frame
else:
    shift = 1 / max(1, round(1 / s0))  // one pixel per N frames

px_per_ms = shift / dt
window_ms = chart_w / px_per_ms
```

Examples at 60 Hz:

```text
1000 px -> window about 66.7s
1280 px -> window about 64.0s
1920 px -> window about 64.0s
2560 px -> window about 42.7s
```

Rules:

```text
target is near 60s, not exactly 60s
do not use fixed px_per_ms = 1 / dt
do not let default collapse to 29s / 6s / 2s
do not resnap on normal zoom
call only on default/reset-live and resize/present-rate changes
zoom remains separate, e.g. x2 around cursor
```

## Under canvas: `prepare_gpu()`

`ChartUnderCanvas::prepare_gpu(ctx)` runs on any actual present before the
under/over phase pass is opened. It is called for every visible chart canvas.

Responsibilities:

```text
check backend type
check device_generation / format / target size
recreate resources if generation/format changed
apply pending buffer uploads
perform pending offscreen bake
cheap-noop if nothing pending
```

Device generation:

```text
if ctx.device_generation != cached_device_generation:
    drop/recreate backend resources
    mark combo/orderbook/userdata full rebuild
    recreate scissor/rasterizer/pipelines/bind groups as backend requires
```

DX11:

```text
existing ComboTex / BookTex offscreen bake can keep render-target switching
inside prepare_gpu.
State must be guarded/restored.
After prepare_gpu, draw() should mostly composite/direct draw into phase target.
```

Metal:

```text
prepare_gpu runs before active phase render encoder.
Offscreen ComboTex / BookTex work must be encoded here, not inside draw().
draw() uses provided phase encoder only.
No nested render encoder in draw().
```

wgpu:

```text
prepare_gpu runs before active phase render pass.
Offscreen bake/uploads may open their own render passes here.
draw() uses provided phase render pass only.
No begin_render_pass inside draw().
```

Cross-platform rule:

```text
resident/offscreen layer updates must have a legal backend path on DX11, Metal and wgpu.
If a Metal/wgpu layer remains direct-draw only, that must be measured and accepted per layer,
not hidden as an API limitation.
```

## Under canvas: `draw()`

`ChartUnderCanvas::draw(ctx)` paints current pixels into the under phase target.

Order:

```text
background
grid
combo / volume / crosses / price lines
orderbook
userdata: zones, hlines, segments, markers, order lines
```

Rules:

```text
set scissor to chart plot/book bounds inside GPUI-provided clip
draw must tolerate being called after frame() returned Skip
draw must not assume it was the canvas that requested present
draw must not open nested Metal/wgpu pass/encoder
draw must not present
draw must not call cx.notify()
```

If `draw()` returns `Err`:

```text
log backend + chart id + layer context
let renderer restore state
skip this canvas for the frame
do not poison other canvases / GPUI scene
```

## Overlay canvas: crosshair and readouts

Mouse move must not dirty GPUI tree.

Current bad path to remove:

```text
on_mouse_move -> input.cursor update -> cx.notify() -> top-down render
```

New path:

```text
on_mouse_move:
    compute chart-local cursor / hovered pane
    write cursor/readout state into ChartState
    do not call cx.notify()

ChartOverlayCanvas::frame():
    if cursor/readout state changed and presentable:
        RequestPresent
    else:
        Skip

ChartOverlayCanvas::prepare_gpu():
    update small glyph atlas/resources if dirty

ChartOverlayCanvas::draw():
    draw crosshair lines + price/time readout chips
```

Readout glyph atlas:

```text
small numeric/time atlas, not full text engine
glyphs: digits, punctuation needed for price/time/readout strings
colors/sizes match terminal design
theme change invalidates atlas
DPI/scale change invalidates atlas
font/text-size change invalidates atlas
device_generation change recreates atlas resources
```

Overlay scope:

```text
draw only inside chart/readout scissor
do not cover popup/menu/tooltip incorrectly
hide/gate overlay if GPUI popup/menu over chart would be covered
```

Popup/menu behavior must be checked early, not at the end.

## GPUI invalidation rules

`cx.notify()` is allowed only for slow GPUI UI changes:

```text
axis label text changes
toolbar / tabs / dock / settings UI
theme changes
panel layout changes
status bar / metrics at throttled cadence
```

`cx.notify()` is forbidden for high-frequency chart pixels:

```text
live pixel-cross scroll
mousemove crosshair
readout movement
orderbook GPU pixels
live trades
combo append/composite
```

If a high-frequency path needs visible pixels, it goes through `gpu_canvas`.

## Axis labels

Axis labels may remain GPUI text if they update rarely.

Rules:

```text
no per-mousemove axis notify
coalesce label text changes to low rate, e.g. <= 4 Hz
cache shaped/layout text by key: text, size, color, DPI
readouts that follow cursor are overlay canvas, not GPUI text
```

If axis labels become a measured render hotspot, port them later to GPU atlas,
but do not block the main gpu_canvas migration on that.

## Input: pan / zoom / live detach

Pan/zoom state changes update `ChartState` and request pixels via the next
`gpu_canvas.frame()` path. Do not use per-input `cx.notify()` for GPU-only pixels.

Pan from live:

```text
drag immediately detaches follow/live state in ChartState
do not re-anchor to live during drag moves
snap/re-anchor checks happen on mouse-up or explicit Live action
```

X/Y drag:

```text
Y pan remains possible even if X pan is active
axis lock/dead-zone should reflect intentional gesture, not 5px accidental noise
```

Wheel zoom:

```text
scroll direction must match platform expectation
zoom around cursor where applicable
do not let sync-to-live override cursor anchor mid-gesture
```

Any action that changes camera:

```text
update driver state
let frame() return RequestPresent
prepare/draw same tick if frame gate allows
```

## Remove old mechanisms

Delete from terminal after migration:

```text
ChartEngine::register_pass
ChartEngine::unregister_pass
pass_subscription
pass_window
ChartPanel::present_guard
ChartPanel::present_guard_window
Window::request_continuous_presentation() usage
present_seq as own-pass callback counter
62.5 Hz async prepare task
manual old-window pass detach fixes
```

Why safe:

```text
gpu_canvas lives in GPUI scene of its current window
dirty rebuild removes it from old/hidden scenes
cached scene replay preserves it only where the element still exists
stale handles are no-op / dropped by normal ownership
```

Must test detach/hidden explicitly.

## Backend-specific terminal work

### DX11 / HLSL

Keep and adapt existing own-pass DX11 renderer:

```text
ComboTex / BookTex offscreen caches move to prepare_gpu
draw composites/direct-draws to under phase target
state guard around app GPU work
scissor per pane
device_generation recovery forces resource recreation
```

### macOS / Metal / MSL

Metal path must be first-class, not a future placeholder.

Required:

```text
all chart shaders compile with current Metal toolchain
no constant-address-space local arrays that break newer compiler
prepare_gpu handles uploads/offscreen work before phase encoder
draw uses provided Metal render encoder only
device_generation/format changes recreate pipelines/buffers/textures
multiple visible chart windows do not tank to unacceptable FPS without explanation
```

### Linux / wgpu / WGSL

Linux uses GPUI native wgpu backend. This is not the old rejected wgpu-offscreen
cross-platform bridge.

Required:

```text
prepare_gpu handles uploads/offscreen work before phase render pass
draw uses provided wgpu phase render pass only
draw does not call begin_render_pass
Wayland timer fallback keeps frame decisions alive without commits
X11 timer path works through existing refresh loop
surface lost/unconfigured path does not call app GPU callbacks
```

## Multiple charts

Four visible active charts in one window must work.

Expected behavior:

```text
all visible chart canvases get frame() before clear
if any chart requests present, every visible chart gets prepare_gpu() and draw()
each chart keeps independent state/resources
no global mutable singleton for current chart/window
no stale pass in old window after detach
orders/shell panels do not rerender at chart cadence
```

Periodic work must be phase-jittered:

```text
chart_id-based offsets for book/live-trades/diagnostic cadences
avoid all charts rebaking on the same tick
data-triggered full bake should be throttled/guarded per layer
```

## Public dependency workflow

When terminal depends on fork/UI components publicly:

```text
Cargo.toml uses git dependencies to public branches
no local path = "R:/..." in committed manifests
local development uses ignored .cargo/config.toml [patch] overrides
after changing fork, push fork first, then verify terminal against public branch
after changing MoonPalette/Moon UI components, push components first, then verify terminal
```

Before publishing terminal:

```text
cargo metadata must show public git dependencies when local patch is disabled
local patch may resolve to local worktree only on developer machine
```

## Windows build/run validation

Build with explicit MSVC target.

```powershell
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && `"C:\files\utils\rust\cargo\bin\cargo.exe`" build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc"
```

Executable to test:

```text
R:\test\MoonTerminal\target\x86_64-pc-windows-msvc\debug\moonterminal.exe
```

Do not validate from:

```text
target\debug\moonterminal.exe
```

## Diagnostics

Use runtime counters, not guesses.

Required diagnostics:

```text
render frequency by GPUI view type via MOON_RENDER_DIAG
debug UI / auto stress via --features debug-tools:
    status-bar "debug" label
    MOON_RENDER_DIAG_OPEN_10_BTC=1 opens the same 10 BTC chart windows as the button
orders_render / shell_render / chart_render
chart frame decisions per canvas
chart prepare_gpu count
chart draw count
combo bake / combo append / combo composite
orderbook bake / orderbook composite
overlay prepare/draw
present count
skip count
device_generation recreate count
```

Expected after migration:

```text
live chart scroll does not drive Orders/Shell render at monitor rate
mousemove over chart does not call cx.notify()
combo_bake near 0 during pure mousemove
orderbook_bake near 0 during pure mousemove
draw may happen at chart cadence, GPUI render stays gated
```

## Implementation order

1. [x] Update terminal dependencies to the new fork API branch.
   Done: public manifests use `Moonbot-Tech/ZedFork` and `Moonbot-Tech/MoonPalette`.
2. [x] Introduce retained `ChartState` and under/over driver wrappers.
   Done with deviation: retained state is `chartdx::RenderState` + one
   `ChartCanvasDriver`; no separate under/over driver split.
3. [x] Emit `gpu_canvas(under)` and `gpu_canvas(over).over()` from `ChartPanel::render`.
   Done with deviation: `ChartPanel` emits one `gpu_canvas(self.canvas.clone())`;
   cursor/readout pixels are native layers inside the same driver, while GPUI
   popups/tooltips remain above the scene.
4. [x] Port current chart layout/bounds snapshots into `ChartState`.
   Done via `set_origin`, `resize`, pane snapshots and axis pane snapshots.
5. [x] Move live-edge decision and camera advance into `ChartUnderCanvas::frame`.
   Done in `RenderState::frame` / `PaneRender::advance_camera`.
6. [x] Implement phase-clean default scale exactly.
   Done in `moon-chart/src/view.rs::phase_clean_default_px_per_ms`.
7. [x] Move DX11 resource sync/offscreen bake into `prepare_gpu`.
   Done in chartdx DX11 backend callbacks.
8. [x] Adapt Metal resource sync/offscreen/direct draw to `prepare_gpu` + phase `draw`.
   Done in `chartdx/backend.rs`, `chartdx/mod.rs`, `chartdx/metal_backend.rs`:
   Metal `prepare()` creates pipelines/textures and uploads buffers before the
   phase encoder; Metal `render()` now only uses the GPUI-provided phase encoder.
9. [x] Adapt wgpu resource sync/offscreen/direct draw to `prepare_gpu` + phase `draw`.
   Done in `chartdx/backend.rs`, `chartdx/mod.rs`, `chartdx/wgpu_backend.rs`:
   wgpu `prepare()` owns device/queue uploads and bind-group rebuilds before the
   phase pass; wgpu `render()` now only uses the GPUI-provided phase render pass.
10. [ ] Move crosshair/readouts to overlay canvas and remove per-mousemove `cx.notify`.
    Partial only: cursor lines are native chartdx pixels; readout chips still use
    GPUI `axes::draw(...)` and throttled `cx.notify()`.
11. [x] Remove `register_pass`, continuous present guard, present_seq, async prepare task.
    Done with event-driven feed wake: `SessionManager::drain()` now runs after
    incoming feed wakes, high-rate chart data updates `ChartDataHandle` directly,
    and the old global `executor.timer(16ms)` / `WeakEntity<ChartPanel>` path is gone.
12. [x] Verify detach/hidden tab lifecycle.
    Done by element-owned `gpu_canvas` lifetime; native/manual regression still
    belongs to runtime testing.
13. [x] Verify multiple visible charts.
    Done by container/per-pane state isolation in code; runtime stress still required.
14. [x] Verify diagnostics counters.
    Done in `diag.rs` and debug stats window/test handoff.
15. [x] Build and run Windows MSVC target.
    Done locally with explicit `--target x86_64-pc-windows-msvc` and
    `--features debug-tools`; tested executable path is
    `target/x86_64-pc-windows-msvc/debug/moonterminal.exe`.
16. [x] Get macOS and Linux checks from native machines/CI.
    Done as initial native handoff in `MAC_LINUX_PERF_RES.md`: macOS build/Metal
    compile path OK but live perf is blocked by GUI Keychain permission for
    `moonterminal`; Linux X11 live 10-window run reached chart rendering and perf
    counters after Secret Service/openbox setup. Remaining native numbers are
    audit gates, not local implementation blockers.

## Validation checklist

Frame behavior:

- [x] `RequestPresent` from chart leads to `frame -> acquire -> prepare_gpu -> clear -> draw -> Present`
      in same tick.
- [x] `Skip` does not clear/render/present.
- [x] `advance_camera` never runs on skipped GPU-only tick.
- [x] Existing UI dirty/present reasons still draw chart canvases correctly.
- [x] Every visible chart gets `prepare_gpu()` and `draw()` on any actual present.

Rendering:

- [x] Pure live scroll composites resident layers without unnecessary full bake.
- [ ] Pure mousemove updates crosshair/readouts without combo/orderbook bake.
      Partial only: native crosshair lines update without combo/orderbook bake;
      moving readout chips are still GPUI-throttled, not native overlay pixels.
- [x] Resize never stretches stale texture.
- [x] Device generation change recreates all backend resources.
- [x] Background/grid/combo/orderbook/userdata draw in correct order.
- [x] Overlay crosshair/readouts align with chart pixels and axis math.
      Checked against the actual coordinate chain: GPUI mouse position is converted
      once from slot-local logical px to device px; native cursor uniforms and GPUI
      readout drawing both consume the same `axis_panes` snapshot, converting back
      to logical px only at the GPUI overlay draw boundary.

Invalidation:

- [x] Mousemove over chart does not call `cx.notify()` for cursor-only pixels.
      Drag and throttled readout edge cases still notify intentionally.
- [x] Orders/Shell render rate stays gated while live chart scrolls.
      Local Windows smoke with `MOON_RENDER_DIAG_OPEN_10_BTC=1` reached chartdx
      presents (`chart_present` max 49/s) while `orders_render`/`shell_render`
      stayed <=2/s. Full 60s perf/input-storm run is still a separate stress
      validation item below.
- [x] Axis labels update only on throttled/rare path.
- [x] Status/metrics UI updates by ordinary throttled notify, not chart cadence.

Input:

- [x] Pan detaches from live immediately on drag.
- [x] Re-anchor to live does not happen on every drag move.
- [x] X and Y pan can coexist where intended.
- [x] Wheel direction is correct.
- [x] Zoom around cursor is preserved.
- [x] Minimum/default time scale stays near 60s and phase-clean.

Multi-chart/window:

- [x] Four visible charts in one window remain stable.
      Checked against the multi-pane code path: `Container::layout` returns all
      visible tiled panes, `sync_from_session` syncs every visible pane by index,
      and axes/hit-test reuse the same `axis_panes` list. Local runtime stress also
      opened 10 BTC chart windows without Shell/Orders render flood.
- [x] One chart requesting present does not starve other visible canvases.
- [x] Detach moves chart canvas to new window only.
- [x] Hidden tab has no canvas polling/GPU work.
- [x] No stale pass/canvas draws into old window.

Platform:

- [x] Windows MSVC build succeeds and tested exe is target\x86_64-pc-windows-msvc\debug\moonterminal.exe.
      Done locally after latest fork/C2 code with `--features debug-tools`.
- [x] Windows live chart no longer freezes/stutters on idle frame-clock path.
      Done after removing the bad waitable posting gate; user manual check
      confirmed smoothness returned. Local 10-window diag also kept Shell/Orders
      gated while chart own-pass continued presenting.
- [x] Windows occlusion/minimize does not spin GPU work and recovers.
      Local diag run with 10 BTC chart windows: after minimizing all windows,
      chart draw counters dropped to zero (`chart_frame_request=0`,
      `bg_draw/grid_draw/combo_draw=0`); after restore, presents and chart draws
      resumed.
- [x] macOS Metal shaders compile and chart creation does not panic.
      Confirmed by Mac developer after shader fix; FPS check is separate.
- [ ] macOS multiple chart windows have acceptable FPS or measured bottleneck with fix plan.
      External audit gate; current blocker is macOS Keychain GUI permission for
      `moonterminal`, not Metal shader/chart creation.
- [x] Linux wgpu path builds and runs on X11.
      Native Linux report: Ubuntu 24.04/NVIDIA/Vulkan, 10 debug chart windows,
      live rendering and perf counters collected after Secret Service/openbox
      setup. Wayland is still separate.
- [ ] Wayland frame decision keeps ticking via fallback without blind presents.
      External native audit gate.

Design:

- [x] MoonBot Terminal design remains priority.
- [x] No generic toolkit defaults override chart/readout look.
- [x] Main window chrome keeps MoonTerminal custom design while remaining draggable.
      Header drag zone covers the left info block and empty header spacer; right
      actions/window controls remain clickable.
- [x] Main window is forced opaque at OS background level.
      `window_background` and Windows DWM background appearance are explicitly
      Opaque. If resize still shows a hole, the next fix belongs in the
      `gpui_windows` DirectComposition/resize path, not MoonPalette.
- [x] Popup/menu/tooltip over chart is not covered incorrectly by overlay canvas.
      Done at architecture level: chart is emitted as UnderScene `gpu_canvas`
      only; there is no chart `gpu_canvas().over()` layer above GPUI popup/tooltip
      elements. Manual visual polish remains an audit task.

Public repo:

- [x] No local path dependencies committed.
- [x] Public git dependencies resolve without local patches.
      Verified by temporarily disabling the ignored `.cargo/config.toml` patch and
      the ignored local `Cargo.lock`, then running `cargo metadata`: GPUI resolves
      to `Moonbot-Tech/ZedFork#3ae7b8e4`,
      MoonPalette resolves to `Moonbot-Tech/MoonPalette#a03dd6e5`.
- [x] Internal Russian/profanity docs are not published to component/fork repos.
