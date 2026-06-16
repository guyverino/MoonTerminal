# ZED_FORK_V1 — полный план нового GPUI/Zed fork

Status: canonical implementation plan v1, 2026-06-16.

Этот документ самодостаточный. Он описывает, что писать в Zed/GPUI fork, как это
оформлять для upstream PR, какие старые идеи не переносить, и какие проверки
обязательны. Терминальной специфики в коде GPUI быть не должно.

## Цель

Добавить в GPUI обычный UI-элемент, который:

```text
layout / bounds / clip / lifetime / window получает от GPUI tree;
рисует backend-native GPU-контент;
решает до clear/present, нужен ли кадр;
если кадр нужен, готовит GPU-ресурсы и рисует в тот же platform tick;
не dirty'т GPUI view tree для GPU-only кадров;
не делает clear/present на skipped tick.
```

Короткое upstream-позиционирование:

```text
Add a phase-composited native GPU element with pre-present frame decision.
```

Use cases для PR:

```text
dense charts / time-series
custom cursor / crosshair overlays
video or GPU preview panes
game/editor viewport
GPU profiler visualizers
map / CAD panes
```

## Базовые инварианты

```text
frame()       = CPU decision before clear/present, no GPU access
prepare_gpu() = GPU resource/upload/offscreen work, no active GPUI scene pass
draw()        = direct draw/composite into provided under/over phase pass
Present       = only when UI dirty, existing present reason, or any gpu_canvas asks
Skip          = no clear, no renderer draw, no Present
```

Канонический successful frame:

```text
platform tick
  -> optional UI scene rebuild
  -> frame() barrier for visible gpu_canvas
  -> acquire GPU frame/drawable/surface
  -> prepare_gpu() for every visible gpu_canvas
  -> clear target
  -> draw under gpu_canvas list
  -> draw normal GPUI scene batches
  -> draw over gpu_canvas list
  -> Present
  -> complete_frame
```

`RequestPresent` не переносится на следующий tick. Решение и draw идут в одном
platform callback.

## Upstream PR strategy

Можно отдельно отправить маленький Windows DPI restore bugfix:

```text
PR0: Windows DPI restore round-trip bugfix.
```

`gpu_canvas` отправлять отдельным generic feature PR:

```text
PR1: gpu_canvas.
```

Windows upstream уже имеет базовый heartbeat:

```text
VSyncProvider -> RedrawWindow -> WM_PAINT -> on_request_frame
```

Этого достаточно для correctness generic `gpu_canvas`: элемент получает frame
opportunity, может вернуть `RequestPresent`, и будет нарисован в тот же tick.
Этот путь не идеален под тяжёлой Windows-нагрузкой, но он функционально существует
в чистом upstream.

Windows frame-clock/pacing patch держать отдельным PR:

```text
PR2: Windows pacing / waitable swapchain / posted frame-clock tick.
```

Это generic Windows perf/latency fix. Он чинит starvation `WM_PAINT` под input
storm / multi-window load и UI-thread blocking around `Present`. Он важен для
нашего высокочастотного терминала, но не является семантической частью `gpu_canvas`.
Если вбандлить его в `gpu_canvas`, PR станет больше, Windows-специфичнее и хуже
принимаемым.

`gpu_canvas` PR держать компактным:

```text
commit 1: gpu_canvas public API + scene/replay/gate
commit 2: renderer hooks for DX11 / Metal / wgpu
commit 3: Wayland timer fallback if needed for skip-present clock
commit 4: example + tests
```

Floating/tool-window style для detached/floating окон — отдельный optional /
delivery-only patch. Не смешивать с `gpu_canvas`.

Wayland fallback остаётся в `gpu_canvas` PR, потому что Wayland frame callback can
depend on commits; skipped presents can otherwise stop the decision clock. Windows
C2 остаётся отдельным PR, потому что на Windows базовый heartbeat уже есть.

## PR0: Windows DPI restore round-trip bugfix

Это отдельный маленький PR, не часть `gpu_canvas`.

Баг:

```text
detached/floating окно восстанавливается на правильном monitor/display_id,
но saved logical origin умножается на scale не того HWND.

Пример при scale=1.25:
saved x=95 -> restored x=119
saved y=-733 -> restored y=-916
```

Причина:

```text
retrieve_window_placement переводит saved logical bounds в device pixels
через scale свежесозданного HWND, а тот ещё держит DPI primary monitor,
не целевого display_id.
```

План фикса:

```text
gpui_windows/src/display.rs:
    добавить WindowsDisplay::scale_factor() getter.

gpui_windows/src/window.rs:
    перед SetWindowPlacement взять scale у выбранного display;
    прокинуть этот scale в state.scale_factor;
    прокинуть scale в direct_manipulation;
    передать scale параметром в retrieve_window_placement;
    не брать initial restore scale из ещё не пере-DPI'нутого HWND.
```

Валидация PR0:

```text
floating/detached окно на мониторе scale != primary scale
close/restart 2-3 раза
origin не дрейфует и не умножается на scale
```

## Main PR: gpu_canvas

### Public API shape

V1 API намеренно phase-composited: canvas рисуется либо под всей GPUI scene,
либо над всей GPUI scene. Arbitrary interleaving между обычными quads/text/sprites
не поддерживается в V1.

```rust
pub fn gpu_canvas<D>(driver: D) -> GpuCanvas
where
    D: Into<GpuCanvasHandle>;

pub enum GpuCanvasLayer {
    UnderScene,
    OverScene,
}

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

Element API:

```rust
gpu_canvas(driver)         // UnderScene default
gpu_canvas(driver).over()  // OverScene
```

`GpuCanvas` должен быть `Styled`, брать layout из style, наследовать
content mask / clip от GPUI tree.

### Driver identity and lifetime

`gpu_canvas(driver)` — декларативный element. Element не должен владеть временным
non-clone объектом, который создаётся заново в `render()`.

Правильная модель:

```text
application owns retained driver state;
element stores cloneable stable handle to that driver state;
scene stores cloneable PaintGpuCanvas payload;
drop/unmount/hidden tab убирает canvas через обычный scene rebuild/replay.
```

Driver handle может быть `Rc<RefCell<_>>`, `Arc<Mutex<_>>`, entity/id handle или
локальный GPUI-handle. Главное: payload в scene/replay cloneable.

### Callback contracts

`frame(info)`:

```text
CPU only.
Runs before clear/present.
May mutate only driver-owned retained state.
Must not call cx.notify().
Must not mutate GPUI entity tree.
Must not trigger layout.
Must not schedule per-vblank/per-mousemove UI invalidation.
Returns Skip or RequestPresent.
```

Если driver понял, что нужен обычный GPUI UI update, он выставляет свой редкий
UI dirty flag, а приложение отдаёт его в throttled notify path. Это не часть
per-frame GPU path.

`prepare_gpu(ctx)`:

```text
GPU access is available.
No active GPUI scene render pass / render encoder is open.
Runs only on actual present path after frame acquisition.
Runs for every visible gpu_canvas, not only for one that requested present.
Must cheap-noop when no resource/upload/offscreen work is pending.
May recreate resources on device_generation / format / drawable changes.
May upload buffers/textures.
May perform offscreen bake using legal backend-specific pass/encoder usage.
Must not present.
Must not store borrowed native handles.
```

`draw(ctx)`:

```text
Runs after target clear.
Runs in under or over phase.
Uses current phase target/pass/encoder.
Must draw current pixels every time it is called.
Must not open nested Metal render encoder or nested wgpu render pass.
Must not present.
Must not store borrowed native handles.
Must restore or leave restorable state according to renderer contract.
```

Important invariant:

```text
Present reason and draw obligation are separate.

frame() -> RequestPresent means this canvas wants a present.
frame() -> Skip does NOT mean this canvas can be omitted from drawing.

Once any present happens:
    every visible gpu_canvas gets prepare_gpu()
    every visible gpu_canvas gets draw()
```

### Backend access

Do not expose heavy backend crate types from `gpui` core in V1.

Use opaque backend enum with lifetime and strong safety contract:

```rust
pub enum RawGpuAccess<'a> {
    D3d11(D3d11RawAccess<'a>),
    Metal(MetalRawAccess<'a>),
    Wgpu(WgpuRawAccess<'a>),
}
```

Backend structs contain opaque borrowed handles:

```text
NonNull<c_void> handles
target size
target format
device_generation
PhantomData<&'a ()>
```

Safety contract:

```text
handles valid only during callback
consumer must not store handles
consumer must not Release/drop native handles
consumer must not present
consumer must not mutate GPUI scene
consumer recreates app-owned GPU resources when device_generation changes
```

Typed helper APIs can be added later in platform/backend-specific crates. Do not
make V1 PR argue about `wgpu`, `windows`, or `metal` crate version coupling.

## Scene integration

Do not add `GpuCanvas` to:

```text
Primitive
PrimitiveKind
PrimitiveBatch
Scene::batches()
```

V1 uses two phase lists outside normal batch iterator:

```rust
pub enum GpuCanvasLayer {
    UnderScene,
    OverScene,
}

pub struct PaintGpuCanvas {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub driver: GpuCanvasHandle,
}

pub struct Scene {
    under_gpu_canvases: Vec<PaintGpuCanvas>,
    over_gpu_canvases: Vec<PaintGpuCanvas>,
}
```

`ContentMask` in current GPUI is rectangular bounds-only, so V1 clip/scissor is:

```text
bounds ∩ content_mask.bounds
```

### PaintOperation and replay

This is mandatory. Without it, partial scene reuse loses canvases or gets stale
draw order.

```rust
enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
    GpuCanvas {
        layer: GpuCanvasLayer,
        canvas: PaintGpuCanvas,
    },
}
```

Insert path:

```text
Window::paint_gpu_canvas(layer, bounds, driver)
  -> compute/snaps content_mask
  -> Scene::insert_gpu_canvas(layer, PaintGpuCanvas)

Scene::insert_gpu_canvas(...)
  -> clip bounds against content_mask
  -> if clipped empty: return
  -> compute order from current layer_stack / primitive_bounds
  -> push into under/over list
  -> push PaintOperation::GpuCanvas into paint_operations
```

Replay path:

```text
Scene::replay(range, prev_scene)
  PaintOperation::GpuCanvas(op) -> self.insert_gpu_canvas(op.clone())
```

Do not copy old `order` blindly during replay. Recompute it through the insert
path, same as normal primitives.

Finish path:

```text
Scene::finish()
  sort under_gpu_canvases by order
  sort over_gpu_canvases by order
  keep existing primitive sorting unchanged
```

Scene clear:

```text
Scene::clear()
  clears paint_operations
  clears under_gpu_canvases
  clears over_gpu_canvases
  clears existing primitive vecs
```

## Frame gate

The gate lives in the existing `Window::on_request_frame` closure.

Definitions:

```text
ui_dirty = invalidator.is_dirty() || force_render

existing_present_reason =
    request_frame_options.require_presentation ||
    window.needs_present ||
    high_rate_input

gpu_tick_allowed = platform/window throttle allows a GPU-only tick
```

Canonical pseudocode:

```text
let ui_dirty = invalidator.is_dirty() || force_render
let existing_present_reason =
    request_frame_options.require_presentation ||
    window.needs_present ||
    high_rate_input
let gpu_tick_allowed = gpu_clock_not_throttled()

if !ui_dirty && !existing_present_reason && !gpu_tick_allowed:
    window.complete_frame()
    return

arena_clear = None
if ui_dirty:
    if force_render:
        window.refresh()
    arena_clear = window.draw(cx) // fresh scene + fresh gpu_canvas lists

gpu_wants = false
if gpu_tick_allowed || ui_dirty || existing_present_reason:
    gpu_wants = frame_gpu_canvases(window.rendered_frame.scene)

needs_present =
    ui_dirty ||
    existing_present_reason ||
    gpu_wants

if needs_present:
    window.present()
else:
    window.complete_frame()

if arena_clear:
    arena_clear.clear()
```

Invariants:

```text
GPU-only throttle happens before mutating gpu_canvas.frame().
Dirty/force UI redraw is not suppressed by GPU-only throttle.
Existing present reasons are not suppressed by GPU-only throttle.
Do not call window.draw(cx) and then return only because GPU-only tick was throttled.
All visible gpu_canvas frame() calls happen before clear.
No short-circuit: poll every visible canvas even after first RequestPresent.
frame() must not dirty GPUI tree.
```

`frame_gpu_canvases(scene)`:

```text
for every visible under canvas:
    decision |= driver.frame(info)
for every visible over canvas:
    decision |= driver.frame(info)
return any RequestPresent
```

`GpuFrameInfo.bounds` and `scale_factor` come from the cached scene's current
canvas records. `presentable` comes from platform/renderer state.

## Frame-clock and platform pacing

Use the existing platform request-frame heartbeat. Do not introduce
`request_continuous_presentation`.

Do not globally cap source tick to 60 Hz. 120/144/240 Hz input/UI animations must
not regress. GPU canvas pacing is owned by `driver.frame() -> Skip`.

Throttle policy:

```text
active/cool: source tick not artificially slowed for gpu_canvas
inactive window: GPU-only ticks may be capped, e.g. ~30 Hz
thermal serious/critical: GPU-only ticks may be capped, e.g. ~60 Hz
dirty/force UI redraw: not suppressed by GPU-only throttle
existing present reasons: not suppressed by GPU-only throttle
```

### Windows

The `gpu_canvas` PR uses the existing upstream Windows heartbeat:

```text
VSyncProvider waits for vblank.
VSyncProvider calls RedrawWindow(..., RDW_INVALIDATE).
WM_PAINT reaches draw_window(false).
draw_window calls the existing request-frame closure.
Clean frames can complete_frame with no renderer draw/present.
```

This is enough for generic `gpu_canvas` correctness.

Known Windows perf issue, handled outside this PR:

```text
Under multi-window / input storm, WM_PAINT delivery can starve behind mouse/input traffic.
Present can block the UI thread under load.
```

That is a separate Windows pacing PR:

```text
posted private frame-clock message instead of RedrawWindow/WM_PAINT transport
frame-latency waitable swapchain pacing
max frame latency 1 where supported
wait for swapchain readiness outside blocking Present path
```

Do not bundle this Windows perf patch into the generic `gpu_canvas` PR unless
upstream explicitly requests it during review.

### macOS

Use existing `CVDisplayLink` path for visible windows.

```text
CVDisplayLink tick -> request-frame callback -> frame gate.
When occluded, display link may stop; frame() is not called.
On becoming visible again, normal display link path resumes.
```

Metal renderer rules:

```text
prepare_gpu runs before any active phase render encoder.
prepare_gpu may encode offscreen work using legal Metal encoder sequencing.
under phase uses one render encoder with Load/Clear as appropriate.
over phase uses one render encoder with Load.
driver.draw uses provided phase encoder only.
driver.draw must not open another render encoder.
```

### Linux / X11

Use existing per-window refresh timer based on monitor refresh.

```text
timer -> request-frame callback -> frame gate
Skip -> no renderer draw/present
RequestPresent -> wgpu path
```

### Linux / Wayland

Wayland compositor frame callbacks are tied to commits. If skipped frames do not
commit, compositor frame callback alone cannot be the GPU canvas decision clock.

Required fallback:

```text
if cached scene has visible gpu_canvas:
    arm calloop timer at refresh-ish interval
    timer invokes same request-frame closure
    if frame() -> Skip:
        no renderer draw
        no present
        timer re-arms itself
```

This is an app-side decision clock. It is not a compositor frame callback.

Implementation note for the current Zed snapshot:

```text
WaylandWindowStatePtr::frame() requests surface.frame(...) before invoking
the request-frame callback.

WaylandWindow::completed_frame() commits the surface when renderer did not
present a buffer.
```

That existing no-buffer commit path satisfies the fallback requirement for
skip-present `gpu_canvas` ticks: skipped GPU frames still complete the Wayland
frame callback cycle without clearing/drawing/presenting a GPU buffer. If an
upstream version removes this behavior, add the explicit calloop timer fallback
above instead of relying on compositor callbacks without commits.

## Presentability

`GpuFrameInfo.presentable` must be accurate enough to avoid work when a frame
cannot be presented, and must recover when presentation becomes possible again.

### Windows

Do not treat only width/height zero as non-presentable. DWM can still composite
occluded/non-visible windows for thumbnails or Alt-Tab.

Required state:

```text
last present status
occluded flag
device/swapchain lost state
```

If `Present` returns `DXGI_STATUS_OCCLUDED`:

```text
mark presentable=false
do not swallow the status via generic .ok()
while occluded, keep probing with Present(0, DXGI_PRESENT_TEST)
when probe returns S_OK, mark presentable=true
```

The probe must happen on frame ticks even while normal presents are skipped.
Otherwise `presentable=false` can become a trap.

### macOS

If display link stops under occlusion, `frame()` is not called. On visibility
return, display link restarts and presentability becomes true through normal path.

### Linux/wgpu

Surface lost/unconfigured/not acquired means not presentable for that frame.
Do not call app GPU callbacks if frame acquisition fails.

## Renderer sequence

On actual present:

```text
acquire drawable/surface/swapchain frame
if acquire fails/lost/not-presentable:
    update presentable/device-lost state
    do not call prepare_gpu()
    do not call draw()
    complete_frame or recover
    return

create command buffer / command encoder as backend requires
call prepare_gpu() for every visible under canvas
call prepare_gpu() for every visible over canvas
clear target
draw under gpu canvases
draw normal GPUI scene batches
draw over gpu canvases
submit/present
complete_frame
```

`prepare_gpu()` is called for every visible canvas on any actual present. The
renderer does not guess which canvas “needs preparation”. Driver cheap-noops.

### DX11 renderer

Required:

```text
D3d11StateGuard around app callbacks where state can be dirtied.
Set scissor to bounds ∩ content_mask for each canvas.
Provide opaque access to device/context/rtv/format/size/device_generation.
prepare_gpu may switch render targets for offscreen bake.
draw restores GPUI render target/state before returning to scene batches.
device_generation increments after device lost/recovery.
```

### Metal renderer

Required:

```text
Create command buffer before prepare_gpu.
No active scene/phase render encoder during prepare_gpu.
prepare_gpu may encode offscreen passes legally.
Under phase: one isolated render encoder for all under canvases.
Switch scissor per canvas.
End under encoder before normal scene if renderer needs separate scene encoder.
Over phase: one isolated render encoder for all over canvases.
driver.draw uses provided phase encoder only.
driver.draw must not open another render encoder.
```

### wgpu renderer

Required:

```text
Acquire current surface texture/view before prepare_gpu.
Create command encoder before prepare_gpu.
No active phase render pass during prepare_gpu.
prepare_gpu may open offscreen render passes legally.
Under phase: one render pass with Load/Clear as appropriate.
Switch scissor per canvas.
Normal GPUI scene batches draw after under.
Over phase: one render pass with Load.
driver.draw uses provided phase pass only.
driver.draw must not call begin_render_pass.
```

## Error and panic policy

`draw()` / `prepare_gpu()` return `anyhow::Result<()>`.

Recommended V1 behavior:

```text
Err:
    log backend + canvas context
    restore renderer state
    skip that canvas for the frame
    continue other canvases and GPUI scene where possible

panic:
    follows existing GPUI app panic policy
    renderer guards/drop still restore backend state where possible
```

Fatal device lost / surface lost / swapchain failure remains on existing renderer
recovery path.

## `over()` and popup/menu/tooltip

V1 `over()` means over the normal GPUI scene, not arbitrary z-order.

Risk:

```text
chart over-canvas draws crosshair/readout
context menu or tooltip opens over chart rect
over-canvas may cover menu/tooltip if those are drawn in same scene before over phase
```

Implementation must check early:

```text
Are GPUI popups/menus/tooltips separate platform windows?
Are they drawn in a late overlay after over_gpu_canvases?
If not, should over_canvas be hidden/gated while popup is open?
Should over_canvas be documented as over normal scene but below platform popups?
```

Validation must include popup/menu/tooltip over a GPU canvas.

## Unsupported platforms

The public API must not break `gpui_web`, test/headless, or platforms without native
GPU access.

Acceptable V1 behavior:

```text
unsupported backend compiles and acts as no-op/Skip
or gpu_canvas is cfg-gated with clear compile-time behavior
test platform can record/poll fake canvases for unit tests
```

## What not to carry forward

Do not expose these as public API:

```text
Window::add_gpu_pass
GpuPhase as old window-global subscription API
request_continuous_presentation
set_present_sync_interval
present_seq
renderer-global under_scene_passes / over_scene_passes subscriptions
```

Do not add `GpuCanvas` to normal primitive batching in V1:

```text
Primitive
PrimitiveKind
PrimitiveBatch
batches()
```

Do not add full frame-clock policies in V1 unless upstream review demands it:

```text
WhileVisible
OnDemand
max_fps
WakeAt
```

If reviewers push on CPU/battery for static canvases, minimal future extension:

```text
gpu_canvas(driver)                     // poll on dirty/present frames only
gpu_canvas(driver).poll_frame_clock()  // visible display ticks may call frame()
```

But V1 product path can use always-polled visible canvases because `frame()` is
cheap and `Skip` prevents clear/present.

## Implementation order

Use a clean upstream clone/branch.

1. Apply PR0 DPI restore bugfix separately if desired.
2. In main `gpu_canvas` branch, use existing platform heartbeat; do not bundle
   Windows posted tick / waitable swapchain pacing.
3. Add public API types:
   `gpu_canvas`, `GpuCanvas`, `GpuCanvasLayer`, `GpuCanvasDriver`,
   `GpuFrameInfo`, `GpuFrameDecision`, `GpuCanvasPrepareContext`,
   `GpuCanvasDrawContext`, opaque `RawGpuAccess`.
4. Add scene storage:
   `PaintGpuCanvas`, under/over Vecs, `PaintOperation::GpuCanvas`,
   `insert_gpu_canvas`, replay, finish, clear.
5. Add element implementation:
   Styled/layout/paint, `Window::paint_gpu_canvas`.
6. Modify window frame gate:
   ui_dirty/existing_present_reason/gpu_tick_allowed,
   no mutating `frame()` on throttled GPU-only tick,
   barrier polling, same-tick present.
7. Implement renderer hooks:
   DX11, Metal, wgpu.
8. Implement Windows presentability probe/recovery.
9. Implement Wayland timer fallback.
10. Add neutral example.
11. Add tests/diagnostics.
12. Port terminal to new API in a separate product branch.
13. Prepare separate Windows pacing PR for delivery/perf if needed.

## Validation checklist

Core frame behavior:

- [ ] Skip tick: no clear, no renderer draw, no Present.
- [ ] RequestPresent: `frame -> acquire -> prepare_gpu -> clear -> draw -> Present`
      in the same platform tick.
- [ ] No short-circuit: all visible canvases get `frame()` before clear.
- [ ] Once present happens for any reason, every visible canvas gets `prepare_gpu()`
      and `draw()`, including canvases that returned `Skip`.
- [ ] `frame()` does not call `cx.notify()` and does not mutate GPUI entity/view tree.
- [ ] GPU-only throttle does not suppress dirty/force UI redraws.
- [ ] GPU-only throttle does not suppress existing present reasons.
- [ ] GPU-only throttle cannot cause driver state/camera mutation without present.

Scene/lifecycle:

- [ ] Partial scene replay preserves gpu canvases.
- [ ] Replay recomputes order through insert path; no stale order copy.
- [ ] Hidden tab removes canvas from cached scene.
- [ ] Detached/moved tab does not leave canvas in old window.
- [ ] Stale handles are safe no-op.
- [ ] Multiple windows and multiple canvases keep ownership isolated.

Renderer/backend:

- [ ] Failed acquire / surface lost / not-presentable frame does not call app GPU callbacks.
- [ ] DX11 state/scissor/render target restored after callbacks.
- [ ] Metal prepare_gpu never runs inside active phase encoder.
- [ ] Metal draw never opens nested render encoder.
- [ ] wgpu prepare_gpu never runs inside active phase render pass.
- [ ] wgpu draw never calls begin_render_pass.
- [ ] device_generation changes force app resource recreation.
- [ ] `draw()` / `prepare_gpu()` Err does not poison renderer state.

Platform clocks for `gpu_canvas` PR:

- [ ] Windows existing VSyncProvider / RedrawWindow / WM_PAINT heartbeat drives frame gate.
- [ ] Windows `DXGI_STATUS_OCCLUDED` is detected and not swallowed.
- [ ] Windows occlusion recovers via `DXGI_PRESENT_TEST` while normal present is skipped.
- [ ] macOS visible windows tick through display link.
- [ ] Linux/X11 timer drives frame gate.
- [ ] Wayland timer fallback works without compositor frame callback.

Separate Windows pacing PR validation:

- [ ] Posted private frame-clock tick works under input storm.
- [ ] Waitable swapchain pacing does not block UI thread in Present.
- [ ] Multi-window/input-storm starvation is fixed without changing `gpu_canvas` API.

PR hygiene:

- [ ] No terminal/domain language in GPUI source.
- [ ] No public old raw-pass API.
- [ ] `gpui_web` / test / headless builds remain valid.
- [ ] Example is neutral and demonstrates skip without GPUI tree rerender.
- [ ] Popup/menu/tooltip over gpu canvas is validated.
