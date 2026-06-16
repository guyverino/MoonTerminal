# ZED_FORK_V0 — изменения в коде zed/gpui

Status: impl spec v0, 2026-06-16. Всё, что пишется в код zed.
Две части: **§A — фича `gpu_canvas`** (идёт в upstream-PR); **§C — отдельные upstream-фиксы** (каждый —
свой маленький PR в Zed, НЕ бандлить с gpu_canvas; пока не приняты — живут в delivery-форке).
Терминальная половина — отдельный док `Terminal_patch_V0.md`.

Факты проверены по дереву: база zed = `f39cf25c`, форк-worktree. Код в gpui — только латиница, без
доменной/терминальной лексики.

## Порядок upstream-PR

1. **PR0 / C1: Windows DPI restore round-trip** — первый PR. Это чистый баг из `docs/CheckNewFork.md`:
   окно восстанавливается на правильный monitor/display_id, но logical bounds умножаются на scale не того
   HWND. Маленький, самодостаточный, без связи с chart/gpu_canvas; должен быть максимально принимаемым.
2. **PR1 / C2: Windows frame pacing bugfix** — отдельный PR до `gpu_canvas`. Он чинит multi-window freeze
   и заменяет слабый `RedrawWindow -> WM_PAINT`-тик на posted vsync-message + waitable swapchain pacing.
   `gpu_canvas` не должен продавать старый `WM_PAINT`-тик как финальную основу.
3. **PR2 / §A: `gpu_canvas`** — фича поверх базы, где Windows pacing уже приведён в порядок. Если PR1 не
   принят заранее, `gpu_canvas`-PR обязан явно содержать минимальный эквивалент frame-clock/pacing или ждать.
4. **PR3 / C3: floating tool-window style** — опционально и отдельно; это поведение, не обязательный багфикс.

## Цель (PR)

> Дать обычному UI-элементу GPUI рисовать backend-native GPU-контент, решать ДО clear/present, нужен ли
> кадр, и рисовать в том же кадре, когда нужен. UI/layout/text/composition остаются за GPUI.

**Инвариант кадра:** опрос `frame()`, `clear`, `draw()`, `Present` — синхронно в ОДНОМ платформенном тике;
`RequestPresent` не переносится на следующий. Immediate-mode (после `Present` бэкбуфер undefined): на любом
present каждый видимый канвас рисует текущие пиксели.

---

# §A. Фича gpu_canvas (upstream-PR)

## A1. Элемент + хранение (две Vec под/над, без примитив-интеграции)

Публичный API как `canvas()`: `Styled`, layout из стиля, наследует clip/content_mask. Слой — бинарный:

```rust
gpu_canvas(driver)         // под сценой (default)
gpu_canvas(driver).over()  // над сценой (оверлей)
```

Под/над, НЕ draw-order interleaving: в draw-order канвас рвал бы энкодер/пасс ×N (Metal/wgpu) и тянул
save/restore D3D-стейта в середине батч-лупа. z-между-UI-примитивами не поддерживаем (нет кейсов).

Хранение — два списка в scene, **НЕ примитив батч-итератора**:
```rust
scene.under_gpu_canvases: Vec<PaintGpuCanvas>
scene.over_gpu_canvases:  Vec<PaintGpuCanvas>
struct PaintGpuCanvas { order, bounds, content_mask, driver: GpuCanvasHandle } // non-Copy payload
```
`paint()` пушит в нужный список через `window.paint_gpu_canvas(layer, …)` (аналог `paint_surface`: клип
bounds к content_mask, draw-order). Сортировка по `order` внутри фазы; списки персистят между не-dirty
кадрами (gate читает их из cached scene). **Не трогать** `Primitive`/`PrimitiveKind`/`PrimitiveBatch`/
`batches()` — gpu_canvas не draw-order примитив, а фазовый хук с bounds.

## A2. Публичный трейт `GpuCanvasDriver` (реализует приложение)

```rust
pub trait GpuCanvasDriver {
    /// CPU-only, before clear, exactly once per tick per visible canvas, NO GPU access.
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision;   // Skip | RequestPresent
    /// GPU, after clear, in under/over phase, under the GPUI scissor.
    fn draw(&mut self, access: &RawGpuAccess) -> anyhow::Result<()>;
}
```

Семантика контракта (документировать как API):
- `frame()` — единственная точка decide, без GPU-вызовов → платформо-независима, зовётся из gate.
- `draw()` — issue draw-calls из готового состояния; вызывается для ВСЕХ видимых канвасов на любом present
  (immediate-mode). Канвас со `Skip` всё равно рисует текущие пиксели.
- **Resize:** дельта `bounds`/`scale_factor` ⇒ драйвер обязан перерисовать (его дело; API лишь даёт поля).
- **device-lost:** `draw()` обязан сверять `access.device_generation` и пересоздавать ресурсы при
  расхождении (документировать как обязанность драйвера).
- Опрос всех канвасов — **без short-circuit**; **барьер**: все `frame()` ДО единственного clear.

## A3. Gate + heartbeat: verified base clock, but Windows transport fixed by PR1/C2

Факт по чистой базе `f39cf25c`: frame-clock в GPUI действительно уже есть, это не надо придумывать с нуля.
- **Windows upstream:** поток `VSyncProvider` (`gpui_windows/src/platform.rs`) делает `wait_for_vsync()`
  (`DwmFlush`, `vsync.rs`) → `RedrawWindow(hwnd, RDW_INVALIDATE)` по всем окнам → `WM_PAINT` →
  `draw_window` (`events.rs`) → `request_frame` → closure `on_request_frame` (`gpui/src/window.rs`).
  Closure уже умеет ничего не делать без dirty/present: `draw+present` только при dirty/force,
  `present` только при `needs_present`, иначе `complete_frame`.
- **Windows delivery-fork:** этот транспорт уже заменён на `PostMessage(WM_GPUI_VSYNC_TICK)` → `draw_window`
  в C2. Это не новая модель кадра, а более надёжная доставка того же tick.
- **macOS:** `CVDisplayLink` вызывает request-frame callback у видимого окна; при occlusion display-link
  останавливается.
- **Linux/X11:** есть per-window calloop timer по refresh rate. **Wayland:** frame-callback завязан на
  surface commit, поэтому ему нужен отдельный fallback while scene has gpu_canvas.

Вывод: на общий frame-gate опираться можно, но Windows `RedrawWindow -> WM_PAINT` нельзя считать финальной
основой для `gpu_canvas`. Сначала PR1/C2 чинит доставку тика и multi-window freeze; уже после этого
`gpu_canvas` добавляет только новое решение `frame() -> needs_present`, а не новый Windows-пейсер.

**Единственное новое для часов после PR1/C2 (~15 строк в общем `gpui/src/window.rs`, у `:1508`):** подмешать голос
канвасов в `needs_present`:
```rust
let gpu_wants = window.scene_gpu_canvases_request_present(info); // frame() всех видимых, барьер, без short-circuit
let needs_present = request_frame_options.require_presentation
    || needs_present.get()
    || (active.get() && input_rate_tracker.is_high_rate())
    || gpu_wants;
```
present-путь исполнит `draw()` канвасов в под/над-фазах. После PR1/C2 на Windows и в текущей модели macOS/X11
не нужен новый поток/таймер; `gpu_canvas` подключается к уже существующему request-frame closure.

**НЕ вводить:** `WhileVisible`/`OnDemand`/`max_fps`/`request_continuous_presentation`. Старый
`continuous_presentation` устарел (форсил present для raw-pass; здесь present решает `frame()`).

**Источник НЕ прорежать** до 60 — сломает 120/144/240Гц UI-анимации и input-sustain всему zed. Пейсинг
чарта — его `Skip` (терминал). Throttle базы (active=None / inactive=33мс / thermal=16.6мс) не трогать.

**Present-независимость:** Windows после PR1/C2 и macOS CVDisplayLink тикают независимо от present →
skip present не глушит следующий тик. **Wayland — исключение:** frame-callback завязан на commit → держать
calloop-таймер, пока в scene окна есть gpu_canvas (validation item, проверить реально). Это единственная
новая per-platform работа по часам.

**Единый gate (одна точка в present-пути):**
```
[если UI dirty/force] window.draw(cx)             // rebuild scene + списки канвасов (свежие bounds)
опросить frame() всех видимых канвасов (барьер, без short-circuit)
needs_present = dirty || force || any(RequestPresent)
если needs_present: window.present()              // clear → under draw() → batches → over draw() → Present
иначе: complete_frame()                            // НИ clear, НИ present
```

## A4. `GpuFrameInfo` + `presentable`

```rust
pub struct GpuFrameInfo {
    pub now: Instant,          // frame-timing only
    pub bounds: Bounds<Pixels>,
    pub scale_factor: f32,
    pub presentable: bool,
}
```
`reason`/`content_mask` не нужны. `presentable`:
- **Windows:** не-minimized И последний `Present` ≠ `DXGI_STATUS_OCCLUDED` (SUCCESS-HRESULT — не глотать
  `.ok()`; при OCCLUDED слать `Present(0, DXGI_PRESENT_TEST)` до `S_OK`). width=0 одного мало (DWM thumbnail).
- **macOS:** CVDisplayLink при окклюзии перестаёт тикать → `frame()` не зовётся.
- **Linux/wgpu:** surface lost → false.

## A5. `RawGpuAccess` = типизированный enum + device_generation

```rust
pub enum RawGpuAccess<'a> {
    D3d11(D3d11Access<'a>),   // device, context, rtv, format, device_generation, size
    Metal(MetalAccess<'a>),   // device, command_buffer, encoder, texture, pixel_format, device_generation
    Wgpu(WgpuAccess<'a>),     // device, queue, encoder, view, format, device_generation
}
```
Не `*mut c_void`. `device_generation` обязателен. Хэндлы borrowed на время callback; хранить/Release/
present/scene-mutate нельзя — документировать.

## A6. Renderer draw (DX11 / Metal / wgpu)

Две фиксированные точки СНАРУЖИ батч-лупа: under-список ДО scene-батчей, over-список ПОСЛЕ. На канвас:
scissor = `bounds ∩ content_mask` → собрать `RawGpuAccess` (+device_generation) → `driver.draw()` → restore.
- DX11: `D3d11StateGuard` (донор §C) + scissor-rasterizer.
- Metal: **ОДИН изолированный энкодер на ФАЗУ** (все under-канвасы рисуются в нём, scissor между ними), НЕ
  на канвас. Как донорский `run_gpu_passes` (один вызов на фазу, цикл проходов внутри,
  `metal_renderer.rs:417/948/1058`) — НЕ как Path (`:977/987` рвёт энкодер на каждую батчу = N). Per-canvas
  энкодер при 4 чартах = N разрывов = цена in-scene, которую и обходим. Драйвер рисует В предоставленный
  фазовый энкодер (свой на нём не открывает); offscreen-bake — через отдельный command buffer от `device`
  (Metal: один активный энкодер за раз).
- wgpu: **один render-pass на ФАЗУ** с `Load` (scissor между канвасами), не на канвас.
Device-recreation инкрементит `device_generation` (Windows handle_device_lost, wgpu recover).

## A7. Пример, форма PR, что НЕ делать

- **Пример** `crates/gpui/examples/gpu_canvas.rs` — нейтральный анимированный вьюпорт (треугольник/партиклы),
  НЕ курсор. Доказать: пропущенные тики не дёргают/ре-рендерят GPUI views.
- **Один PR:** `Add gpu_canvas: immediate-mode GPU drawing element with pre-present frame decision`.
  Use cases: плотные графики, видео/превью, game/canvas-превью, GPU-профайлеры, map/CAD, кастомный курсор.
- **НЕ делать:** retained-texture compositor; доменная/нелатинская лексика в gpui source; draw-order примитив
  для gpu_canvas; публичные `request_continuous_presentation`/`add_gpu_pass`/`set_present_sync_interval`;
  бандлить §C в этот PR; `WhileVisible/OnDemand/max_fps`; прорежание источника; short-circuit `frame()`.

## Порядок реализации (форк)

Чистый клон upstream → PR0/C1 DPI restore bugfix → PR1/C2 Windows frame pacing bugfix → A1 элемент+две Vec
→ A6 хранение/finish → A3 опрос `frame()` в существующем closure → A6 DX11 draw → A6 Metal draw →
A6 wgpu draw → Wayland таймер-фолбэк → A7 пример → C3 по решению → (порт терминала) → диагностика →
PR-текст.

## Валидация §A

- [ ] Пропущенный тик: ноль clear / renderer-draw / Present.
- [ ] `RequestPresent` рисует в ТОТ ЖЕ тик; gate — одна точка; опрос без short-circuit; барьер до clear.
- [ ] Dirty-кадр рисует все видимые канвасы; `frame()` вызван и на dirty.
- [ ] heartbeat = существующий VSync/DisplayLink; на Windows/macOS нового потока нет; Wayland таймер-фолбэк реальный.
- [ ] Источник НЕ замедлен (120/144/240Гц UI-анимации не регрессят).
- [ ] `presentable==false` под окклюзией; Windows `DXGI_STATUS_OCCLUDED` пойман, не проглочен.
- [ ] DX11 стейт восстановлен после `draw()`; Metal границы энкодера валидны; wgpu `draw()` не рвёт GPUI-pass.
- [ ] Несколько канвасов/окон; detach → канвас только в своём окне; hidden tab → нет записей в списках.
- [ ] В gpui source нет доменной/нелатинской лексики; пример без терминальной специфики.

---

# §C. Отдельные upstream-фиксы (чинить и слать в Zed; НЕ в gpu_canvas-PR)

Стартуем с чистого клона upstream. Старый форк (`gpui-fork-pr`, 7 коммитов над базой) НЕ переносим
целиком — раскладываем на три корзины:
- **PORT** — generic-фиксы, каждый своим маленьким PR в Zed (не бандлить с gpu_canvas). Пока не приняты —
  живут в delivery-форке. **Цель форка — ноль:** это не вечные порты, а очередь в upstream.
- **DONOR** — backend-механика: НЕ cherry-pick, выдрать куски кода в A6 (таблица ниже).
- **DROP** — выкинуть совсем (заменено фичей).

## PORT — три отдельных PR в Zed

Каждый самодостаточен: ЧТО (баг/поведение) → ЗАЧЕМ (наш кейс) → ГДЕ (файлы/символы) → СУДЬБА.

### C1. PR0: DPI round-trip при restore окна на мониторе с другим scale — баг
- **ЧТО:** detached-окно восстанавливается на правильном мониторе (`display_id`), но `origin` сдвигается
  на scale-множитель монитора (пример scale=1.25: x 95→119, y −733→−916). `retrieve_window_placement`
  переводит saved logical bounds в device pixels через scale свежесозданного HWND, а тот ещё держит DPI
  primary-монитора, не целевого `display_id`.
- **ЗАЧЕМ:** откреплённые чарт-окна на multi-monitor с разным DPI не прыгают при restore.
- **ГДЕ:** `gpui_windows/src/display.rs` — добавить `WindowsDisplay::scale_factor()` getter;
  `gpui_windows/src/window.rs` — перед `SetWindowPlacement` брать scale у выбранного `display`, прокинуть
  в `state.scale_factor` + `direct_manipulation` + параметром в `retrieve_window_placement`.
- **СУДЬБА:** **первый upstream-PR**. Чистый generic баг gpui (ловит любой multi-monitor app), маленький
  и самодостаточный. Полная причина/репро/готовый патч — `docs/CheckNewFork.md` (фикс найден, но НЕ
  закоммичен → в `git log` его нет). Этот PR не зависит от chart/gpu_canvas и должен быть максимально
  принимаемым.

### C2. PR1: мультиоконный UI-фриз: waitable swapchain + vsync-message pacing — perf-баг
- **ЧТО:** `Present` блокировал UI-поток под нагрузкой → фриз при нескольких окнах. Фикс: ждать
  готовности свопчейна ВНЕ `Present` (frame-latency waitable, max-latency=1); будить окна
  posted-сообщением (`WM_GPUI_VSYNC_TICK`) вместо `RedrawWindow→WM_PAINT` (иначе draw голодает под
  `WM_MOUSEMOVE` соседнего окна на одном UI-потоке).
- **ЗАЧЕМ:** мультиоконный/мультичартовый терминал не фризит. Бонус: waitable-on-present — основа
  present-независимого heartbeat для gpu_canvas (A3).
- **ГДЕ:** `gpui_windows/src/{directx_renderer.rs, events.rs, platform.rs}`. Символы:
  `frame_latency_waitable`, `GetFrameLatencyWaitableObject`,
  `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT`, `WM_GPUI_VSYNC_TICK`.
- **СУДЬБА:** **второй upstream-PR, до `gpu_canvas`**. Generic Windows perf/latency bugfix, не часть
  публичного `gpu_canvas` API. Без него `gpu_canvas` будет вынужден опираться на старый
  `RedrawWindow -> WM_PAINT` тик, который уже показал starvation/freeze под multi-window/input нагрузкой.
  Если PR1 не принят заранее, `gpu_canvas`-PR должен ждать или включать минимально тот же pacing-фикс, но
  не маскировать его как часть canvas API.

### C3. Tool-window стиль для floating-окон — поведение
- **ЧТО:** `WindowKind::Floating` → `WS_EX_TOOLWINDOW` (нет иконки в taskbar, тонкая рамка).
- **ЗАЧЕМ:** откреплённые чарт-окна не плодят иконки в taskbar.
- **ГДЕ:** `gpui_windows/src/window.rs` (2 строки).
- **СУДЬБА:** поведенческий выбор, не баг — upstream может не захотеть менять floating. Либо опц-PR, либо
  единственная реально-приватная правка delivery-форка.

**Полнота PORT** (самопроверка, НЕ источник описания): сверить C1–C3 с двумя реестрами, чтобы локальный
фикс не потерялся при чистом клоне — `git log <base>..<fork>` (закоммичено: C2, C3) и `docs/CheckNewFork.md`
(uncommitted: C1 и будущие). Что делать — описано выше; дифф нужен лишь чтобы убедиться, что список полон.

## DONOR — выдрать backend-механику в A6 (не писать заново)

Коммиты `Add custom GPU pass hook` + `Add Metal and wgpu GPU pass hooks` реализуют СТАРУЮ модель
(window-global `add_gpu_pass`), которую gpu_canvas заменяет. **Публичный API оттуда не берём.** Но внутри —
тяжёлая backend-сантехника, нужная A6 один-в-один. Код есть в ТЕКУЩЕМ worktree (`gpui-fork-pr`) — открыть
файлы рядом, перенести символы, переподключив их к `scene.{under,over}_gpu_canvases` вместо
`add_gpu_pass`/`under_scene_passes`-Vec:

| Выдрать | Откуда (текущий worktree) | Куда в новой фиче |
|---|---|---|
| `D3d11StateGuard` (capture/restore D3D11-стейта вокруг callback) | `gpui_windows/src/directx_renderer.rs:74,108,260` | A6 DX11: обернуть каждый `driver.draw()` |
| Добыча raw-указателей device/context/rtv/format | `directx_renderer.rs:413` (`raw_gpu_access`) | A5/A6: собрать `RawGpuAccess::D3d11` на канвас |
| Инкремент `device_generation` при device-lost/recover | `directx_renderer.rs:579`; wgpu `recover()` | A5: `device_generation` в access |
| Изолированный `MTLRenderCommandEncoder` с `Load` | `gpui_macos/src/metal_renderer.rs` (под/над энкодер у батч-лупа) | A6 Metal: под/над фаза |
| wgpu native Device/Queue/Encoder/View + pass с `Load` | `gpui_wgpu/src/wgpu_renderer.rs` | A6 wgpu: под/над фаза |

Рабочий процесс: пишешь A1–A5 чисто → на A6 (renderer draw) открываешь эти файлы донора и переносишь
перечисленное, перевесив на новую модель.

## DROP — выкинуть (устарело, заменено)

- `Window::add_gpu_pass` + `GpuPhase` + `Subscription`-регистрация + renderer-Vec
  `under_/over_scene_passes` → заменено элементом + `scene.{under,over}_gpu_canvases` + `paint_gpu_canvas` (A1).
- `continuous_presentation` + `request_continuous_presentation` + 60-cap → заменено опросом `frame()` в
  существующем VSync-closure (A3): база и так будит каждый vblank, форсить present не нужно. Выдирать нечего —
  только понять, почему была и почему больше не нужна.
- `gpu_pass.rs` example → заменён `gpu_canvas.rs` (A7).

## Валидация §C

- [ ] C1–C3 перенесены и проверены; список сверен с `git log <base>..<fork>` + `docs/CheckNewFork.md` (полнота).
- [ ] Мультиоконный фриз не возвращается (waitable-on-present работает).
- [ ] Floating-окно: taskbar/рамка как раньше.
- [ ] DPI: detached-окно на мониторе со scale≠primary восстанавливается без множителя (2-3 close/restart без дрейфа).
