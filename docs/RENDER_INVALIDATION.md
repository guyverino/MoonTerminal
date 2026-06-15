# Идеология рисования чарта — что когда грязнится и пересчитывается

Карта инвалидации: каждый источник данных и каждый слой → **что делает его dirty** →
**где в коде**. Цель — чтобы следующий агент не проебал, например, что линии ордеров
обязаны грязниться и при управлении ими юзером (drag-drop), а не только от биржи.

Эталон — **MoonBot** (`ChartFrameUnit.pas`, модель `bm`/`bmGlass`/`RepaintCursor`,
выверено в `X:/proj-X/MoonBot/doc/doc_pipeline/04_chart_render-*.final-01.md`).
**Где наш код расходится со здравым смыслом или MoonBot — помечено ⚠️ = кандидат в баг.**

---

## 0. Главный принцип (общий с MoonBot)

**Слой пересобирается ТОЛЬКО когда меняются его пиксели. Движение мыши само по себе
пиксели контента НЕ меняет → контент не трогаем, рисуем только курсор поверх.**

У MoonBot это флаг `RepaintCursor`: «битмап валиден, нужен только повторный блит +
оверлей курсора». У нас этот принцип реализован **частично** (см. §5 GAP-1).

Дискретные редкие события (приход данных, pan/zoom, авто-Y) → пересборка. Непрерывные
(mouse-move без drag) → НЕ пересборка. Где слой пересобирается на mouse-move без причины —
это грех (см. `ЕБАНИНА.md`).

---

## 1. Эталон MoonBot — кто что грязнит (для сверки)

MoonBot копит 6 флагов между тиками, разруливает по стоимости (дорогое реже):

| Флаг | Что значит | Кто ставит |
|---|---|---|
| `RepaintMoving` | юзер тянет/зумит → DoMoving + полный ререндер | mouse-drag, колесо (зум), пинч |
| `RepaintFull/Order/Bg` | полный ререндер `bm` | **ордерные события** (`UpdateDraw` от воркеров/сервера/UI), **MouseUp после дропа ордера/фигуры** |
| `RepaintGlass` | полный ререндер (стакан) | **биржа обновила книгу** (гейт: smooth + OrderBookFastUpdate/CheckIfNeedRedraw 200мс) |
| `RepaintCursor` | **ТОЛЬКО блит + курсор, ререндера НЕТ** | **mouse-move без drag** |

Плюс: трейды дорисовываются **инкрементально** (fast-path `DrawTrades` поверх готового
`bm`, без очистки), а не полным ререндером; авто-сдвиг (`Latest`) → `RepaintFull` на
**пересечении границы пиксель-времени** (контент сдвинулся на ≥1px).

**Фигуры (`ChartObjects`, §5 MoonBot-доки)** грязнятся:
1. **юзер рисует/тащит** фигуру (Ctrl+ЛКМ draw → MouseMove → MouseUp);
2. **алерт сработал** — активный алерт **инвертирует цвет** фигуры (`SwapColor`);
3. **карандаш `Pencil`** в эмуляторе — точки, чьё время догнало часы, **вырезаются из
   фигуры и становятся эмулированными трейдами** (фигура сама себя потребляет);
4. `CO_OrderLine` — **системная** фигура (трасса цены ордера), точки добавляют
   ордер-воркеры на каждый replace.

**Y-диапазон** (trap 11): НЕ пересчитывается каждый кадр — `yCenter` ползёт EMA,
пересчёт по ReShift (юзер неактивен 25с + цена ушла к краю 10%). **При перетаскивании
ордера диапазон ЗАМОРОЖЕН** — иначе ордер уезжал бы из-под курсора.

---

## 2. Модель терминала — три уровня инвалидации

Поток рисования у нас иной (own-pass DX11 под сценой GPUI), но уровни те же:

```
Уровень A — cx.notify()        перерисовать элемент ChartPanel (запустить render())
Уровень B — prepare()          пересчитать вид + залить данные слоёв в VRAM (гейт!)
Уровень C — own-pass callback  GPU рисует ВСЕ активные панели (каждый кадр GPUI)
+ axes overlay (GPUI)          оси/подписи/крестик поверх
```

### Уровень A — `cx.notify()` (что будит render)
| Источник | Где |
|---|---|
| приход данных (data_signature сменилась) | `observe(&backend)` — [panels/chart.rs:96](../crates/moon-ui-gpui/src/panels/chart.rs) |
| дренаж данных | timer-цикл — [main.rs](../crates/moon-ui-gpui/src/main.rs) (`drain()->bool`, не coord-пульс) |
| mouse move/wheel/drag/hover-change | input-хендлеры — [panels/chart.rs](../crates/moon-ui-gpui/src/panels/chart.rs), только при смене cursor/drag/view |
| fast-чарт: каждый vsync | `request_animation_frame` — [panels/chart.rs:271](../crates/moon-ui-gpui/src/panels/chart.rs) |
| смена настроек темы/ордеров/масштаба/follow | `settings_changed` — [panels/chart.rs:290-296](../crates/moon-ui-gpui/src/panels/chart.rs) |

### Уровень B — `prepare()` гейт
[panels/chart.rs:313](../crates/moon-ui-gpui/src/panels/chart.rs): `cadence_due || data_changed || geometry_changed || view_changed`
- **cadence_due** — fast-чарт ~60Гц (каждый N-й vblank), не-fast — каждый render.
- **data_changed** — `data_signature` сменилась ([chartdx/mod.rs:642](../crates/moon-ui-gpui/src/chartdx/mod.rs): `ticks_rev+price_lines_rev+book_rev` + `orders_rev` по панелям).
- **geometry_changed** — ресайз слота (`chart_dev`/`chart_bounds`).
- **view_changed** — `view_dirty`: ставится при pan/zoom/follow (`mark_input_changed`), смене настроек, open/add монеты, ПКМ-up, TTL-prune.

### Уровень C — own-pass GPU
[chartdx/mod.rs:197-304](../crates/moon-ui-gpui/src/chartdx/mod.rs): callback рисует **ВСЕ активные панели целиком КАЖДЫЙ кадр GPUI**. Гейта «кадр валиден → только блит» НЕТ. ⚠️ см. GAP-1.

---

## 3. Реестр источников: что грязнит что

### 3.1 Рыночные данные (от биржи) — ревизии в `MarketView`
| Ревизия | Грязнится когда | Где бампается | Частота |
|---|---|---|---|
| `ticks_rev` | биржа прислала трейды | `push_ticks` — [market/mod.rs:99](../crates/moon-core/src/market/mod.rs) | поток сделок |
| `book_rev` | биржа обновила стакан | `set_book` — [market/mod.rs:115](../crates/moon-core/src/market/mod.rs) | ~20Гц (троттл фида 50мс) |
| `price_lines_rev` | пришла точка last/mark price | `push_price_line` — [market/mod.rs:110](../crates/moon-core/src/market/mod.rs) | ~0.5Гц (`UpdateMarketsList`, дефолт 2с) |

### 3.2 Ордера юзера/бота — `orders_rev`
[session/order_lines.rs:298](../crates/moon-core/src/session/order_lines.rs): `OrderLineStore::update` бампает `rev`, когда `changed`:
- новый ордер / воскрешение закрытого;
- смена panic/moonshot/коридора/liq;
- сдвиг любой линии (вход/sell/стоп/трейл/TP/vstop/pending — `update`/`update_server` вернул true);
- ордер закрылся.

**Закрытие линии — по ЯВНОМУ флагу `job_is_done`** (как MoonBot `o.IsClosed`): стор помечает
закрытым сразу, пока ордер ещё в снимке ([order_lines.rs](../crates/moon-core/src/session/order_lines.rs)).
Грейс `CLOSE_GRACE_MS` (2500мс) — теперь лишь **backstop** на ордер, убранный ядром без виденного
нами `job_is_done`. (Было: закрытие по исчезновению+грейс → задержка ~2.5с; см. ЕБАНИНА Пример 4.)

**Источник данных — биржевые снимки ордеров** (фид, троттл 250мс → ≤4Гц). ⚠️ см. GAP-6.

> ⚠️ **GAP-3 (не баг, а незакрытая идеология): нет триггера «юзер управляет ордером».**
> По MoonBot и здравому смыслу ордер грязнится ДВУМЯ путями: **(1) биржа/бот**,
> **(2) юзер тащит ордер (drag-drop)** → `RepaintMoving` на ходу + `RepaintFull` на дропе.
> У нас drag ордеров НЕ реализован (`uid` в `RetainedOrder` помечен `#[allow(dead_code)]`
> «этап 5»). Когда будут — **обязательно** бампать инвалидацию userdata при старте/ходе/
> дропе перетаскивания, иначе линия не поедет за курсором.

### 3.3 Фигуры (`ChartObjects`) — ⚠️ GAP-4: слоя НЕТ
В терминале нет слоя фигур (9 типов MoonBot: PriceLine/Channel/Fibo/Triangle/Rect/Pencil/
OrderLine/SysRect/NewsMarker). `userdata` рисует ТОЛЬКО линии ордеров. Когда фигуры
появятся — модель инвалидации по MoonBot §5:
- **юзер рисует/тащит** фигуру → dirty (как ордер-drag);
- **алерт сработал** → dirty (инверсия цвета `SwapColor`);
- **карандаш-эмулятор** → точки превращаются в трейды → dirty как трейды;
- остальное время фигуры статичны (координаты в данных Time/Price, переживают зум/скролл) →
  **НЕ грязнить на pan/zoom-контент, только пересчитать их экранную проекцию из трансформа.**

### 3.4 Вид (pan/zoom/follow/авто-Y) — грязнит ВСЁ рисование
| Действие | Что ставит dirty | Где |
|---|---|---|
| pan по X (drag/Shift-колесо) | `view.follow=false` + `view_dirty` | [moon-chart/view.rs:226](../crates/moon-chart/src/view.rs), `mark_input_changed` |
| zoom по X (колесо) | `view_dirty` | `zoom_x_at` [moon-chart/view.rs:243](../crates/moon-chart/src/view.rs) |
| pan/zoom по Y | `view_dirty` | `pan_y_px`/`rmb_zoom` |
| follow Live/Пауза (тулбар) | `set_follow` → resume/freeze панелей | [chartdx/mod.rs:597](../crates/moon-ui-gpui/src/chartdx/mod.rs) |
| **авто-Y (масштаб сам)** | `update_y` двигает `render_center/render_range` | [chartdx/mod.rs:409](../crates/moon-ui-gpui/src/chartdx/mod.rs) каждый prepare |

Любое из этого меняет трансформ → combo/orderbook/userdata перерисовываются с новым видом.

---

## 4. Что пересобирается когда — по слоям (это и есть Tier-2-кэширование)

«Пересборка» = CPU-работа + заливка в VRAM. Ниже — **точный гейт** каждого слоя.

### combo (трейды) — [chartdx/mod.rs:521](../crates/moon-ui-gpui/src/chartdx/mod.rs), [chartdx/combo.rs](../crates/moon-ui-gpui/src/chartdx/combo.rs)
- **append живого края** по абсолютному индексу `[last_total, total)` — на новые тики. Дёшево.
- **полный re-bake** только при: device-lost / регрессии `total` (смена монеты, reset рынка) /
  отставании дальше глубины кольца / смене Y-трансформа (`price_to_px`/`view_price0`) / зуме X /
  исчерпании 20%-margin при скролле.
- ⚠️ **GAP-2:** `set_price_lines` (приход last/mark-точки, ~0.5Гц) **инвалидирует весь
  combo-битмап** ([combo.rs:137](../crates/moon-ui-gpui/src/chartdx/combo.rs)) → полный re-bake.
  У MoonBot last/mark price — ОТДЕЛЬНАЯ линия-серия на канве, НЕ запечена в битмап трейдов.
  Это CRIT-A (отложено): линии надо вынести из текстуры в дешёвый live-pass.

### orderbook (стакан) — [chartdx/mod.rs:471](../crates/moon-ui-gpui/src/chartdx/mod.rs)
- Пересборка `build_instances` **только** при `book_rev != last || ценовое окно (lo/hi) сдвинулось`.
- `lo/hi = render_center ± render_range/2`, снапнуто (`CENTER_SNAP_PX`) → в live не дёргается каждый кадр.
- **НЕ сортируем** заново (биржа уже отсортировала; страховка `debug_assert`) — [data/orderbook.rs](../crates/moon-core/src/data/orderbook.rs). 1:1 с MoonBot («стакан грязнится, когда его обновляет биржа»).
- device-lost инвалидирует гейт (иначе после потери device стакан гас).

### userdata (линии ордеров) — [chartdx/mod.rs:492](../crates/moon-ui-gpui/src/chartdx/mod.rs)
- Пересборка `build_order_geometry` **только** при `orders_rev != last`; смена `OrdersStyle` сбрасывает гейт; device-lost инвалидирует.
- ⚠️ **долг (HIGH-C, отложено):** строятся ВСЕ ордера (`±∞` вместо окна) — нет time-куллинга.

### grid / background
- Параметры ставятся каждый prepare (фикс-позиции экрана + `nice_interval`). Дёшево — кэшировать не стоит (16 линий).

### buy_sell_range (авто-Y по ордерам) — [session/order_lines.rs](../crates/moon-core/src/session/order_lines.rs)
- **Кэш по рынкам**, пересобирается `rebuild_buy_sell_ranges` **только при `changed`** в `update()` (≤4Гц), не сканируется каждый prepare. Свежесть гарантирована: цены линий мутируют только в `update()`.

### axes overlay — [panels/chart.rs:328](../crates/moon-ui-gpui/src/panels/chart.rs), [axes.rs](../crates/moon-ui-gpui/src/axes.rs)
- `axis_panes` (раскладка) считается **один раз за кадр** (был дважды), переиспользуется для hit-теста и отрисовки.
- ⚠️ **долг:** подписи осей **ре-шейпятся (glyph-раскладка) КАЖДЫЙ кадр** без кэша — на mouse-move зря. MoonBot держит `TextWidthInt`-кэш по строке (5с). Кандидат на кэш по (текст, стиль).

### Дренаж данных — [main.rs](../crates/moon-ui-gpui/src/main.rs)
- Данные дренятся **~60Гц** (фид кладёт каждые ~8мс); тяжёлая координация (reconcile_providers/метрики/сохранения) — **~100мс** (каждый 6-й тик). Окна будятся **только когда реально пришли данные** (`drain()` → bool) и сработал общий cap `notify_due`. Координация/сохранения сами по себе НЕ являются причиной UI-render.

---

## 5. ⚠️ Расхождения с MoonBot / здравым смыслом = кандидаты в баги

| # | Расхождение | Здравый смысл / MoonBot | Статус |
|---|---|---|---|
| **GAP-1** | own-pass рисует ВСЕ слои каждый кадр; на mouse-move (данные статичны) тоже | `RepaintCursor`: блит готового кадра + крестик | **Tier 3, не сделано** — главная дыра |
| **GAP-2** | last/mark-линия запечена в combo-битмап → её приход (0.5Гц) = полный re-bake истории | отдельная линия-серия на канве | **CRIT-A, отложено** |
| **GAP-3** | ордера грязнятся только от биржи; нет триггера «юзер тащит ордер» | (1) биржа/бот **+ (2) drag-drop** | drag не реализован — **учесть при реализации** |
| **GAP-4** | нет слоя фигур (ChartObjects) | фигуры + dirty на draw/drag, алерт-инверсия, карандаш→трейды | не реализовано — модель в §1/§3.3 |
| **GAP-5** | авто-Y `update_y` каждый prepare → может дёргать combo Y-rebake | MoonBot стабилизирует EMA, ReShift-триггер, **заморозка Y при drag ордера** | проверить/сверить — возможный источник лишних re-bake |
| **GAP-6** | данные ордеров ПОЛЛЯТСЯ снимком (250мс троттл), `Event::Order` игнорится (`_ => {}` в live.rs) | событийно — мгновенно по открытию/закрытию (MoonBot рисует по событию) | закрытие уже по `job_is_done`; темп данных всё ещё ≤250мс — добить событиями |

GAP-5 особенно: при перетаскивании ордера (когда появится) Y-диапазон **обязан замораживаться**,
иначе ордер уедет из-под курсора — это прямой урок MoonBot (trap 11). Сейчас `update_y`
безусловный — заведомая мина для будущего order-drag.

---

## 6. Правила для агентов (как не проебать инвалидацию)

1. **Добавляешь слой/данные** — сначала ответь: *что делает его пиксели другими?* Это и есть
   его единственный триггер dirty. Заведи `rev`/гейт ровно на это, не на «каждый кадр».
2. **Mouse-move ничего не грязнит** в контенте. Если твой слой пересобирается на move без
   данных — это баг/грех (ЕБАНИНА).
3. **Ордера и фигуры грязнятся и от юзера (drag-drop), и от данных** — не забудь первый путь.
4. **Авто-Y и pan/zoom грязнят весь контент** (меняют трансформ) — это норма; задача в том,
   чтобы при drag замораживать Y (GAP-5) и не пересобирать неизменную историю (GAP-2).
5. **Источник правды один.** Не держи второй кэш того же; инвалидируй ровно по событию-причине.
6. Сверяйся с MoonBot `04_chart_render-*.final-01.md` §1 (флаги) и §5 (фигуры) — там модель
   выверена практикой; расхождение нашего кода с ней = почти всегда наш баг, а не «улучшение».

---

## 7. Реестр observe/гейтов + ИЗМЕРЕННЫЕ частоты (диаг)

ВСЕ места, где UI подписан на `backend`, и их гейты. **Гейт = условие, при котором вью
само-нотифается** (`cx.notify()`). Где гейта нет — вью перерисовывается на каждый backend-notify.

| Вью | Файл | Гейт | Исключение/частота |
|---|---|---|---|
| **ChartPanel (Main/fast)** | [chart.rs](../crates/moon-ui-gpui/src/panels/chart.rs) | `data_signature` + троттл **≥250мс** | **НЕТ `raf`**. Скролл — 60-Гц prepare-задача (`new()`) + own-pass по vsync (форк), БЕЗ notify. Оси/хром освежает общий backend-пульс |
| **ChartPanel (AddToChart)** | [chart.rs](../crates/moon-ui-gpui/src/panels/chart.rs) | `data_signature` + троттл **≥1с** | фон/мультичарт, нет задачи; скролл по observe + present |
| **OrdersPanel** | [orders.rs](../crates/moon-ui-gpui/src/panels/orders.rs) | `orders_sig`(эпоха/статус) ИЛИ 1с (цены) | **ИСКЛЮЧЕНИЕ: НЕ ЧАЩЕ 250мс** — ордерные ивенты летят часто, глаз не различит → коалесцируем до 4 Гц |
| **LogPanel** | [log.rs:85](../crates/moon-ui-gpui/src/panels/log.rs) | `log_sig` сменился | — |
| **DetectsPanel** | [detects.rs:60](../crates/moon-ui-gpui/src/panels/detects.rs) | `detects_sig` сменился | TTL-prune отдельным one-shot timer до ближайшего истечения, не от backend observe |
| **ReportPanel** | [report.rs:119](../crates/moon-ui-gpui/src/panels/report.rs) | `generation` сменился | — |
| **Shell** | [main.rs](../crates/moon-ui-gpui/src/main.rs) | **троттл ≥250мс** (`last_notify`) | показывает диагностику (fps/тики/cpu) — ≤4 Гц человеку хватает |
| **DetachedWindow** | [detached.rs:128](../crates/moon-ui-gpui/src/detached.rs) | backend observe убран | геометрия сохраняется по `observe_window_bounds`, не polling по данным/render |
| **ChartTabs** | [chart_tabs.rs:69](../crates/moon-ui-gpui/src/chart_tabs.rs) | `chart_tabs_sig`: detects_rev группы, split-настройка, open_request только своей группы | `open_request` больше не съедается чужой группой |
| **StrategiesView** | [strategies/mod.rs:112](../crates/moon-ui-gpui/src/strategies/mod.rs) | `strategies_sig` (strategies/schema rev) | `param_deps.toml` hot-reload отдельным 1с file-mtime timer, не backend observe |
| **SettingsView** | [settings/mod.rs:221](../crates/moon-ui-gpui/src/settings/mod.rs) | `settings_sig` (draft/config + статусы ядер) | only-when-open; вводы нотифают сами |

**AddToChart TTL:** [chart.rs](../crates/moon-ui-gpui/src/panels/chart.rs) больше не prune'ит TTL из backend observe. У каждой AddToChart-панели свой one-shot timer до ближайшего deadline; он будит панель только если реально удалил истёкшую монету.

**Источник backend-notify (ГЛАВНЫЙ рычаг):** дренаж-цикл [main.rs](../crates/moon-ui-gpui/src/main.rs),
`notify_due = tick % 16` → **≤4 Гц (256мс)**, единый UI-пульс, И только когда `drain()->bool` показал
приход данных (causal-гейт: рынок молчит → нет холостых top-down перерисовок). Данные дренятся 60 Гц,
но `cx.notify()` бэкенда — редкий и по факту изменения. Синхронизирует ВСЕ backend-зависимые
пробуждения хрома в один кадр (см. инсайт ниже). Это пожарный кап top-down сцепки, не замена
view-caching (отдельная задача в moon-palette).

### ИЗМЕРЕНО счётчиками (`diag.rs`, монитор 240 Гц, fast-чарт в follow)
```
ДО фикса:    orders_render=240  shell_render=240  chart_render=240  chart_raf=240
после raf-кила: orders_render=13  chart_raf=0   (диско монитор-рейта убито, но >4 Гц)
+ единый пульс: orders_render=2-4 shell_render=2-4 chart_render=2-4 backend_notify=2-4 chart_raf=0
```

**Проверка 2026-06-16 после dirty/notify cleanup:** `MOON_RENDER_DIAG=1`, MSVC debug exe,
8-10 секунд живого окна. Слепых `observe(&backend, |_this, _b, cx| cx.notify())` grep не находит.
Фактический steady-state: `orders_render=2-4/s`, `shell_render=2-4/s`, `chart_render=2-4/s`,
`backend_notify=2-4/s`; `chart_input_notify=0`, `chart_ttl_notify=0`, `chart_open_notify=0`,
`chart_canvas_notify=1` только на первичное измерение геометрии окна.

**Инсайт (доказан замером, не догадкой):** гейт ОТДЕЛЬНОЙ вьюхи (Shell/Orders/chart) против top-down
рендера БЕССИЛЕН — GPUI перерисовывает дерево СВЕРХУ, минуя само-гейт листа (видели: гейтнули Shell
10→3, а `orders_render` остался 13, т.к. дерево дёргали ДРУГИЕ источники). Любой `cx.notify()` любой
вьюхи метит дёрти весь путь до корня → `draw_roots` перерисовывает ВСЁ поддерево (Orders встроена
через `into_any_element`, GPUI её НЕ переиспользует). Поэтому лечится НЕ листовыми гейтами, а
**редким пульсом у ИСТОЧНИКА** (backend-notify ~4 Гц): все пробуждения хрома коалесцируются в один
кадр. Листовые гейты остаются как контракт «буди свой срез» (на случай отдельных быстрых источников).

### ✅ GAP-7 (корневой) — ЗАКРЫТ
Было: чарту нужен present каждый vsync (own-pass скролл), он добивался этого через
`request_animation_frame`, который перерисовывал GPUI-дерево (а не только презентил) на refresh
монитора (240 Гц) = диско Orders. **Фикс (реализован):**
1. **ФОРК** (ZedFork, локальный патч) — **opt-in continuous-present, PR-grade, кроссплатформенный**:
   новый `Window::request_continuous_presentation() -> Subscription` ([gpui/src/window.rs]). Держишь
   гайд → счётчик `continuous_presentation > 0` → общее замыкание `request_frame` (одно на все
   платформы) OR-ит его в `needs_present` → окно презентит каждый кадр БЕЗ дёрти вьюх. GPUI `present()`
   → `platform.draw(cached_scene)` → `run_gpu_passes`: own-pass перерисовывается, GPUI-элементы — НЕТ.
   **Кроссплатформенно по построению** (НЕ «любой active pass», а явный opt-in — иначе upstream завернёт
   по батарее): счётчик в platform-agnostic `gpui::Window`, а каждый frame-драйвер гонит то самое
   замыкание per-frame — Windows vsync-поток, macOS `CVDisplayLink` (пока active), Linux X11 calloop
   refresh-таймер, Wayland `wl_callback`-цикл (самоподдержка через present→commit). Per-platform кода
   НОЛЬ. Прежний хак (`directx_renderer.has_active_gpu_pass()` + `events.rs require_presentation`) убран.
   _Windows frame-pacing (WM_GPUI_VSYNC_TICK, waitable swapchain) — ОТДЕЛЬНЫЙ пласт форка, в этот PR не входит._
2. **Чарт** ([chart.rs](../crates/moon-ui-gpui/src/panels/chart.rs)): убран `request_animation_frame`;
   живой край двигает 60-Гц prepare-задача (`follow_edge(now)`); гайд берётся в render на live-follow
   (только fast-чарт, привязан к WindowId), дропается на паузе/скрытии вкладки (present → on-demand,
   батарея). Задача движет край, ЛИШЬ когда держим guard И вырос `present_seq` own-pass'а (счётчик
   реальных present'ов) → матчит фактический present-rate и СПИТ при occluded-окне (macOS CVDisplayLink
   стоп → present=0) или inactive (Windows 30fps → 30 prep/с, а не 60 вхолостую). Скролл гладкий БЕЗ notify.
3. **Источник** ([main.rs](../crates/moon-ui-gpui/src/main.rs)): backend-notify → ≤4 Гц единый causal пульс.

Итог (замерено): `orders_render: 240 → ~3-4/s` (≥250мс, требование юзера), `chart_raf=0`.
`chart_task_prep` = present-rate чарта: ~60 на активном фокусном окне, ~30 на inactive (заcapped),
0 на occluded — задача больше не молотит вхолостую, когда кадров нет.
**Остаточная архитектурная сцепка:** Orders всё ещё перерисовывается top-down (не кэш-вью). Пока её
держат на ≤4 Гц пульсом — ОК. Полная развязка (per-panel view-caching, чтобы Orders рисовалась ТОЛЬКО
по своему гейту независимо от хрома) — отдельная задача в moon-palette (DockArea встраивает панели
не как кэш-вью). Тогда пульс можно поднять, не задевая Orders.
