# OBSOLETE: Draft PR description for zed-industries/zed

This PR description targets the old raw `add_gpu_pass` /
`request_continuous_presentation` direction. Do not use it for the next upstream
attempt. The current draft path is `docs/GPUI_GPU_CANVAS_FORK_PLAN.md`:
`gpu_canvas`, an immediate-mode GPU drawing element with pre-present frame
decision.

Branch: `Moonbot-Tech/ZedFork:codex/gpui-gpu-pass-hook-pr`

## Summary

Adds a small custom GPU pass integration point to GPUI windows.

Applications can register a frame-scoped backend-native callback for a documented render phase:

- `GpuPhase::UnderScene` — before GPUI scene primitives, useful for custom viewports under GPUI UI/text.
- `GpuPhase::OverScene` — after GPUI scene primitives.

The API is intentionally generic. It does not contain chart, terminal, or application-specific rendering code.

## Motivation

Some GPUI applications need high-throughput GPU-rendered viewports inside normal GPUI composition:

- dense time-series/charts;
- video or media previews;
- GPU/game/canvas previews;
- visualization/profiler panes.

Without a frame-level hook, applications have poor alternatives:

- emit tens or hundreds of thousands of GPUI primitives every frame;
- render offscreen and copy/read back into a GPUI image;
- create native child surfaces and fight compositor/input/z-order/overlay behavior.

A custom GPU pass lets the app keep its specialized renderer while GPUI still owns the window, text,
overlay UI, clipping, popups, and frame lifecycle.

## API / lifecycle

- `Window::add_gpu_pass(phase, callback)` registers a callback and returns a `Subscription`.
- Dropping the subscription unregisters the pass.
- The callback returns `anyhow::Result<()>`; errors propagate through the renderer draw path.
- Unsupported platforms return an explicit error instead of silently ignoring the pass.
- `RawGpuAccess::device_generation` lets custom renderers recreate backend resources after device
  recovery/recreation.
- All raw handles are borrowed for the callback duration only. Consumers must not store or release them,
  present the swap chain, or block the frame loop.

## Platform support

Implemented in this PR:

- Windows / D3D11:
  `ID3D11Device*`, `ID3D11DeviceContext*`, `ID3D11RenderTargetView*`, `DXGI_FORMAT`.
  The pass is wrapped in a D3D11 state guard for GPUI-owned OM/RS/IA/VS/PS state.
- macOS / Metal:
  `MTLDevice*`, `MTLCommandBuffer*`, isolated `MTLRenderCommandEncoder*`, `MTLTexture*`,
  `MTLPixelFormat`.
- Linux / wgpu:
  `wgpu::Device*`, `wgpu::Queue*`, `wgpu::CommandEncoder*`, `wgpu::TextureView*`,
  `wgpu::TextureFormat*`.

## Example

Adds `crates/gpui/examples/gpu_pass.rs`, a minimal backend-agnostic example that:

- registers an `UnderScene` pass;
- keeps the returned `Subscription`;
- counts actual callback frames;
- displays backend, device generation, and target size in GPUI UI.

Checked with:

```text
cargo check -p gpui --example gpu_pass
```

## Performance context

The motivation came from a dense chart renderer, but the API is generic.

Local benchmark evidence from the GPUI chart stand:

- Re-emitting 100k chart markers as GPUI primitives was around 15-19 fps.
- A direct backend-native instanced pass rendered 100k+ markers at hundreds of fps uncapped.
- The combo/static texture cache path measured roughly 100k visible markers -> 369 fps and
  318k visible markers -> 223 fps in the stand, with no CPU readback path.

The important point for GPUI is not these exact chart numbers; it is that zero-copy custom GPU viewports
avoid both UI primitive overload and offscreen/readback latency while preserving GPUI composition.

## Extension: continuous presentation for retained GPU passes

The base GPU-pass hook is enough for data-driven redraws, but it exposes a second generic need:
some custom GPU passes are retained/animated independently of GPUI view state.

Examples:

- video or media surfaces whose texture advances every display tick;
- profiler/game/canvas panes with their own GPU state;
- dense chart viewports that scroll resident GPU data while GPUI labels and controls remain unchanged;
- external texture producers that need presentation cadence without rebuilding the UI tree.

Using `Window::request_animation_frame()` for this is too broad. It invalidates a GPUI view and schedules
a full view redraw. That is correct for GPUI element animations, but wasteful for GPU-only content where
the cached GPUI scene is still valid and only the backend-native pass needs another presentation frame
(in a real terminal this forced a full top-down re-render of unrelated heavy views every vblank).

The desired semantics are:

```text
no GPUI view invalidation
no root/view recomputation
reuse the cached GPUI scene
run registered GPU passes for the frame
present the window
```

### Implemented API

```rust
// Hold while the pass animates; drop when idle. Stacks; continuous until last drop.
let _present = window.request_continuous_presentation();
```

`Window::request_continuous_presentation() -> Subscription`. Chosen over a per-pass
`GpuPassPresentation` flag because the "keep presenting" decision is a *window* policy, orthogonal to
*which* passes are registered: an app can pause/resume a retained surface without re-registering it, and
a static custom pass never costs continuous frames. Lifetime-based and opt-in by construction.

### Why it is cross-platform with no per-platform code

The request is a counter on the platform-agnostic `gpui::Window`. The single `on_request_frame` closure
(shared by every platform) ORs `continuous_presentation > 0` into `needs_present`; on a non-dirty frame
the window then `present()`s the cached scene (which re-runs the registered GPU passes) instead of
recomputing views. Every platform already drives that closure once per display tick, so all honor the
flag without platform-specific plumbing:

- Windows: the vsync pacer thread posts a frame message every vblank;
- macOS: `CVDisplayLink` calls back every vblank while the window is active;
- Linux/X11: a calloop timer ticks at the monitor refresh rate while the window is mapped;
- Linux/Wayland: the `wl_callback` frame loop is self-sustaining (each continuous frame commits, which
  re-arms the next frame callback).

Contract (lifetime-based, opt-in):

- no behavior change when nothing requests continuous presentation (registering a pass alone does not);
- dropping the subscription returns the window to on-demand presentation (no display wakeups when idle);
- inactive-window, thermal, and platform throttling still apply;
- the callback still receives frame-scoped raw handles only;
- the pass must not present, block, or mutate GPUI-owned state outside the documented callback.

Upstream-friendly validation compares the two generic modes:

```text
request_animation_frame path:
  custom GPU content advances, but GPUI views are marked dirty and recomputed.

continuous-presentation path:
  custom GPU content advances at presentation cadence while unrelated GPUI views stay cached.
```

The argument avoids application-internal metrics: retained GPU surfaces need a way to present without
abusing view invalidation, preserving GPUI's scene cache and reducing CPU churn for any app embedding
high-frequency GPU content. (This is independent of, and must not be bundled with, the Windows
frame-pacing changes — vsync-message redraw, waitable swapchain — which are a separate fork concern.)

## Non-goals

- No chart-specific renderer in GPUI.
- For the base GPU-pass hook PR: no frame pacing or vsync API changes. The presentation-only frame
  mode above is a separate follow-up/extension if needed.
- No new widget/layout abstraction.
- No behavior change when no custom GPU pass is registered.
- No promise that raw handles are safe to store past the callback.

## Validation

Run on the PR worktree:

```text
cargo check -p gpui -p gpui_windows -p gpui_wgpu -p gpui_linux -p gpui_macos
cargo check -p gpui --example gpu_pass
```
