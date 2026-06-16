# FORK_revX2 — уточнения после проверки `FORK_rev.md`

Status: follow-up review notes, 2026-06-16. Не самостоятельный план. Читать после `FORK_rev.md`.

Критичных возражений к `FORK_rev.md` по сути нет: проверка по коду подтвердила основные факты:

- `Scene::replay()` действительно переигрывает только `paint_operations`.
- `ContentMask` сейчас прямоугольный (`bounds`).
- Throttle в `gpui/src/window.rs` стоит до вычисления `needs_present`.
- Wayland request-frame завязан на `surface.frame()` / `commit`, поэтому skip-present требует отдельного
  timer fallback.

## Уточнения, которые надо учесть перед кодингом

### 1. `PaintOperation::GpuCanvas` должен replay-иться через insert path

Нельзя просто хранить `PaintGpuCanvas` в новых `Scene` Vec и нельзя при replay тупо копировать старый
`order` из предыдущей сцены.

Правильная модель такая же, как у `Primitive`:

```text
Scene::replay()
  PaintOperation::GpuCanvas(op) -> self.insert_gpu_canvas(op.clone())

insert_gpu_canvas()
  clips bounds/content_mask
  computes order from current layer_stack / primitive_bounds
  pushes into under/over gpu_canvas vec
  pushes PaintOperation::GpuCanvas(...)
```

Иначе partial scene reuse потеряет canvas или получит stale draw order после replay.

Следствие: payload в `PaintOperation::GpuCanvas` должен быть cloneable. Driver туда кладётся только через
cloneable/stable handle (`Rc`/`Arc`/id-handle), не как raw non-clone driver object.

### 2. `prepare_gpu()` лучше вызывать для всех visible canvases на любой present

В `FORK_rev.md` есть формулировка “prepare_gpu for visible canvases needing preparation”. Это может быть
слишком хитро для V0.

Более безопасный контракт:

```text
if needs_present:
    prepare_gpu() for every visible gpu_canvas
    clear
    under draw()
    GPUI batches
    over draw()
    Present
```

Driver сам делает no-op по своим dirty-флагам. Так мы не ловим случай, где present вызван dirty UI или другим
canvas, а данный canvas всё равно должен нарисоваться, но не получил шанс применить uploads/resource sync.

### 3. Каноническая последовательность кадра должна включать prepass

В validation нельзя оставлять старое:

```text
frame -> clear -> draw -> Present
```

После введения `prepare_gpu()` канон:

```text
frame -> prepare_gpu -> clear -> draw -> Present
```

Это особенно важно для Metal/wgpu, где offscreen bake/upload должен происходить до активного under/over
phase pass.

### 4. `FORK_rev.md` надо слить в canonical docs до реализации

`FORK_rev.md` сейчас прямо говорит, что он не самостоятельный план. Но в нём находятся критичные правки к
`ZED_FORK_V0.md`:

- `PaintOperation::GpuCanvas`;
- `prepare_gpu`;
- throttle-before-mutating-frame;
- opaque backend access;
- Wayland timer fallback;
- validation additions.

Перед началом clean fork implementation надо обновить `ZED_FORK_V0.md` и `Terminal_patch_V0.md`, чтобы они
стали единственным canonical планом. Иначе следующий агент легко начнёт кодить по старому V0 и пропустит
самые важные исправления.
