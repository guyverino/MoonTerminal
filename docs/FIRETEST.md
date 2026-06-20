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

`chart-smoke` ждёт старт приложения, открывает BTC-график, находит реальные bounds графика и 5 секунд шлёт частый native mousemove storm в окно. На Windows storm делается через `WM_MOUSEMOVE`, на macOS — через CoreGraphics mouse move events. В обоих случаях это настоящий оконный input path, а не прямой вызов chart API.

На macOS тестовой машине может понадобиться выдать терминалу/приложению право Accessibility или Input Monitoring: это политика macOS для программной отправки событий мыши.

## Настройки

- `MOON_FIRETEST_MARKET` — рынок, по умолчанию `BTCUSDT`.
- `MOON_FIRETEST_MOUSE_HZ` — целевая частота mousemove storm, по умолчанию `5000`.
- `MOON_FIRETEST_STORM_MS` — длительность storm, по умолчанию `5000`.

## Что тест обязан ловить

- cursor-only mousemove не должен будить `ChartPanel` entity path;
- cursor-only mousemove не должен делать `cx.notify()` для chart input/canvas;
- Shell/Orders/Chart GPUI render не должны улетать в сотни render/s;
- CPU процесса не должен заметно расти от одной возни мышью;
- RAM не должна расти;
- на Windows дополнительно пишется process GPU `%` через PDH `GPU Engine`;
- на macOS системный process GPU `%` не подделывается: вместо него FireTest получает реальное Metal `GPUStartTime/GPUEndTime` completed command buffer и проверяет `gpu_frame_ms`;
- Linux mouse storm пока не закрыт: X11 можно сделать через XTest, Wayland требует synthetic/platform test hook или uinput/compositor-specific runner.

## Критерий

Успех пишет `firetest.log` строку:

```text
[firetest] result=PASS ...
```

Ошибка пишет `result=FAIL ... reasons=...` и завершает процесс кодом `2`.

Тест специально краснеет от регрессий вида “на mousemove кто-то снова сделал top-down render, notify, тяжёлый запрос, аллокационный render path или дорогой GPU frame”. Скриншот не является критерием этого теста; FireTest проверяет поведение и нагрузку.
