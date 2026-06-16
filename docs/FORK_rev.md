# FORK_rev — итоговая ревизия `ZED_FORK_V0.md` / `Terminal_patch_V0.md`

Status: final review notes, 2026-06-16. Этот файл сводит `FORK_rev1.md`-`FORK_rev4.md`
после проверки против текущего GPUI-кода, терминального chartdx-кода и исходной цели.

Это не самостоятельный план реализации. Это список правок, которые надо внести в
`ZED_FORK_V0.md` и `Terminal_patch_V0.md`, прежде чем считать их достаточными для
нового clean fork / upstream PR.

## Вердикт

Общее направление правильное:

```text
gpu_canvas = element-owned native GPU drawing hook
placement/lifetime/clip/window = GPUI tree
frame decision = before clear/present
draw = same platform tick
composition = under scene / over scene
```

Это лучше старого `Window::add_gpu_pass` / `request_continuous_presentation` и лучше
полной in-scene `PrimitiveBatch` интеграции для V0. Две фазы under/over дают сильно
меньше кода, меньше encoder/pass churn на Metal/wgpu и выше шанс на PR.

Но текущий `ZED_FORK_V0.md` ещё нельзя кодить вслепую. Ниже обязательные правки.

## Проверка против базовых целей

### 1. Идеально решить задачу терминала

Цель выполняется только если соблюдены все четыре условия:

```text
chart.frame() решает до clear/present;
если chart решил RequestPresent, draw идёт в тот же platform tick;
Skip не делает clear/render-pass/Present;
камера/phase state не мутируют на tick, который потом не будет presented.
```

Значит обязательны:

- `PaintOperation::GpuCanvas` для scene replay;
- throttle до mutating `frame()`;
- no short-circuit: все visible canvases опрошены до clear;
- `draw()` всех visible canvases на любом present;
- `advance_camera` только на пути, который реально ведёт к present.

### 2. Максимальные шансы upstream PR

Для PR надо продавать не терминальный хак, а общую фичу:

```text
phase-composited native GPU element with pre-present frame decision
```

Use cases: dense charts, custom cursor/crosshair overlays, game/editor viewport,
video/GPU preview, profiler/map/CAD panes.

PR-friendly границы:

- no terminal/domain code;
- no public `add_gpu_pass` / `request_continuous_presentation`;
- no in-scene `PrimitiveBatch` interleaving in V0;
- backend access strict, borrowed, frame-scoped;
- C1/DPI можно отдельно; `gpu_canvas` PR должен использовать существующий Windows
  heartbeat (`VSyncProvider -> RedrawWindow -> WM_PAINT -> on_request_frame`);
- C2 Windows pacing / waitable swapchain / posted tick держать отдельным perf PR,
  а не обязательной частью `gpu_canvas`.

### 3. Кросс-платформенность

Нельзя писать “Metal/wgpu offscreen bake потом” как будто это не часть задачи.
Если терминал должен быть быстрым на Mac/Linux, API должен уже в V0 иметь место
для offscreen resource work без нарушения Metal/wgpu encoder/pass правил.

Вывод: в V0 нужен не только `frame()` и `draw()`, а ещё optional GPU prepass:

```text
frame(info)       // CPU decision before clear
prepare_gpu(ctx)  // optional, no active scene pass, uploads/offscreen bake
draw(ctx)         // direct draw/composite in under/over phase pass
```

`prepare_gpu()` может быть default no-op для простых кейсов и примера. Но сам
hook должен быть в API/renderer contract сразу, иначе cross-platform ComboTex /
BookTex придётся ломать через “потом”.

### 4. Минималистично и строго

Минимализм тут не в том, чтобы выкинуть нужную фазу, а в том, чтобы разделить
ответственность без мутных правил:

```text
frame      = CPU decision, no GPU
prepare_gpu = GPU resource/offscreen work, no active GPUI scene pass
draw       = paint current pixels into provided under/over phase
GPUI       = owns bounds/clip/lifetime/window
renderer   = owns clear/present/pass boundaries/state restore
```

Это строже и понятнее, чем разрешать driver внутри `draw()` самому открывать
любые passes в момент, когда у renderer уже активен phase pass.

## Проверенные факты по коду

- `Scene::replay()` сейчас переигрывает только `paint_operations`. Если `gpu_canvas`
  хранить только в новых Vec, partial scene reuse потеряет canvas.
- `ContentMask` в GPUI сейчас прямоугольный (`bounds` only), поэтому scissor =
  `bounds ∩ content_mask.bounds` корректен для V0.
- Текущий `window.rs` считает throttle до `needs_present`. Новый `gpu_wants` нельзя
  просто добавить после throttle без уточнения политики.
- Windows upstream `VSyncProvider` действительно будит окна через `RedrawWindow`;
  delivery-fork C2 меняет доставку на posted vsync message.
- Linux/X11 уже имеет periodic refresh timer; Wayland привязан к frame callback после
  commit и требует отдельного timer fallback для skip-present сценария.
- В текущем терминале DX11 combo/orderbook реально делают offscreen bake в D3D RT.
  Metal/wgpu chart path сейчас direct-draw в предоставленный encoder/pass, без
  equivalent offscreen bake.

## Обязательные правки к `ZED_FORK_V0.md`

### 0. Upstream PR strategy: разделить generic feature и Windows perf

PR0/C1 DPI restore bugfix можно и нужно слать отдельно: это маленький независимый
generic баг, и его судьба не должна блокировать терминал.

Правильная стратегия:

```text
PR0: Windows DPI restore bugfix — отдельно.
PR1: gpu_canvas — generic retained GPU canvas API.
PR2: Windows pacing / waitable swapchain / posted vsync tick — отдельный perf PR.
C3: floating tool-window style — отдельно / optional / delivery-only.
```

Причина: на чистом upstream Windows уже есть heartbeat:

```text
VSyncProvider -> RedrawWindow -> WM_PAINT -> draw_window -> on_request_frame
```

Этого достаточно, чтобы `gpu_canvas` функционально работал: callback получает
frame opportunity, сам решает рисовать или no-op, а чистый frame не должен dirty'ть
GPUI views. C2 чинит другой слой — Windows latency/perf под input storm,
мультиокнами и blocking Present. Это критично для delivery-форка терминала, но
не является семантической частью generic `gpu_canvas`.

Итого: C2 не вбандливать в `gpu_canvas` PR, если upstream сам не попросит. В
`gpu_canvas` PR оставить только минимальные platform hooks, без которых canvas
не может быть корректен на конкретной платформе. Например, Wayland timer fallback
остаётся внутри `gpu_canvas`, потому что без present compositor может перестать
давать callbacks, и canvas реально теряет frame opportunities.

Код всё равно держать по коммитам:

```text
commit 1: gpu_canvas scene/replay/gate
commit 2: renderer hooks DX11/Metal/wgpu
commit 3: platform-specific correctness fallbacks, if any
commit 4: example/tests/docs
```

А Windows pacing PR после этого формулировать отдельно:

```text
Improve Windows frame-clock delivery under input/multi-window load.
Move the vblank tick away from WM_PAINT starvation and avoid blocking Present on
the UI thread via waitable swapchain pacing.
```

### 1. Scene replay/cache integration

Если `gpu_canvas` не идёт в `Primitive` / `PrimitiveKind` / `PrimitiveBatch`, это
правильно для короткого V0. Но он обязан попасть в `PaintOperation`, иначе GPUI
scene reuse его потеряет.

Требуемая форма:

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

И дальше:

```text
Scene::clear() clears under/over gpu_canvas vecs
Scene::replay() replays PaintOperation::GpuCanvas
Scene::finish() sorts under/over lists by order
cached rendered_frame.scene is the source for frame-gate polling
```

Важно: replay должен идти через insert path, а не тупым копированием старого
`order`.

```text
Scene::replay()
  PaintOperation::GpuCanvas(op) -> self.insert_gpu_canvas(op.clone())

insert_gpu_canvas()
  clips bounds/content_mask
  computes order from current layer_stack / primitive_bounds
  pushes into under/over gpu_canvas vec
  pushes PaintOperation::GpuCanvas(...)
```

Иначе partial scene reuse может получить stale draw order. Payload в
`PaintOperation::GpuCanvas` должен быть cloneable: driver хранится через stable
handle (`Rc`/`Arc`/id-handle), не как raw non-clone object.

Без этого пункт A1 неполный.

### 2. Frame gate + throttle order

Нельзя мутировать `frame()` драйвера, если этот tick потом будет зарезан window
throttle. Для live chart это означает: камера уехала, кадра нет.

Правильная схема:

```text
let ui_dirty = invalidator.is_dirty() || force_render
let existing_present_reason =
    request_frame_options.require_presentation ||
    window.needs_present ||
    high_rate_input
let gpu_tick_allowed = gpu_clock_not_throttled()

if !ui_dirty && !existing_present_reason && !gpu_tick_allowed:
    window.complete_frame()
    return                       // no scene rebuild, no mutating frame()

arena_clear = None
if ui_dirty:
    arena_clear = window.draw(cx) // rebuild scene + fresh canvas lists

gpu_wants = false
if gpu_tick_allowed || ui_dirty || existing_present_reason:
    gpu_wants = poll frame() of all visible gpu_canvas, no short-circuit

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

Нюанс: текущий GPUI throttle не обязан капать dirty UI redraw. Для `gpu_canvas`
надо считать наличие visible gpu_canvas / gpu clock interest причиной, к которой
применяется inactive/thermal cap. В active/cool режиме источник не прорежаем:
пейсинг делает сам chart через `Skip`.

Инварианты:

```text
GPU-only throttle happens before mutating gpu_canvas.frame().
Dirty/force UI redraw is not suppressed by GPU-only throttle.
Existing present reasons are not suppressed by GPU-only throttle.
Do not call window.draw(cx) and then return solely because GPU-only tick was throttled.
```

### 3. Windows `presentable` must recover while skipped

`presentable=false` не должен стать ловушкой.

Если Windows поймал `DXGI_STATUS_OCCLUDED`, canvases начнут возвращать `Skip`, и
реального `Present` может больше не быть. Поэтому проверка восстановления должна
жить вне обычного present path:

```text
when occluded:
    on frame ticks, run Present(0, DXGI_PRESENT_TEST)
    update presentable state until S_OK
```

Иначе окно может не узнать, что снова стало presentable.

### 4. `RawGpuAccess`: opaque backend enum, not heavy typed core API

Идея enum по backend хорошая. Но полностью typed public API с `wgpu::Device`,
`wgpu::RenderPass`, `metal::*`, `windows::*` в `gpui` core создаёт dependency /
version-lock риск.

Для V0 безопаснее:

```rust
pub enum RawGpuAccess<'a> {
    D3d11(D3d11RawAccess<'a>),
    Metal(MetalRawAccess<'a>),
    Wgpu(WgpuRawAccess<'a>),
}
```

А внутри backend structs:

```text
NonNull<c_void> / opaque borrowed handles
format
size
device_generation
PhantomData<&'a ()>
```

Контракт:

```text
valid only during callback
do not store
do not Release/drop native handles
do not present
do not mutate GPUI scene
recreate app resources when device_generation changes
```

Typed helpers можно добавить позже в platform/backend crates, но не тащить это в
первый PR как обязательный спор.

### 5. Metal/wgpu contract: add optional GPU prepass in V0

`ZED_FORK_V0.md` сейчас смешивает два контракта:

```text
WgpuAccess gives encoder/view
но A6 говорит one render-pass per phase
```

Нужно выбрать один. Для V0 лучше:

```text
frame() decides on CPU before clear/present;
renderer calls prepare_gpu() before any under/over scene pass;
renderer opens one under/over phase encoder/pass;
driver.draw() only direct-draws/composites into the provided phase pass;
driver.draw() must not open another render pass/encoder.
```

Почему `prepare_gpu()` обязателен уже в V0:

```text
Metal cannot open another render encoder while phase encoder is active.
wgpu cannot open another render pass while phase render pass is active.
DX11 currently bakes ComboTex/BookTex by switching render targets.
Mac/Linux need an equally legal place for uploads/offscreen bake.
```

API shape:

```rust
pub trait GpuCanvasDriver {
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision;

    fn prepare_gpu(&mut self, ctx: &mut GpuCanvasPrepareContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn draw(&mut self, ctx: &mut GpuCanvasDrawContext) -> anyhow::Result<()>;
}
```

Вызов:

```text
if needs_present:
    acquire current GPU frame/drawable/surface texture
    create command buffer / encoder as needed
    prepare_gpu() for every visible gpu_canvas
    clear target
    under draw()
    GPUI scene batches
    over draw()
    Present
```

`prepare_gpu()` вызывается для каждого visible canvas на любом actual present,
не только для canvas, который вернул `RequestPresent`. Renderer не должен гадать,
кому “нужна подготовка”: device generation, drawable/surface recreation, missing
resources и pending uploads лучше знает сам driver.

`prepare_gpu()` не должен быть “обязательной тяжёлой работой” на каждый present.
Driver обязан быстро no-op'нуться, если нечего делать:

```rust
fn prepare_gpu(&mut self, ctx: &mut GpuCanvasPrepareContext) -> anyhow::Result<()> {
    if self.device_generation != ctx.device_generation() {
        self.recreate_resources(ctx)?;
    }
    if self.has_pending_upload_or_bake() {
        self.upload_or_bake(ctx)?;
    }
    Ok(())
}
```

Если acquire failed / surface lost / frame not presentable:

```text
do not call prepare_gpu()
do not call draw()
update presentable/device-lost state
complete_frame or recover through existing renderer path
```

### 6. Callback error / panic policy

`draw() -> anyhow::Result<()>` без политики недостаточен.

Рекомендованный V0-контракт:

```text
draw() Err:
    log backend + canvas context
    restore renderer state
    skip this canvas for this frame
    continue other canvases / GPUI scene

panic:
    follows existing GPUI app panic policy
    renderer guards/drop still restore backend state where possible
```

Device-lost/fatal renderer errors отдельно: они могут идти по существующему
renderer recovery path.

### 7. Wayland fallback must be concrete

Не оставлять “Wayland validation item” как туман. Минимальная форма:

```text
if cached scene has visible gpu_canvas:
    arm calloop timer at refresh-ish interval
    timer invokes same request_frame closure
    if frame() -> Skip:
        no renderer draw, no present
        timer re-arms itself
```

Это app-side decision clock, не compositor frame callback. Иначе “нет commit ->
нет следующего callback” снова ломает live chart.

### 8. Phase semantics must be honest

`gpu_canvas` в V0 не равен CPU `canvas()` по z-order semantics. Он element-owned
по bounds/lifetime, но composited только в двух фазах:

```rust
gpu_canvas(driver)        // under whole GPUI scene
gpu_canvas(driver).over() // over whole GPUI scene
```

Документация PR должна говорить:

```text
V0 is phase-composited, not arbitrary draw-order interleaving.
In-scene interleaving is future work.
```

Иначе reviewer справедливо спросит, почему `canvas()` в названии, но normal paint
order не поддержан.

Название можно оставить `gpu_canvas`, но альтернативы для обсуждения:

```text
gpu_layer
native_gpu_layer
gpu_viewport
```

### 9. `over()` vs popup/menu/tooltip

Так как `over()` рисует поверх всей GPUI scene, он может оказаться выше popup /
menu / tooltip внутри того же rect. Для chart crosshair/readout это надо проверить
руками и тестом.

Добавить validation:

```text
popup / tooltip / context menu над chart не перекрывается over-canvas неправильно
```

Если GPUI popup рисуется отдельным окном или late overlay после over-canvas —
зафиксировать факт. Если нет — terminal должен скрывать/гейтить over-canvas при
открытом popup либо не использовать over там, где это ломает UI.

### 10. Frame-clock policy wording

Не надо тащить старые `WhileVisible` / `OnDemand` / `max_fps` в V0. Но и жёсткое
“никогда не вводить policies” лучше убрать.

Формулировка:

```text
V0 uses the existing platform request-frame heartbeat and lets driver.frame()
return Skip. More explicit clock policies are deferred unless upstream review
requires them.
```

Если reviewer упрётся в battery/CPU для static canvas, минимальное расширение:

```text
gpu_canvas(driver)                     // poll only on dirty/present frames
gpu_canvas(driver).poll_frame_clock()  // visible display ticks may call frame()
```

Но для нашего terminal path V0 с always-polled visible canvases достаточен.

### 11. Unsupported platforms

Public API не должен ломать `gpui_web`, test/headless и платформы без native GPU
access.

Добавить:

```text
unsupported backend = no-op/Skip or cfg-gated element
gpui_web/test/headless builds are validation items
```

### 12. Driver callbacks must not dirty GPUI tree

`GpuCanvasDriver::frame()` — часть GPU frame gate, а не обычного GPUI render path.
Он может мутировать только driver-owned retained state.

Запрещено в `frame()`:

```text
cx.notify()
mutate GPUI entity tree
trigger layout
schedule per-vblank/per-mousemove UI invalidation
```

Если driver понял, что нужен обычный UI update, он должен выставить свой редкий
UI dirty flag, а приложение уже отдаст его в throttled `cx.notify()` path.

Для терминала:

```text
pixel-cross / mouse-move / cursor / readouts / orderbook GPU pixels -> gpu_canvas path
axis text / toolbar / panel UI -> rare cx.notify() path
```

### 13. Present reason and draw obligation are separate

Не смешивать “кто попросил present” и “кого надо рисовать”.

```text
frame() -> RequestPresent means this canvas wants a present.
frame() -> Skip does NOT mean this canvas may be omitted from drawing.

Once a present happens for any reason:
    every visible gpu_canvas gets prepare_gpu()
    every visible gpu_canvas gets draw()
```

Это защищает от бага: canvas A попросил present, canvas B вернул `Skip`, но после
clear B всё равно обязан нарисовать текущие пиксели и синхронизировать ресурсы.

## Правки к `Terminal_patch_V0.md`

### 1. Phase-clean default scale remains exact

Сохранить исходный инвариант дефолтного окна около 60 секунд. `effective =
monitor / round(monitor / 60)` допустим только как `present_rate` для `dt`, а не
как замена формулы.

Формула остаётся:

```text
dt = 1000 / effective_present_rate
s0 = chart_w * dt / 60000
if s0 >= 1:
    shift = max(1, round(s0))
else:
    shift = 1 / max(1, round(1 / s0))
px_per_ms = shift / dt
window = chart_w / px_per_ms
```

Нельзя возвращаться к жёсткому `1px за frame` как дефолту.

### 2. `advance_camera` only on present path

Пункт B2 правильный и критичный:

```text
pixel-cross? yes -> advance_camera + RequestPresent
pixel-cross? no  -> Skip, no camera mutation
```

Если frame gate/window throttle может зарезать tick, `advance_camera` не должен
быть вызван до этого решения.

### 3. Mouse-move path: over-canvas is correct, but atlas invalidation required

Перенос crosshair + readouts из GPUI `cx.notify()` path в `gpu_canvas(...).over()`
правильный. Это закрывает per-move top-down render.

Добавить:

```text
theme change invalidates readout glyph atlas
DPI/scale change invalidates atlas
font/text-size change invalidates atlas
popup/menu-over-chart behavior validated
```

Readout atlas — маленький numeric/time atlas, не полноценный text engine.

### 4. D3D offscreen bake must become cross-platform, not future fog

Текущий DX11 path имеет ComboTex/BookTex offscreen bake. Текущие Metal/wgpu paths
рисуют direct в target/phase pass.

Против базовой цели “быстрый кросс-платформенный терминал” нельзя оставить это как
“потом”. Mac bench с direct chartdx уже показывал слабое место на многих окнах.

Правка к `Terminal_patch_V0.md`:

```text
V0 terminal uses prepare_gpu() to support resident/offscreen layer updates
legally on DX11, Metal and wgpu.

DX11 may keep existing render-target switch internally.
Metal/wgpu move uploads/offscreen bake to prepare_gpu(), before phase pass.
draw() composites/direct-draws only into the under/over phase target.
```

Если какой-то слой на Metal/wgpu остаётся direct-draw без offscreen cache, это
должно быть сознательное измеренное решение по конкретному слою, а не дырка API.

### 5. Detach/hidden tab assumption remains a must-test

B4 правильно помечает риск: cached scene старого окна должен потерять canvas после
detach/hidden. После добавления `PaintOperation::GpuCanvas` этот риск особенно
важно проверить через scene replay/rebuild.

Validation:

```text
detach tab -> old window scene has no canvas
hidden tab -> no canvas polling, no GPU work
stale handle/request is safe no-op
```

## Что убрать / перенести из основного плана

Не держать это в core spec `gpu_canvas`:

- C3 tool-window style как обязательный пункт. Это delivery/optional UX PR.
- Длинные подробности C1/C3 рядом с API. Лучше отдельный `ZED_PORTS_V0.md`
  или appendix.
- C2 подробности из core spec `gpu_canvas`. Держать C2 как отдельный Windows
  perf PR/appendix: posted tick, waitable swapchain, input-storm starvation.
- “Нет кейсов для z-between-UI-primitives”. Заменить на V0 boundary:
  `in-scene interleaving is future work`.
- Пример “треугольник/партиклы, не курсор” — это PR notes, не implementation
  invariant.
- Категоричные запреты на future frame policies.

## Перед кодингом

`FORK_rev.md` не должен остаться “замечаниями сбоку”. Перед началом clean fork
implementation надо обновить canonical docs:

```text
ZED_FORK_V0.md
Terminal_patch_V0.md
```

Иначе следующий агент легко начнёт кодить по старому V0 и пропустит самые
важные исправления: `PaintOperation::GpuCanvas`, `prepare_gpu`, throttle-before-
mutating-frame, opaque backend access, Wayland timer fallback и новые validation
items.

## Что из старых ревизий НЕ переносить буквально

- `prepare_gpu()` как “всё всегда делает тяжёлую работу” — не переносить. Но сам
  optional hook нужен в V0, потому что без него Metal/wgpu offscreen bake законно
  некуда положить.
- Полностью typed backend handles в `gpui` core — не переносить. Берём opaque
  backend enum + lifetime + строгий safety contract.
- `WakeAt` / full OnDemand policy как обязательный API — не переносить в V0.
  Оставить как deferred review-response.

## Минимальная итоговая форма реализации

```text
0. PR strategy:
   - PR0 DPI restore bugfix separately;
   - PR1 gpu_canvas uses existing Windows heartbeat;
   - PR2 Windows pacing / waitable swapchain / posted tick is a separate perf PR.
1. Add gpu_canvas element: Styled/layout/clip/lifetime from GPUI tree.
2. Add PaintGpuCanvas + GpuCanvasLayer { UnderScene, OverScene }.
3. Add PaintOperation::GpuCanvas so scene replay/cache works.
4. Scene stores under/over gpu_canvas vecs, sorted by order in finish().
5. Window gate:
   - compute ui_dirty, existing present reason, and gpu_tick_allowed first;
   - do not suppress dirty/force UI redraw with GPU-only throttle;
   - do not suppress existing present reasons with GPU-only throttle;
   - do not mutate gpu frame state on a throttled GPU-only tick;
   - rebuild scene if ui_dirty;
   - poll all visible gpu_canvas frame() as a barrier;
   - present same tick if dirty/force/existing present/gpu_wants;
   - otherwise complete_frame with no clear/present.
6. Renderers:
   - acquire drawable/surface frame before app GPU callbacks;
   - if acquire fails/lost/not-presentable: no prepare_gpu/draw callbacks;
   - prepare_gpu for every visible canvas before phase passes when presenting;
   - under gpu canvases;
   - normal GPUI batches;
   - over gpu canvases;
   - restore state and scissor per backend.
7. RawGpuAccess is opaque backend enum with device_generation.
8. No public add_gpu_pass / request_continuous_presentation / present_seq.
9. Terminal migrates chart state into retained driver, removes pass guards/task,
   and moves per-move crosshair/readouts out of cx.notify path.
```

Эта форма остаётся короткой, решает нашу задачу и выглядит наиболее PR-friendly.

## Validation additions to merge into main docs

- Partial scene replay preserves gpu canvases.
- Skip tick: no clear, no renderer draw, no Present.
- RequestPresent: `frame -> acquire -> prepare_gpu -> clear -> draw -> Present`
  in the same platform tick.
- Window throttle cannot cause camera/state mutation without present.
- GPU-only throttle does not suppress dirty/force UI redraws or existing present reasons.
- Dirty UI frame draws every visible canvas, even canvases that returned `Skip`.
- `prepare_gpu()` is called for every visible canvas on any actual present and
  cheap-noops when nothing is pending.
- Failed acquire / surface lost / not-presentable frame does not call app GPU callbacks.
- Windows occlusion recovers via `DXGI_PRESENT_TEST` even while normal present is skipped.
- Wayland timer fallback works without compositor frame callback.
- `draw()` error in one canvas does not poison renderer state for the rest.
- Metal/wgpu offscreen bake/uploads happen only in `prepare_gpu()`, never inside an active phase pass.
- Direct-draw-only Metal/wgpu layers are explicitly measured and accepted per layer, not assumed.
- `frame()` does not call `cx.notify()` and does not mutate GPUI view/entity tree.
- API builds on gpui_web/test/headless.
- Popup/menu/tooltip over chart does not get covered by over-canvas.
- 4 visible charts in one window: all frame() calls before clear, all draw() calls on present,
  no Orders/Shell render storm.
