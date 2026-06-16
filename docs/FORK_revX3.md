# FORK_revX3 — extra critical notes after FORK_rev

Status: extra review notes, 2026-06-16.

`FORK_rev.md` в целом корректен и полнее предыдущих ревизий. Ниже только дополнительные замечания, которые
нужно перенести в основной план перед кодингом.

## 1. `prepare_gpu()` должен обслуживать present, а не только автора present

Важно: на любом present backbuffer очищается, значит **каждый visible gpu_canvas обязан нарисовать текущие
пиксели**, даже если этот canvas сам вернул `Skip`.

Следствие для `prepare_gpu()`:

```text
если кадр будет presented по любой причине:
    вызвать prepare_gpu() для всех visible canvases, у которых есть pending GPU work
    затем clear
    затем draw() всех visible canvases в under/over фазах
```

Не только для canvas, который вернул `RequestPresent`.

Пример бага, если сделать иначе:

```text
canvas A вернул RequestPresent
canvas B вернул Skip, но у B есть pending resize/device_generation/data upload
renderer clear'ит backbuffer
draw(B) вызывается без prepare_gpu(B)
=> B рисует stale/blank/невалидные ресурсы
```

Правильный контракт:

```rust
frame()       // решает, нужен ли present из-за этого canvas, и выставляет pending flags
prepare_gpu() // применяет pending GPU work, если этот canvas будет рисоваться на present
draw()        // композитит current pixels
```

`prepare_gpu()` должен быть дешёвым no-op, если pending work нет.

## 2. `frame()` не должен инвалидировать GPUI tree

Добавить в контракт явно:

```text
GpuCanvasDriver::frame() may mutate only driver-owned retained state.
It must not call cx.notify(), must not mutate GPUI entity tree, and must not trigger layout.
```

Иначе вся фича теряет смысл: мы снова можем получить top-down invalidation через заднюю дверь.

Если driver понял, что нужен GPUI UI update (оси, toolbar, labels), он должен:

- выставить свой редкий UI dirty flag;
- отдать его обычному приложенческому throttled notify path;
- не делать per-vblank/per-mousemove notify из `frame()`.

Для терминала:

```text
pixel-cross / mouse-move / cursor / readouts / orderbook GPU pixels -> gpu_canvas path
axis text / toolbar / panel UI -> редкий cx.notify() path
```

## 3. `over()` vs popup/menu/tooltip проверять рано

`gpu_canvas(...).over()` в V0 означает "over whole GPUI scene". Это может оказаться выше popup/menu/tooltip,
если эти UI-слои рисуются в той же scene до over-phase.

Риск:

```text
chart over-canvas рисует crosshair/readout
context menu / tooltip открыт над chart rect
over-canvas перекрывает меню или tooltip
```

Нужно проверить в начале реализации, не в финальной валидации:

- GPUI popup/menu/tooltip рисуется отдельным OS window или поверх `over_gpu_canvases`?
- если нет, нужен один из вариантов:
  - terminal hides/gates over-canvas while popup/menu over chart is open;
  - renderer phase для over-canvas стоит ниже popup layer;
  - V0 документирует, что `over()` is over normal scene but below platform popups.

Без этой проверки можно получить красивый chart overlay, который ломает базовый UI.

## 4. Minor wording to merge

В основной план добавить короткий invariant:

```text
Present reason and draw obligation are separate:
- `frame() -> RequestPresent` only says this canvas wants a present.
- Once a present happens for any reason, every visible canvas is prepared if needed and drawn.
```

Это убирает двусмысленность "Skip значит не рисовать".
