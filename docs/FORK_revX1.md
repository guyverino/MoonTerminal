# FORK_revX1 — extra review notes for `FORK_rev.md`

Status: extra review notes, 2026-06-16.

Scope: this file only adds critical clarifications found after reading `FORK_rev.md`.
It does not replace `FORK_rev.md`.

## 1. Frame-loop pseudocode: throttle placement is ambiguous

`FORK_rev.md` currently shows:

```text
if dirty || force:
    window.draw(cx)              // rebuild scene + fresh canvas lists

if gpu-clock tick is throttled:
    window.complete_frame()
    return                       // no mutating frame()
```

Read literally, this can rebuild/mutate the scene and then skip present because of the GPU-clock
throttle. That is not the desired contract.

Required clarification:

- GPU-clock throttle must run before mutating `gpu_canvas.frame()` state.
- GPU-clock throttle must not suppress a real dirty/force UI redraw that already needs presentation.
- Do not call `window.draw(cx)` and then return only because the GPU-only tick was throttled.

Safer shape:

```text
let ui_dirty = invalidator.is_dirty() || force_render;
let gpu_tick_allowed = gpu_clock_not_throttled();

if !ui_dirty && !gpu_tick_allowed {
    complete_frame();
    return;
}

arena_clear = None;
if ui_dirty {
    arena_clear = window.draw(cx);       // fresh scene + fresh gpu_canvas lists
}

gpu_wants = false;
if gpu_tick_allowed || ui_dirty {
    gpu_wants = poll_all_gpu_canvas_frame(); // no short-circuit; barrier before clear
}

needs_present =
    ui_dirty ||
    request_frame_options.require_presentation ||
    window.needs_present ||
    high_rate_input ||
    gpu_wants;

if needs_present {
    window.present();
} else {
    complete_frame();
}

clear arena if needed;
```

The exact variable names do not matter. The invariant does:

```text
No state mutation on a tick that is skipped solely by GPU-clock throttle.
Dirty/force UI redraw is not dropped by GPU-clock throttle.
```

## 2. `prepare_gpu()` should be called for every visible canvas when presenting

`FORK_rev.md` says:

```text
if needs_present:
    prepare_gpu() for visible canvases needing preparation
```

The phrase "needing preparation" is risky.

The renderer often cannot know this from CPU flags alone. A canvas may need GPU preparation because:

- `device_generation` changed;
- render target format changed;
- swapchain/surface/drawable was recreated;
- Metal/wgpu backend object identity changed;
- canvas resources are missing after recovery.

Those facts are known only after a valid GPU frame context exists.

Required clarification:

```text
if needs_present:
    acquire current GPU frame/drawable/surface texture
    call prepare_gpu() for every visible gpu_canvas
    clear target
    draw under canvases
    draw GPUI scene
    draw over canvases
    present
```

`prepare_gpu()` itself must be cheap when nothing is dirty:

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

Do not make the renderer guess which canvases need preparation. Let each driver no-op cheaply.

## 3. GPU frame acquire must be explicit before `prepare_gpu()`

`prepare_gpu()` needs GPU access, so the renderer must already have a valid backend frame context, but
must not yet have opened the active scene render pass/encoder.

This matters on Metal and wgpu:

- Metal needs a drawable/command buffer, but only one active render encoder at a time.
- wgpu needs the current surface texture/texture view/command encoder, but cannot open an offscreen
  render pass while the phase render pass is active.

Required sequence:

```text
needs_present decided
    -> acquire drawable/surface frame
    -> create command buffer / command encoder as needed
    -> prepare_gpu() for visible canvases, no active GPUI scene pass
    -> open under phase pass/encoder
    -> draw under canvases
    -> draw normal GPUI scene
    -> open over phase pass/encoder
    -> draw over canvases
    -> submit/present
```

If acquire fails, surface is lost, or the frame is not presentable:

```text
do not call prepare_gpu()
do not call draw()
update presentable/device-lost state
complete_frame or recover through existing renderer path
```

This should be part of the renderer contract, not left implicit.

## 4. Final invariants to merge back into the main docs

- GPU-only throttle happens before mutating `gpu_canvas.frame()` state.
- UI dirty/force frames are not suppressed by GPU-only throttle.
- On every actual present, every visible canvas gets `prepare_gpu()` and then `draw()`.
- `prepare_gpu()` is allowed to no-op cheaply; the renderer does not guess dirtiness.
- GPU frame acquire happens before `prepare_gpu()` and before any active phase pass.
- Failed acquire / lost surface / not-presentable frame does not call app GPU callbacks.
