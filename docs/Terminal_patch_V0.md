# Terminal_patch_V0 — изменения в коде терминала

Status: impl spec v0, 2026-06-16. Всё, что пишется в код MoonTerminal под новый форк-API.
Форк-половина (что меняется в gpui) — отдельный док `ZED_FORK_V0.md`.

Задача: переехать с window-global GPU-pass + continuous-presentation на элемент `gpu_canvas`. Чарт-драйвер
реализует трейт форка; вся обвязка регистрации/презента/окклюзии исчезает.

## API форка, который реализуем (стык — канонично в ZED_FORK_V0)

```rust
trait GpuCanvasDriver {
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision;   // Skip | RequestPresent — CPU, до clear
    fn draw(&mut self, access: &RawGpuAccess) -> anyhow::Result<()>; // GPU, после clear, под scissor
}
struct GpuFrameInfo { now: Instant, bounds: Bounds<Pixels>, scale_factor: f32, presentable: bool }
enum RawGpuAccess<'a> { D3d11{ device, context, rtv, format, device_generation, size }, Metal{…,device_generation}, Wgpu{…,device_generation} }
```

Свойства, которые задаёт форк (от них пляшем):
- `frame()` зовётся каждый видимый канвас раз за такт ДО clear, без GPU-доступа; его `RequestPresent`
  подмешивается в present-решение окна.
- `draw()` зовётся для всех видимых канвасов на ЛЮБОМ present (immediate-mode: после Present бэкбуфер
  undefined). Значит `draw()` обязан быть дёшев (блит резидентных текстур), даже если канвас вернул `Skip`.
- Размещение/clip/bounds/lifetime/окно — дерево GPUI; канвас живёт в scene своего окна.

## B1. Driver в Entity; эмит элемента

Состояние чарта (камера, GPU-буферы, кэш-флаги, `device_gen`) переживает кадры → живёт в `ChartPanel`
(Entity, и так retained):
```rust
driver: Rc<RefCell<ChartState>>           // в ChartPanel
ChartPanel::render(): … gpu_canvas(self.driver.clone()) …   // под чарт; элемент stateless
```
**Никаких `driver::new()` в `render()`** — иначе состояние чарта обнуляется каждый кадр (тихий смертельный
баг). Несколько чартов → несколько `gpu_canvas`; каждый в scene своего окна.

## B2. Реализация `frame()` / `draw()`

**`frame()` (CPU, decide+prepare, один главпоточный вызов — роль `UpdateDrawTimerTimer`):**
- решение по своим триггерам: pixel-cross живого края; приход данных (rev/total); стакан 5Гц; live-trades
  ~10Гц; 500ms-пол диагностики; **+ фазовый офсет по B3**;
- `advance_camera` (целопиксельный сдвиг камеры живого края — математика);
- детект resize: дельта `info.bounds`/`info.scale_factor` vs кэш ⇒ `Skip` запрещён, `need_full=true`;
- `info.presentable==false` или нулевой размер ⇒ `Skip`;
- иначе при любом сработавшем триггере ⇒ `RequestPresent`, выставить флаги `need_full`/`append`.

**Present-rate чарта = частота pixel-cross, а ей правит `px/ms` (критично для цели №1).** Источник не
прорежаем и SkipCount убран → если откалибровать `px/ms` под монитор (144), пиксель пересекается каждый
vblank → 144 RequestPresent = ровно «бездумная очистка», которую убиваем. Поэтому:
- **`px/ms` калибровать под ЦЕЛЕВОЙ present-rate, НЕ под монитор:** `effective = monitor/round(monitor/60)`
  (144→72, 240→60, 60→60), 1px ≈ интервал этой частоты. Тогда на 144-источнике ~72 pixel-cross/сек →
  столько RequestPresent, остальные Skip.
- **Backstop в `frame()`:** минимальный интервал между RequestPresent (свой SkipCount-эквивалент). На
  зуме/catch-up `px/ms` высокий → pixel-cross зачастит выше целевого; guard режет. Одна `px/ms`-калибровка
  держит rate только в установившемся follow.
- **`ToBeFixed §3` закрыт:** старый рассинхрон был «внешний cap ~60 vs `px/ms` ~72»; теперь внешнего cap
  нет, чарт владеет ОДНОЙ величиной (cap = `px/ms` = effective-rate) → мисматчу неоткуда взяться.

**Инвариант advance ⟺ present (P5):** `advance_camera` только на пути `RequestPresent` (pixel-cross). На
Skip-пути — НИКАКИХ мутаций камеры, иначе край уедет без рисунка → рассинхрон. Структура: «pixel-cross? да →
advance + RequestPresent; нет → Skip без advance», НЕ «advance, потом решаю».

**`draw()` (GPU, после clear, под scissor):**
- apply uploads;
- **условный bake** (только по флагам из `frame()`): combo — инкрементальный append, иначе full при
  `need_full`; orderbook — bake при dirty + 200ms-троттл;
- composite-блит по слоям: background → grid → combo → orderbook → userdata;
- **device-lost:** сверить `access.device_generation` со своим кэшем; при расхождении — пересоздать ресурсы
  ДО рисования (combo инкрементит свой `device_gen` → полная перезаливка истории; scissor-rasterizer
  кэшировать по device-ptr и пересоздавать при смене; per-backend проверка `device_generation`). Иначе
  ресурсы старого device на новом context → blank/D3D error/паника.

Bake остаётся в `draw()` (рабочий путь, не переносить в `frame()` — там нет GPU-доступа); `frame()` его
только гейтит флагами. В установившемся скролле `draw()` = блит готовых текстур — дёшево.

## B3. Фазовый джиттер против синхронного спайка

Биржевое событие (закрытие минутной свечи) приходит на ВСЕ пары в один тик → N драйверов вернут
RequestPresent, и их периодические перерисовки сложатся в один тик = спайк.

Митигация (эталон MoonBot `ChartFrameUnit.pas:5919-5922`, `LastDrawTradesTick := TimeMS + random(96)`):
начальную фазу **периодических** каденций докраса каждого чарта (live-trades ~10Гц, стакан 5Гц,
500ms-диагностика) сместить на детерминированный/случайный офсет по `chart_id`, чтобы перерисовки растеклись
по тикам. Ставить офсет **сразу** (известный приём, не «после замера»). Замером проверять лишь остаточный
спайк от data-триггерного combo-rebake (он и так троттлится margin/transform — не каждый candle-close = full
bake).

## B4. Удалить / поглотить

**Удалить** (исчезает по конструкции — канвас живёт в scene своего окна):
- `ChartEngine::register_pass`/`unregister_pass`/`pass_subscription`/`pass_window`;
- `ChartPanel::present_guard`/`present_guard_window`;
- `Window::request_continuous_presentation()`-usage;
- **62.5Гц async prepare-таск** (`panels/chart.rs:182-209`) — сливается в `frame()`;
- `present_seq` как счётчик коллбеков (`chartdx/mod.rs:171,261`) — окклюзия теперь `GpuFrameInfo.presentable`;
- ручные detach-фиксы («старый пас в старом окне» / «снять пас у скрытой вкладки»).

> **Допущение (проверить на миграции):** удаление detach-фикса безопасно только потому, что detach дёргает
> dirty исходного окна (пейн убран из view-дерева) → исходное окно перерисуется без канваса → канвас
> выпадает из его cached-списков, и gate там его не опрашивает. Если detach не дёрнет dirty — канвас
> продолжит рисоваться в старом окне (текущий баг). Проверить явно.

**Поглотить:** decide + CPU-prepare (advance camera) раньше размазаны по prepare-таску и pass-колбэку (через
`present_seq`); теперь один главпоточный `frame()`.

## B5. Инвалидация (mouse-move — самый частый путь)

Сейчас: крестик = GPUI-quad, ридауты = GPUI-текст, `on_mouse_move`→`cx.notify()`→полный re-render
(`panels/chart.rs:666-705`, `axes.rs:252-308`). Цель §5.3: mouse-move НЕ дёргает GPUI view tree.

**Ключевой факт (железное правило + замер: `AGENTS.md` `orders_render=240` при `notify=2`): `cx.notify()`
НЕ изолируется — это top-down инвалидация.** Значит ЛЮБОЙ notify на каждый move = дёрнул дерево = по
определению нарушил §5.3, независимо от того «дёшево» это или «диско» (стоимость — рантайм-вопрос, статикой
не выводится). Поэтому всё, что меняется на каждый move, обязано быть ВНЕ notify-пути:
- **Крестик + ридауты цены/времени → один `gpu_canvas(...).over()`-оверлей.** Рисует линии крестика И цифры
  ридаутов из маленького атласа глифов (`0-9 . : -` — НЕ текстовый движок; паритет с MoonBot `RepaintCursor`,
  который плашки рисует в bm). `on_mouse_move` пишет позицию курсора в shared driver-state **без `cx.notify()`**;
  `frame()` over-канваса (опрашивается каждый vblank) видит смену позиции → `RequestPresent` → `draw()`
  перерисовывает крестик+цифры. mouse-move дерево не трогает → §5.3 достигнута.
  *(Поправка к прежней версии: «изолированный GPUI-оверлей, notify дёшев» — неверно, notify top-down; «глифы
  в чарте = overkill» — для цифр это крошечный атлас, не движок.)*
- **Подписи осей → `cx.notify()` оси — ДОПУСТИМО**, потому что меняются РЕДКО (только смена текста) и
  коалесцируются ≤4Гц-пульсом — это НЕ каждый move. В этом вся разница: редкий notify терпим, per-move notify
  = нарушение §5.3. Плюс кэш глифов по ключу (текст,размер,цвет) — закрывает ре-шейп ~17 подписей
  (`axes.rs:202-249`, зов `panels/chart.rs:758`).
- **Если digit-атлас в чарте откладывается:** ридауты на notify — ОСОЗНАННОЕ нарушение §5.3, чья цена —
  рантайм-вопрос (мерить `MOON_RENDER_DIAG`, не предполагать). Только как временное и только если замер
  показал дёшево; не как дизайн.

Разделение: `cx.notify()` — только редко-меняющийся GPUI-UI (оси-текст, тулбар, табы/док, тема, панели);
`gpu_canvas` — всё, что меняется на каждый move/тик (крестик, ридауты, pixel-cross, live-trades, стакан,
ордер-линии).

## Валидация (терминал)

- [ ] Mouse-move над чартом: крестик+ридауты в over-канвасе, `cx.notify()` НЕ зовётся → нет top-down
      re-render (проверить `MOON_RENDER_DIAG`); `combo_bake≈0`, `orderbook_bake≈0`.
- [ ] `RequestPresent` от чарта рисует в тот же тик; в установившемся скролле `draw()` = блит, без re-bake.
- [ ] Resize чарта ⇒ нет блита текстуры старого размера (no stale stretch).
- [ ] device-lost: после смены `device_generation` `draw()` пересоздаёт ресурсы (нет blank/паники/мусора).
- [ ] **Синхронный спайк:** N чартов на закрытии свечи не стакают перерисовки в один тик (джиттер работает).
- [ ] Detach/move таба: чарт рисует только в своём текущем окне (B4-допущение держится).
- [ ] Hidden tab: нет `gpu_canvas` в scene, нет prepare-работы; `present_seq`/62.5Гц-таск удалены.
- [ ] Пауза (не follow): `frame()` возвращает `Skip`, нет холостого present/bake.
- [ ] `orders_render`/`shell_render` остаются гейтнутыми, пока живой чарт скроллит; 4 чарта без мерцания/гонки.
- [ ] Метрики статус-бара/прочий UI обновляются по `cx.notify()`, не по чарт-каденции.

## Что НЕ менять

`NoFill` host/content в MoonPalette (чарт под сценой) — под `UnderScene` ничего не ломается. Per-layer кэш
combo/orderbook (ComboTex/BookTex bake-vs-blit гейты) — оставить как есть, `frame()` лишь гейтит их флагами.
