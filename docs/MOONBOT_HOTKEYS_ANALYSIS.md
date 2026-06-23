# MoonBot Hotkeys Analysis

Дата: 2026-06-23.

Цель: зафиксировать полный MoonBot-реестр горячих клавиш и мышиных жестов,
чтобы в MoonTerminal была та же карта bindings. На этом этапе в терминале
закладывается список и сохранение настроек; сами действия подключаются отдельно
по этой таблице.

## Источники Delphi

- `X:\proj-X\MoonBot\src\Config.pas:439` — `THotkeysConfig`, основной record клавиатурных хоткеев.
- `X:\proj-X\MoonBot\src\Config.pas:481` — `TManualStratsConfig.hotKeys[1..10]`.
- `X:\proj-X\MoonBot\src\Config.pas:518` — `TMultiOrdersConfig`, мышиные жесты стакана.
- `X:\proj-X\MoonBot\src\Vars.pas:33` — enum `TOrderReplaceClick`.
- `X:\proj-X\MoonBot\src\Unit1.pas:17240` — назначение `HotkeysConfig.*` на `TAction.ShortCut`.
- `X:\proj-X\MoonBot\src\Unit1.pas:21957` — назначение manual strategy hotkeys.
- `X:\proj-X\MoonBot\src\PropU.pas:1758` и `X:\proj-X\MoonBot\src\PropU.pas:2698` — чтение/запись хоткеев в UI настроек.
- `X:\proj-X\MoonBot\src\PropU.pas:2918` и `X:\proj-X\MoonBot\src\PropU.pas:3766` — чтение/запись мышиных жестов.
- `X:\proj-X\MoonBot\src\ChartFrameUnit.pas:2158` — интерпретация `TOrderReplaceClick`.
- `X:\proj-X\MoonBot\src\ChartFrameUnit.pas:2549` — применение мышиных жестов в зоне стакана.

## Короткий итог

Активный пользовательский набор MoonBot состоит из:

1. 6 hotkeys размера ручного ордера: `OKeys[1..6]`.
2. 6 hotkeys fixed sell: `SKeys[1..6]`.
3. 27 одиночных `HotkeysConfig` actions.
4. 10 hotkeys ручных стратегий: `ManualStratsConfig.hotKeys[1..10]`.
5. 12 мышиных click-bindings для стакана + флаг `SameHotkeysForMove`.

В `THotkeysConfig` есть raw-поле `k6: TShortCut`, а в `TManualStratsConfig`
есть raw-поля `u1/u2: TShortCut`; по grep они не назначаются в UI и не
попадают в `TAction.ShortCut`. Это резерв/след бинарного формата, а не
пользовательские действия.

## Формат мышиного жеста

Delphi enum `TOrderReplaceClick`:

| Delphi value | Смысл |
|---|---|
| `None_Click` | не назначено |
| `Dbl_Click` | двойной левый клик без модификаторов |
| `CTRL_Click` | Ctrl + левый клик |
| `Shift_Click` | Shift + левый клик |
| `Alt_Click` | Alt + левый клик |
| `Mid_Click` | средняя кнопка без модификаторов |
| `CTRL_Mid` | Ctrl + средняя кнопка |
| `Shift_Mid` | Shift + средняя кнопка |
| `Alt_Mid` | Alt + средняя кнопка |
| `Dbl_Right` | двойной правый клик без модификаторов |
| `CTRL_Right` | Ctrl + правый клик |
| `Shift_Right` | Shift + правый клик |
| `Alt_Right` | Alt + правый клик |
| `CTRL_Dbl` | Ctrl + двойной левый клик |
| `Shift_Dbl` | Shift + двойной левый клик |
| `Alt_Dbl` | Alt + двойной левый клик |

`ChartFrameUnit.IsGoodClick` проверяет эти значения отдельно для левой,
средней и правой кнопки. Для правой кнопки `Dbl_Right` дополнительно участвует
в `NeedDeferRightPopup`, чтобы popup меню не конфликтовало с double-right
действиями.

## Пресеты размера ордера

Поля MoonBot: `HotkeysConfig.OKeys[1..6]`, `HotkeysConfig.OSize[1..6]`,
`HotkeysConfig.bNum`.

| Slot | Default key | Default size | Что делает |
|---|---:|---:|---|
| Order size 1 | `F1` | `FixedTradeBalance` | выбирает активный размер ручного ордера |
| Order size 2 | `F2` | BTC: `0.1`, USD/USDT: `100`, ETH: `1` | то же |
| Order size 3 | `F3` | BTC: `0.5`, USD/USDT: `500`, ETH: `5` | то же |
| Order size 4 | `F4` | BTC: `1`, USD/USDT: `1000`, ETH: `10` | то же |
| Order size 5 | `F5` | BTC: `2`, USD/USDT: `3000`, ETH: `20` | то же |
| Order size 6 | `F6` | BTC: `5`, USD/USDT: `10000`, ETH: `100` | то же |

MoonTerminal сейчас переносит key-list. Значения размера ордера уже живут в
персерверных `order_sizes` и не смешиваются с hotkey-list.

## Fixed sell presets

Поля MoonBot: `HotkeysConfig.SKeys[1..6]`, `HotkeysConfig.SPrice[1..6]`,
`HotkeysConfig.sbNum`.

| Slot | Default key | Default percent | Что делает |
|---|---:|---:|---|
| Fixed sell 1 | `Shift+F7` | `1%` | выбирает fixed sell процент |
| Fixed sell 2 | `Shift+F8` | `3%` | то же |
| Fixed sell 3 | `Shift+F9` | `5%` | то же |
| Fixed sell 4 | `Shift+F10` | `10%` | то же |
| Fixed sell 5 | `Shift+F11` | `25%` | то же |
| Fixed sell 6 | `Shift+F12` | `100%` | то же |

## Основные keyboard actions

| MoonBot field | Default | MoonBot action | Что делает |
|---|---:|---|---|
| `CancelBuy` | empty | `ActionCancelBuy` | отмена buy/pending buy по активному рынку |
| `PanicSell` | empty | `ActionPanicSell` | аварийная продажа активной позиции/рынка |
| `PanicSellOne` | `Ctrl+F1` | `ActionPanicSellOne` | аварийная продажа одного выбранного/активного sell-сценария |
| `CancelAllBuys` | `Ctrl+Del` | `ActionCancelAllBuys` | `CloseAllBuyOrders` + `CancelPendings` |
| `JoinSells` | empty | `ActionJoinSells` | объединяет sell-ордера активного рынка |
| `SwitchCharts` | empty | `ActionSwitchCharts` | переключает fullscreen-график между открытыми chart frames |
| `ReloadBook` | empty | `ActionReloadBook` | перезагружает стакан активного рынка |
| `ReloadChart` | `Ctrl+R` | `ActionReloadChart` | перезагружает историю/график |
| `NewLong` | empty | `ActionNewLong` | создать/подготовить long-сценарий |
| `NewShort` | empty | `ActionNewShort` | создать/подготовить short-сценарий |
| `SplitOrder` | empty | `ActionSplitOrder` | split выбранного visual order |
| `SplitOrderX` | empty | `ActionSplitOrderX` | split крупнейшего активного sell активного рынка на `SplitParts` |
| `ShiftBuyUp` | empty | `ActionShiftBuyUp` | `OrdersWorkers.MoveAllBuys(..., +1)` |
| `ShiftBuyDown` | empty | `ActionShiftBuyDown` | `OrdersWorkers.MoveAllBuys(..., -1)` |
| `ShiftSellUp` | empty | `ActionShiftSellUp` | `OrdersWorkers.MoveAllSells(..., +1)` |
| `ShiftSellDown` | empty | `ActionShiftSellDown` | `OrdersWorkers.MoveAllSells(..., -1)` |
| `MakeShot` | `Ctrl+F10` | `ActionMakeShot` | screenshot активного отчёта/графика |
| `MakeShotBot` | `Ctrl+F12` | `ActionMakeShotBot` | screenshot через bot/info form |
| `ScalePlus` | `Ctrl+Q` | `ActionScalePlus` | увеличить вертикальный scale/price range и recenter |
| `ScaleMinus` | `Ctrl+W` | `ActionScaleMinus` | уменьшить вертикальный scale/price range и recenter |
| `SellPlus` | `Ctrl+1` | `ActionSellPlus` | увеличить fixed sell/TP процент |
| `SellMinus` | `Ctrl+2` | `ActionSellMinus` | уменьшить fixed sell/TP процент |
| `SpyMode` | `F7` | `ActionHidelabels` | hide-balance/spy mode |
| `ShowCharts` | fresh config `F4` | `ActionShowCharts` | показать/скрыть charts UI |
| `SwitchFigure` | `Ctrl+F` | `ActionSwitchFigure` | переключить текущий drawing tool |
| `FitSells` | `Ctrl+S` | `ActionFitSells` | режим rectangle fit-sells |
| `Broadcast` | empty | `ActionBroadCastCoin` | broadcast активной монеты во внешний сценарий |

Примечание: в миграциях старых конфигов `ShowCharts` встречается как `F9`,
но fresh default в `SetDefault` — `F4`.

## Manual strategy hotkeys

Поля MoonBot: `ManualStratsConfig.hotKeys[1..10]`,
`ManualStratsConfig.ShowButton[1..10]`, `ManualStratsConfig.UseButtons`.

| Slot | Default key | Action | Что делает |
|---|---:|---|---|
| Manual strategy 1 | empty | `ManStratAction1` | `sbManualStrats[1].DoClick` |
| Manual strategy 2 | empty | `ManStratAction2` | `sbManualStrats[2].DoClick` |
| Manual strategy 3 | empty | `ManStratAction3` | `sbManualStrats[3].DoClick` |
| Manual strategy 4 | empty | `ManStratAction4` | `sbManualStrats[4].DoClick` |
| Manual strategy 5 | empty | `ManStratAction5` | `sbManualStrats[5].DoClick` |
| Manual strategy 6 | empty | `ManStratAction6` | `sbManualStrats[6].DoClick` |
| Manual strategy 7 | empty | `ManStratAction7` | `sbManualStrats[7].DoClick` |
| Manual strategy 8 | empty | `ManStratAction8` | `sbManualStrats[8].DoClick` |
| Manual strategy 9 | empty | `ManStratAction9` | `sbManualStrats[9].DoClick` |
| Manual strategy 10 | empty | `ManStratAction10` | `sbManualStrats[10].DoClick` |

В MoonBot action включается только если `ShowButton[k] = true`.

## Мышь в зоне стакана

Активный путь: `ChartFrameUnit.ImageMouseDown`, только когда курсор находится
правее plot-зоны (`x > xStart + DrawWidth`) и рынок есть.

| Config field | Default | Delphi action branch | Что делает |
|---|---:|---|---|
| `MultiOrders.BuySetClick` | `Dbl_Click` | `Place Long` | ставит long/buy по цене под курсором |
| `MultiOrders.ShortSetClick` | `None_Click` | `Place Short` | ставит short по цене под курсором |
| `PendingOrderSetClick` | `None_Click` | `Pending Long` | ставит pending long |
| `MultiOrders.PendingShortSetClick` | `None_Click` | `Pending Short` | ставит pending short |
| `MultiOrders.BuyMoveClick` | `Shift_Click` | `Move Open` | двигает open/buy через `OrdersWorkers.MoveAllBuys(... ReplaceBuyKind ...)` |
| `MultiOrders.SellMoveClick` | `CTRL_Click` | `Move TP` | двигает TP/sell через `OrdersWorkers.MoveAllSells(... ReplaceSellKind ...)` |
| `MultiOrders.BuyMoveClick2` | `None_Click` | `Move Open #2` | второй move-open жест через `ReplaceBuyKind2` |
| `MultiOrders.SellMoveClick2` | `None_Click` | `Move TP #2` | второй move-TP жест через `ReplaceSellKind2` |
| `MultiOrders.SameHotkeysForMove` | `true` | settings sync | short move gestures копируются из long move gestures |
| `MultiOrders.ShortBuyMoveClick` | `Shift_Click` при default sync | `Move Open` side `FP_Short` | двигает open/buy short |
| `MultiOrders.ShortSellMoveClick` | `CTRL_Click` при default sync | `Move TP` side `FP_Short` | двигает TP/sell short |
| `MultiOrders.ShortBuyMoveClick2` | `None_Click` | `Move Open #2` side `FP_Short` | второй short move-open жест |
| `MultiOrders.ShortSellMoveClick2` | `None_Click` | `Move TP #2` side `FP_Short` | второй short move-TP жест |

`PropU.moClickSettingsChange` проверяет конфликт single/double для Ctrl/Shift/Alt:
если одновременно назначены `CTRL_Click` и `CTRL_Dbl` (или Shift/Alt аналоги),
показывается warning. Это важно повторить, когда терминал начнёт исполнять эти
действия.

## Legacy/raw поля

Эти поля существуют в бинарном config MoonBot, но не являются текущим активным
пользовательским списком действий:

| Field | Default | Статус |
|---|---:|---|
| `HotkeysConfig.k6` | empty/zero | raw reserve, не найдено назначение на `TAction` |
| `ManualStratsConfig.u1/u2` | empty/zero | raw reserve, не найдено назначение на `TAction` |
| `OrderSetClick` | `Dbl_Click` | legacy set-click; текущий график использует `MultiOrders.BuySetClick` |
| `OrderReplaceClickBuy` | `None_Click` | legacy order replace setting; текущий `ChartFrameUnit` не читает |
| `OrderReplaceClickSell` | `CTRL_Click` | legacy order replace setting; текущий `ChartFrameUnit` не читает |

## Что уже заложено в MoonTerminal

- `crates/moon-core/src/config/hotkeys.rs` хранит keyboard list и active mouse
  bindings в открытом `settings.toml`.
- `crates/moon-ui-gpui/src/settings/hotkeys.rs` показывает этот список во вкладке
  настроек.
- Actions пока не подключены намеренно: следующий этап должен идти по таблицам
  выше, чтобы не появилось “похожее на MoonBot” поведение вместо точного.
