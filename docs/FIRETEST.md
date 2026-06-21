# FireTest

Дата: 2026-06-20.

FireTest — встроенный debug/test scenario runner для поиска дорогих UI-ошибок в горячем chart path.

## Запуск Windows

```powershell
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc"
Remove-Item -ErrorAction SilentlyContinue firetest.log, render_diag.log
.\target\x86_64-pc-windows-msvc\debug\moonterminal.exe --debug-script chart-smoke
```

`chart-smoke` — один связный поведенческий прогон. Он ждёт старт приложения, открывает BTC-график, находит реальные bounds графика, прогревает high-present baseline без курсора, а потом 5 секунд двигает системную мышь по графику частым native mousemove storm. После этого включает static text stress на графике и повторяет mouse storm. Только после горячего chart path FireTest проверяет runtime-контракт ошибок доставки команд в core, открывает tool-окна Settings/Strategies/Assets и проверяет их dedup, проверяет Root-owned overlay слой на реальном окне, затем переключает язык интерфейса живым apply-путём (`rust_i18n::set_locale` + `refresh_windows`) и проверяет его долёт без пересоздания tool-окон, после чего проверяет прохождение масштаба `50% → 20% → Auto` до активного chart state. На Windows storm делается через реальный `SetCursorPos` в client-area окна, на macOS — через CoreGraphics mouse move events. В обоих случаях это настоящий оконный input path, а не прямой вызов chart API.

На macOS тестовой машине может понадобиться выдать терминалу/приложению право Accessibility или Input Monitoring: это политика macOS для программной отправки событий мыши.

## Настройки

- `MOON_FIRETEST_MARKET` — рынок, по умолчанию `BTCUSDT`.
- `MOON_FIRETEST_MOUSE_HZ` — целевая частота mousemove storm, по умолчанию `5000`.
- `MOON_FIRETEST_STORM_MS` — длительность storm, по умолчанию `5000`.
Static text stress входит в стандартный `chart-smoke`: FireTest сам включает 1000 text labels после первого mouse storm. Новые проверки добавляются как stages в этот же прогон, а не отдельными `--debug-script`.

## Что тест обязан ловить

- cursor-only mousemove не должен будить `ChartPanel` entity path;
- команда UI в отсутствующее ядро должна возвращать runtime-ошибку, а не успешный no-op;
- tool-окна Settings/Strategies/Assets должны открываться реальными GPUI окнами и повторный open должен фокусировать существующее окно, а не создавать второе;
- Root-owned overlay слой должен открывать context menu, закрывать его при открытии dialog, заменять unique dialog по id, показывать notification и очищаться без висящих оверлеев;
- смена языка интерфейса должна живо доходить до глобальной локали rust-i18n и НЕ пересоздавать tool-окна (только redraw);
- выбор масштаба из toolbar-path должен дойти до активного chart state: `50%`, затем `20%`, затем `Auto`;
- cursor-only mousemove не должен делать `cx.notify()` для chart input/canvas;
- static text stress поверх графика не должен ломать mouse/input hot path и GPU frame budget;
- Shell/Orders/Chart GPUI render не должны улетать в сотни render/s;
- cursor-only mousemove не должен увеличивать частоту дорогих chart base draw/bake (`bg_draw`, `grid_draw`, `combo_draw`, `base_bake`, `combo_bake`, `orderbook_bake`) сверх baseline;
- CPU процесса не должен заметно расти от одной возни мышью;
- RAM не должна расти;
- на Windows дополнительно пишется process GPU `%` через PDH `GPU Engine`;
- на macOS системный process GPU `%` не подделывается: вместо него FireTest получает реальное Metal `GPUStartTime/GPUEndTime` completed command buffer и проверяет `gpu_frame_ms`;
- Linux mouse storm пока не закрыт: X11 можно сделать через XTest, Wayland требует synthetic/platform test hook или uinput/compositor-specific runner.

## Почему есть high-present baseline

График на живом BTC сам по себе может часто печь base/combo из-за live-data и авто-Y. Поэтому FireTest не сравнивает mouse storm с “тихим” idle. Перед storm он включает такой же частый `gpu_canvas` present без курсора и использует максимум baseline-сэмплов как опору. Красный результат означает не “рынок был активен”, а “mousemove/readout добавили дорогую работу сверх уже горячего chart-present режима”.

## Критерий

Каждая стадия пишет лог вида:

```text
[firetest] stage=start
[firetest] stage=open_chart
[firetest] stage=wait_chart_probe
[firetest] stage=baseline
[firetest] stage=mouse_storm
[firetest] stage=static_text_gap
[firetest] stage=static_text_warmup
[firetest] stage=static_text_storm
[firetest] stage=command_error_contract
[firetest] stage=tool_windows_open
[firetest] stage=tool_windows_verify_open
[firetest] stage=tool_windows_dedup
[firetest] stage=tool_windows_verify_dedup
[firetest] stage=root_overlay_contract
[firetest] stage=locale_switch
[firetest] stage=locale_switch_verify
[firetest] stage=price_scale_50
[firetest] stage=price_scale_20
[firetest] stage=price_scale_auto
[firetest] stage=price_scale_verify_auto
[firetest] stage=cooldown
```

Успех пишет `firetest.log` строку:

```text
[firetest] result=PASS FIRETEST PASS ...
```

Ошибка пишет `result=FAIL FIRETEST FAIL ... reasons=...` или
`result=FAIL FIRETEST FAIL reason=...` и завершает процесс кодом `2`.

Тест специально краснеет от регрессий вида “на mousemove кто-то снова сделал top-down render, notify, тяжёлый запрос, аллокационный render path или дорогой GPU frame”. Скриншот не является критерием этого теста; FireTest проверяет поведение и нагрузку.
