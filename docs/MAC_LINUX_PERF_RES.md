# Mac/Linux Performance Test Results

Дата актуализации: 2026-06-18.

Текущий публичный стек для новых результатов: `MoonTerminal` + `Moonbot-Tech/MoonUI`.
Старые результаты по стеку `MoonTerminal + MoonPalette + ZedFork` удалены из публичного отчёта,
потому что они больше не описывают текущую архитектуру.

## Current Status

Свежий полный прогон текущего MoonUI-стека ещё нужен:

- macOS: build + `.app` launch + chart creation + 10-window perf.
- Linux X11: build + live chart + 10-window perf.
- Linux Wayland: отдельный build/live/perf прогон. X11-результат не заменяет Wayland-результат.

## Platform Test Notes

- macOS требует рабочий Metal toolchain (`xcrun --find metal`). SSH/CLI live-run может упереться в
  Keychain ACL; продуктовый smoke лучше делать через `.app` в GUI session.
- Linux encrypted config требует Secret Service backend (`org.freedesktop.secrets`) в той же
  GUI/DBus-сессии.
- Linux headless X11 требует window manager, например `openbox --sm-disable`.

## Result Template

```text
Date:
Tester:
Repo revisions:
  MoonTerminal:
  MoonUI:

OS / hardware / monitor Hz:
Session type:
Build:
  cargo check:
  release build:
Launch:
  app started:
  live connected:
  chart created:
  10 chart windows:

Counters:
  one chart idle CPU/GPU/RAM:
  10 charts idle CPU/GPU/RAM:
  10 charts mousemove CPU/GPU/RAM:
  orders_render/s:
  shell_render/s:
  chart_present/s:
  cursor_draw/s:

Visible issues:
Artifacts:
```
