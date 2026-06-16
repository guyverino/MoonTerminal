# ZED_FORK_F1 — audit findings after mousemove/render review

Дата: 2026-06-17.

Контекст: проверка текущих терминальных патчей:

- `ba6d540 fix(chart): move cursor crosshair to native gpu layer`;
- `da1d53b fix(chart): prepare dx11 layers before draw`;
- `9a61259 fix(chart): prepare data without present sequence`;
- `a460ded fix(chart): gate gpu prepare by dirty state`;
- `185d4c8 feat(chart): cache dx11 base for cursor-only frames`;

и локального GPUI fork
`2d9dd2a947 fix(gpui): throttle only gpu-only canvas ticks`.

Это не replacement для `ZED_FORK_V1.md`. Это список находок, которые надо
перенести в следующий план/патч, чтобы не получить красивый API с плохой
семантикой в терминале.

## Current status after follow-up fixes

Закрыто кодом:

- Finding 1: старый `present_seq` loop удалён (`9a61259`), `prepare()` больше
  не ставит `needs_present` безусловно (`a460ded`).
- Finding 2 для Windows/DX11: добавлен full-window base cache (`185d4c8`).
  Cursor-only DX11 present теперь должен идти как:

```text
base_blit
cursor_draw
```

  а не как полный `bg/grid/combo/book/userdata` replay.
- `chart_gpu_prepare` на DX11 теперь gated by `gpu_prepare_dirty`; cursor-only
  кадр не должен гонять `combo.prepare/orderbook.prepare/userdata.prepare`.

Открыто:

- Runtime `MOON_RENDER_DIAG=1` ещё должен подтвердить частоты на живом окне:
  `base_blit/cursor_draw` растут на mousemove, а `combo_draw/book_draw/user_draw`
  и `orders_render/shell_render` не улетают в monitor/mouse rate.
- Readout chips всё ещё GPUI throttled path (`notify_cursor_readout_if_due`),
  не native overlay.
- Metal/wgpu path пока direct-draw stack on present. Он компилируется, но не
  получил такой же base-cache path, как DX11.
- Mac GUI/runtime проверка не закрыта локально; нужен Mac host result.

## Цель, против которой проверяем

Нужная модель:

```text
mousemove:
  update cursor/readout state
  request one present if pixels changed

present:
  restore already baked base image
  draw cursor/readouts overlay
  no GPUI tree dirty
  no Orders/Shell render storm
  no redraw of all chart layers just because cursor moved
```

MoonBot reference:

- `ImageMouseMove` sets `RepaintCursor` for cursor-only movement.
- `TChartVSyncPacer.WaitFrame` wakes the UI task on vblank.
- `UpdateDrawTimerTimer` decides whether anything must repaint.
- Cursor-only path reaches `ImagePaint`, which blits cached `bm` and draws the
  cheap overlay. It does not rebuild the whole UI tree and does not recompute
  the whole chart base for cursor movement.

## Finding 0 — NOTE: latest DX11 prepare split is correctness, not the perf fix

`da1d53b` вынес DX11 offscreen bake/upload из layer draw в
`GpuCanvasDriver::prepare_gpu`.

Это правильный backend-level фикс: `combo.prepare`, `orderbook.prepare` и
`userdata.prepare` могут переключать render target, поэтому им место до
основного draw, а не внутри слоя, который уже пишет в window backbuffer.

Сам по себе этот split не закрывал mousemove/perf проблему:

- cursor-only present теперь гоняет не только full layer draw, но и
  `prepare_gpu` по всем активным панелям;
- большинство prepare-вызовов может быть cheap/no-op, но обход всех pane и
  dirty-проверки всё равно идут на каждый present;
- финального composited base cache тогда ещё не было.

Правильный следующий шаг — не откатывать `prepare_gpu`, а поставить поверх него
строгую dirty-модель: base rebuild только при `base_dirty`, cursor/readout
present — только base blit + overlay.

## Finding 1 — RESOLVED: prepare could re-arm continuous present

Old flow could self-sustain:

```text
mousemove
  -> ChartEngine::set_cursor(...)
  -> RenderState.needs_present = true

gpu frame tick
  -> GpuCanvasDriver::frame() returns RequestPresent
  -> draw_gpu()
  -> present_seq += 1

16ms chart task
  -> sees present_seq changed
  -> calls chart.prepare(...)
  -> prepare unconditionally sets st.needs_present = true

next gpu frame tick
  -> RequestPresent again
  -> repeats
```

Evidence:

- old `crates/moon-ui-gpui/src/panels/chart.rs`: 16ms task called
  `chart.prepare(...)` whenever `present_seq` changed.
- old `crates/moon-ui-gpui/src/chartdx/mod.rs`: `ChartEngine::prepare(...)`
  ended with `st.needs_present = true`.
- After `da1d53b`, a re-armed present also runs `prepare_gpu` before draw.

Current code:

- `present_seq` is gone.
- visible data pump prepares data without using draw/present count.
- `prepare()` sets `base_dirty/needs_present` only when it detected changed
  GPU-visible state or cursor params.
- no-op prepare leaves `needs_present` unchanged.

Why this was bad:

`gpu_canvas` is supposed to be by-need. A single present must not automatically
create the next present unless visible pixels actually changed. With the old
mechanics, a cursor move or any other first present could accidentally become
continuous-present again.

Required fix direction:

- `prepare()` must not unconditionally request present.
- It must set dirty/request-present only when it actually changed visible GPU
  state:
  - new combo data uploaded;
  - orderbook texture/base changed;
  - userdata/orders changed;
  - camera crossed a pixel;
  - geometry/theme changed;
  - cursor/readout changed.
- No-op prepare after a successful draw must leave `needs_present=false`.
- Dirty flags should be consumed only after successful draw or successful base
  cache update.

## Finding 2 — PARTIAL: cursor-only present prepared/redrew the whole chart stack

Old cursor-only frame did not only draw cursor.

Old D3D path:

```text
prepare_gpu()
  for every active pane:
    combo.prepare
    orderbook.prepare
    userdata.prepare

draw_gpu()
  draw window background/logo
  for every active pane:
    draw background
    draw grid
    draw combo
    draw orderbook
    draw userdata/orders
    draw cursor
```

Evidence:

- `crates/moon-ui-gpui/src/chartdx/mod.rs`: `draw_gpu()` loops all active panes.
- `crates/moon-ui-gpui/src/chartdx/mod.rs`: `prepare_gpu()` also loops all
  active panes.
- `crates/moon-ui-gpui/src/chartdx/backend.rs`: `render_d3d()` bumps/draws
  `bg_draw`, `grid_draw`, `combo_draw`, `orderbook_draw`, `userdata_draw`,
  then `cursor_draw`.
- Mac/WGPU paths were extended similarly with cursor params.

Current DX11 code:

- `RenderState::prepare_gpu()` checks `gpu_prepare_dirty`; cursor-only frames
  skip layer upload/bake.
- `RenderState::draw_gpu(D3d11)` uses `BaseCache`:

```text
if base_dirty or target changed:
  bake base texture: window bg/logo + per-pane bg/grid/combo/book/userdata
always:
  blit base texture to window
  draw cursor overlay
```

Current gap:

- Metal/wgpu paths still direct-draw the stack on every present.
- Runtime counters must still prove DX11 behaves as intended under real mousemove.

Why this is bad:

The agent treated "resident GPU layers" as "cached enough". That is false.

`combo` and `orderbook` not rebaking is good, but it is not enough. A cursor
move still replays all chart layers as separate GPU work. For 4 visible panes,
moving cursor over one pane still prepares/checks and draws the whole active
pane set. This is not the MoonBot pattern.

Remaining fix direction:

- Introduce a base/composite cache for chart pixels without high-frequency
  cursor/readout overlay.
- Mousemove-only present should be:

```text
blit base texture(s)
draw cursor/readouts overlay
```

- The base cache should be the whole chart canvas/slot image without the
  high-frequency cursor/readout overlay. Its job is intentionally simple:
  restore the exact pixels under the moving cursor.
- The base image includes everything that does not change because of cursor
  movement: background, grid, combo history, orderbook, userdata/orders, static
  chart decorations, and preferably static axis gutters if they are visually part
  of the chart.
- Do not split `plot_base + book_base + userdata_base` in the first PR/patch.
  That is unnecessary complexity for this bug and increases the chance of
  sync/bounds/order bugs. Split only later if measurement proves the single base
  texture is the bottleneck.
- For cursor-only movement, `userdata/orders` must not run as a separate layer
  draw. It must already be baked into the base.

## Finding 3 — MAJOR: readout chips are still GPUI, not native overlay

The crosshair lines moved to native `chartdx`, but price/time readout chips did
not.

Current behavior:

```text
mousemove
  -> native cursor position updated
  -> every >=250ms notify_cursor_readout_if_due()
  -> cx.notify()
  -> GPUI render path updates readout chips
```

Evidence:

- `ChartPanel::notify_cursor_readout_if_due(...)` throttles readout notify to
  250ms.
- `axes::draw(..., cursor, false, ...)` still paints readout chips in GPUI.
- `cursor_lines=false` disables stale GPUI crosshair lines, but not readout
  chips.

Why this is bad:

It prevents the "mousemove never dirties GPUI tree" goal. It is much better than
240Hz notify, but still not ideal. It can still wake Shell/Orders top-down up to
4Hz and it makes readouts less smooth than the native cursor.

Required fix direction:

- Move readout chips to the native overlay too, or
- build a retained text/atlas path for readouts that updates GPU state without
  `cx.notify()`.
- Until that is done, docs must say "crosshair lines native; readouts throttled
  GPUI", not "cursor fully native".

## Finding 4 — MAJOR: ToBeFixed.md mixes bake, draw, and GPUI render

`docs/ToBeFixed.md` is better than before but still too optimistic.

The doc says graph/book/cursor are cached, but it must distinguish:

- `bake`: expensive rebuild/upload of a cache texture;
- `draw/blit`: GPU work on every present;
- `Render::render`: CPU GPUI tree render;
- `notify`: cause that can dirty GPUI view tree.

Correct statement:

- `combo_bake` and `orderbook_bake` should stay near zero on mousemove.
- Before `185d4c8`, `chart_gpu_prepare` still ran on every chart present for
  every active pane after `da1d53b`.
- Before `185d4c8`, `combo_draw`, `orderbook_draw`, `userdata_draw`,
  `grid_draw`, `bg_draw` still happened on every DX11 chart present.
- `orders_render` should not happen on every mousemove if cursor-only does not
  call `cx.notify()`, but this is a runtime fact and must be verified with
  `MOON_RENDER_DIAG=1`.

Required diagnostic targets:

```text
Pure mousemove over chart:
  chart_input_notify ~= 0/s
  chart_cursor_readout_notify <= 4/s while readouts are still GPUI
  orders_render and shell_render must not rise to monitor/mouse rate
  cursor_draw follows cursor presents
  combo_bake ~= 0
  orderbook_bake ~= 0

After base cache fix:
  chart_gpu_prepare should only rebuild/update the base when base_dirty is set
  combo_draw/orderbook_draw/userdata_draw should not rise on cursor-only frames
  chart_base_blit should rise instead
```

## Finding 5 — MAJOR: dirty ownership is blurred

Right now `prepare`, `frame`, input handlers, and present sequence are coupled in
a way that makes it hard to reason about "who requested the next frame".

Required shape:

```text
state:
  base_dirty
  cursor_dirty
  readout_dirty

causes that set base_dirty:
  new market/orderbook/userdata pixels
  live camera pixel-cross
  pan/zoom/scale/follow change
  geometry/resize/DPI/theme/device change

mousemove:
  cursor_dirty = true
  maybe readout_dirty = true

prepare(session):
  uploads new data if any
  marks base_dirty only if base pixels changed
  does not request present for no-op work

frame(info):
  if !presentable: Skip
  if live camera pixel-cross: base_dirty = true
  if base_dirty || cursor_dirty || readout_dirty: RequestPresent
  else Skip

prepare_gpu(ctx):
  if base_dirty: rebuild/update the whole chart base texture without cursor
  if readout_dirty: update overlay atlas/instances

draw(ctx):
  blit chart base texture
  draw cursor/readouts
  clear dirty flags after success
```

This makes the terminal behavior explainable and gives the Zed PR a clean story:
the framework asks, the app decides, and a no-op tick stays a no-op.

## Finding 6 — PARTIAL: multi-chart amplified the full-stack replay mistake

With several visible panes/charts in one window, the old DX11 path and the
current Metal/wgpu direct-draw paths loop active panes on every present.

For cursor movement over one pane, that cost is effectively:

```text
N active panes * (background + grid + combo + orderbook + userdata) + cursor
```

Desired cost:

```text
one chart base blit + cursor/readout overlay for hovered pane
```

For the current ChartPanel/container, the base should cover the whole chart
canvas/slot, including all visible panes. Cursor movement does not invalidate
that base. If the app later has multiple independent chart widgets in one GPUI
window, each widget can own one base; still do not split by plot/book/orders
unless measurements force it.

## Finding 7 — GPUI scene replay still happens on every present

`Window::present()` calls `platform_window.draw(&rendered_frame.scene)` even
when there is no GPUI dirty tree. On Windows this means clear + drawing the
cached scene buffers + gpu canvases.

This is not the same as CPU `Render::render`, but it is still GPU work. The
terminal must not rely on "cached GPUI scene" as a substitute for chart base
caching.

For the upstream PR this is acceptable if documented honestly:

- `gpu_canvas` avoids dirtying/re-rendering GPUI views;
- it does not promise OS-level partial window redraw;
- apps with cursor/video/chart workloads should use retained base textures if
  their own canvas has expensive layers.

## PR implications for Zed

The generic `gpu_canvas` idea is still valid and useful. These findings mostly
hit the terminal implementation, not the existence of the API.

But the PR story must be precise:

- Do not sell `gpu_canvas` as "custom cursor is free" unless the example uses a
  retained base + overlay model.
- The API must guarantee same-tick decision:
  `frame()` decides before clear/present, and `draw()` runs in the same tick if
  present is requested.
- The API must not require `cx.notify()` for high-frequency pixels.
- The API docs should explicitly say: `frame()` should only request present when
  visible pixels changed; no-op ticks must return `Skip`.
- The sample should demonstrate:
  - static retained base;
  - high-frequency overlay;
  - no view dirty on mousemove;
  - counters/log proving only GPU canvas presents occur.

## Validation checklist

Before saying this is fixed:

- [ ] Pure mousemove does not call `CHART_INPUT_NOTIFY`.
- [ ] Pure mousemove does not raise `orders_render` / `shell_render` to monitor
      or mouse frequency.
- [x] DX11 code path has a base cache (`BaseCache`) and separate
      `render_base_d3d` / `render_cursor_d3d`.
- [x] DX11 cursor-only code path skips `chart_gpu_prepare` unless
      `gpu_prepare_dirty` is set.
- [ ] Runtime: pure mousemove does not raise `chart_gpu_prepare` as full
      per-pane prepare work after base cache exists.
- [ ] Runtime: pure mousemove does not raise `combo_draw`, `orderbook_draw`,
      `userdata_draw` after base cache exists.
- [ ] Runtime: pure mousemove raises only `base_blit` + `cursor_draw`
      / readout overlay.
- [x] Code-level: old single-mousemove -> `present_seq` -> 16ms task ->
      next present loop is removed.
- [ ] Runtime: a single mousemove does not start a continuous-present loop.
- [ ] Runtime: four visible panes do not multiply cursor-only work by full layer stack.
- [ ] Readout chips move without GPUI notify, or the doc explicitly marks them
      as throttled/temporary.
- [x] Windows check/build/tests pass with `--target x86_64-pc-windows-msvc`.
- [x] Linux wgpu check/test/build passes on VPS.
- [ ] Mac Metal compile/run check covers current HEAD.
- [ ] Metal/wgpu either get base-cache parity or are explicitly measured and
      accepted per layer.

## Current verification done after fixes

Current HEAD verified in this audit: `185d4c8`.

Windows:

- `cargo check -p moon-ui-gpui --bin moon-gpui --target x86_64-pc-windows-msvc`
- `cargo build -p moon-ui-gpui --bin moon-gpui --target x86_64-pc-windows-msvc`
- `cargo test -p moon-ui-gpui --test theme_contract --target x86_64-pc-windows-msvc`
- public-resolution check without local `.cargo/config.toml` / local lockfile.

Linux VPS:

- `cargo check -p moon-ui-gpui --bin moon-gpui`
- `cargo test -p moon-ui-gpui --test theme_contract`
- `cargo build -p moon-ui-gpui --bin moon-gpui`

Still missing:

- runtime `MOON_RENDER_DIAG=1` mousemove session;
- Mac Metal compile/run result for current HEAD;
- Linux GUI runtime check, not only compile/link.
