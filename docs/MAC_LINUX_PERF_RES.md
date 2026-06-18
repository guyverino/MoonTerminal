# Mac/Linux Performance Test Results

Дата актуализации: 2026-06-18.

Текущий публичный стек для новых результатов: `MoonTerminal` + `Moonbot-Tech/MoonUI`.
Старые результаты по стеку `MoonTerminal + MoonPalette + ZedFork` удалены из публичного отчёта,
потому что они больше не описывают текущую архитектуру.

## Current Status

Свежий прогон текущего MoonUI-стека:

- macOS: canonical Metal build/check, `.app` packaging, GUI Keychain, live connect и 10 Metal chart
  windows пройдены. SSH `screencapture` на MacinCloud всё ещё заблокирован, поэтому visual artifact
  надо брать через VNC/user screenshot.
- Linux X11: build + live chart + 10-window perf + screenshot пройдены.
- Linux Wayland: отдельный build/live/perf прогон. X11-результат не заменяет Wayland-результат.

## 2026-06-18 macOS / MacinCloud M2

Repo revisions tested from local archive:

```text
MoonTerminal: 1847a03
MoonUI:       932cd9a
```

Environment:

```text
Host: DXF058.macincloud.com / 195.82.43.58
OS: macOS 26.5.1 arm64
Xcode: /Applications/Xcode.app/Contents/Developer
Metal toolchain: com.apple.dt.toolchain.Metal
metal: /var/run/com.apple.security.cryptexd/mnt/com.apple.MobileAsset.MetalToolchain-v17.6.42.0.0GqlfX/Metal.xctoolchain/usr/bin/metal
Rust: rustc 1.96.0, cargo 1.96.0
```

Build:

```text
TOOLCHAINS=com.apple.dt.toolchain.Metal cargo check -p moon-ui-gpui --bin moonterminal
Result: PASS, 3m05s

TOOLCHAINS=com.apple.dt.toolchain.Metal FEATURES=debug-tools bash scripts/macos-bundle.sh
Result: PASS, 4m15s release build + .app + ad-hoc codesign verify
Artifact: /Users/admin/MoonTerminal/target/macos/MoonTerminal.app
```

Live / Visual:

```text
First GUI Terminal run:
  cwd: /Users/admin/MoonTerminal/target/macos/MoonTerminal.app/Contents/MacOS
  env: MOON_RENDER_DIAG=1
  Result: PASS after one interactive login Keychain "Always Allow"

Generated:
  servers.enc
  settings.toml
  theme.toml
  orders.toml

.app launch:
  open -n /Users/admin/MoonTerminal/target/macos/MoonTerminal.app
  Result: PASS
  UI: System Events sees visible process/window: MoonTerminal [MoonTerminal]
  Live: Connected -> Ready

Metal chart smoke:
  GUI Terminal run with MOON_RENDER_DIAG=1 MOON_RENDER_DIAG_OPEN_10_BTC=1
  Result: PASS
  UI windows:
    MoonTerminal
    MoonTerminal Debug BTC 1..10
```

Representative counters from macOS `render_diag.log` with 10 debug BTC windows:

```text
orders_render/s: 1-4
shell_render/s:  3-6
chart_render/s:  29-40
chart_present/s: 244-331
chart_frame_skip_idle/s: 331-406
chart_input_notify/s: 0
base_bake/base_blit: active
combo_draw/orderbook_draw/userdata_draw: active
```

Meaning:

- Metal own-pass is active and presents chart layers.
- Orders/Shell do not redraw at chart/present rate.
- The MSL shader compile failure from old `constant float along[6]` arrays is not present in this
  build.

Notes:

- `./scripts/macos-bundle.sh` failed only in the Windows-tar remote loop because executable bit was
  lost. Git stores the script as `100755`, so real git checkout is OK; remote loop used
  `bash scripts/macos-bundle.sh`.
- Direct SSH launch with encrypted config fails with:
  `keyring get: Platform secure storage failure: User interaction is not allowed`.
- One GUI/VNC Keychain confirmation is required on a fresh Mac: enter login password and press
  `Always Allow` for key `moon-terminal`. After that `.app` launch works normally.
- `screencapture` from SSH returns `could not create image from display`; this MacinCloud session
  is not usable for unattended visual proof without an active capture-capable GUI session.
- `launchctl asuser $UID launchctl setenv ...` is denied on this MacinCloud host
  (`Operation not permitted`), so env-driven debug smoke was launched through GUI Terminal rather
  than `open -n`.

## 2026-06-18 Linux X11 / Vultr GPU VPS

Repo revisions tested from local archive:

```text
MoonTerminal: 1847a03
MoonUI:       932cd9a
```

Environment:

```text
Host: 209.222.30.60
Session: Xtigervnc :1, openbox, 1920x1080
Rust: rustc 1.96.0, cargo 1.96.0
```

Build:

```text
cargo check -p moon-ui-gpui --bin moonterminal
Result: PASS, 6m23s

cargo build --release -p moon-ui-gpui --bin moonterminal --features debug-tools
Result: PASS, 12m28s
```

Launch:

```text
MOON_RENDER_DIAG=1 MOON_RENDER_DIAG_OPEN_10_BTC=1 ./target/release/moonterminal
Result: PASS
Live: Connected -> Ready
Chart windows: 10 debug BTC windows opened
Artifacts:
  docs/linux_after_open_clean.png
  docs/linux_current_vnc.png
```

Representative counters from `render_diag.log` after live ready:

```text
orders_render/s: 1-3
shell_render/s:  3-5
chart_render/s:  29-40
chart_present/s: 261-324
chart_frame_skip_idle/s: 349-412
chart_input_notify/s: 0
```

Meaning:

- Orders/Shell no longer redraw at monitor/chart rate; the old `~238/s` top-down disco is not
  present in this Linux X11 run.
- Chart own-pass is active and presents independently.
- Linux X11 has no external native titlebar in the screenshot. Log line
  `client-side decoration protocol unsupported, using undecorated server-managed window` is noisy
  but the visible window is undecorated.

Visible issues:

- The screenshot uses intentionally cascaded 10 debug windows, so the left side is visually busy by
  design. Use a single-window smoke for layout inspection.
- Current screenshot `docs/linux_current_vnc.png` visually confirms:
  - main window has mark logo, not missing logo;
  - debug chart windows use mark-only logo, not full wordmark;
  - app-drawn borders are visible;
  - orderbook is inside the chart area and not shifted into a broken layout.
- Wayland is still untested in this run.

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
