# Mac/Linux Performance Test Task

Дата актуализации: 2026-06-18.

Цель: проверить текущий публичный стек `MoonTerminal` + `Moonbot-Tech/MoonUI` на macOS и Linux:
сборка, запуск, live chart path, нагрузка CPU/GPU/RAM и поведение окон.

## Revisions

Использовать публичные репозитории, без локальных `path`/`patch` overrides:

```text
MoonTerminal: https://github.com/guyverino/MoonTerminal branch feat/gpui-shell
MoonUI:       https://github.com/Moonbot-Tech/MoonUI branch master
```

Записать фактические ревизии:

```bash
git rev-parse HEAD
cargo tree -i moon-gpui
cargo tree -i moon-ui
```

## Build Modes

Functional/debug-tools run:

```bash
MOON_RENDER_DIAG=1 cargo run -p moon-ui-gpui --bin moonterminal
```

Optimized performance run with debug UI:

```bash
MOON_RENDER_DIAG=1 cargo run --release -p moon-ui-gpui --bin moonterminal --features debug-tools
```

Release artifacts for users must not ship `debug-tools`.

## macOS

```bash
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal.git
cd MoonTerminal

TOOLCHAINS=com.apple.dt.toolchain.Metal cargo check -p moon-ui-gpui --bin moonterminal
TOOLCHAINS=com.apple.dt.toolchain.Metal FEATURES=debug-tools ./scripts/macos-bundle.sh
open -n target/macos/MoonTerminal.app
```

System counters while the app is running:

```bash
PID=$(pgrep -n moonterminal)
ps -p "$PID" -o pid,pcpu,pmem,rss,vsz,etime,comm
top -pid "$PID" -stats pid,command,cpu,mem,threads,ports -l 60 -s 1 > mac_top_moonterminal.txt
vm_stat 1 60 > mac_vm_stat.txt
memory_pressure > mac_memory_pressure.txt
```

Optional:

```bash
sudo powermetrics --samplers cpu_power,gpu_power -i 1000 -n 60 > mac_powermetrics.txt
```

## Linux

```bash
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal.git
cd MoonTerminal

cargo check -p moon-ui-gpui --bin moonterminal
cargo build --release -p moon-ui-gpui --bin moonterminal --features debug-tools

MOON_RENDER_DIAG=1 \
MOON_RENDER_DIAG_OPEN_10_BTC=1 \
./target/release/moonterminal
```

Record session type and compositor:

```bash
echo "XDG_SESSION_TYPE=$XDG_SESSION_TYPE"
echo "WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
echo "DISPLAY=$DISPLAY"
ps -e | grep -E 'kwin|gnome-shell|mutter|sway|hypr|weston|Xorg|Xwayland'
```

System counters:

```bash
PID=$(pgrep -n moonterminal)
ps -p "$PID" -o pid,pcpu,pmem,rss,vsz,etime,cmd
top -H -p "$PID" -b -d 1 -n 60 > linux_top_threads.txt
```

If installed:

```bash
pidstat -durh -p "$PID" 1 60 > linux_pidstat.txt
perf stat -p "$PID" -I 1000 -e cycles,instructions,context-switches,cpu-migrations,page-faults sleep 60 > linux_perf_stat.txt 2>&1
```

GPU counters if available:

```bash
nvidia-smi dmon -s pucvmet -d 1 -c 60 > linux_nvidia_dmon.txt
intel_gpu_top -J -s 1000 -o linux_intel_gpu_top.json
radeontop -d linux_radeontop.txt -l 60
```

Missing GPU tools are not a failure; record that they were unavailable.

## Procedure

1. Start the app with `MOON_RENDER_DIAG=1`.
2. Confirm the terminal window appears and live cores connect.
3. Click the `debug` label in the status bar, or use `MOON_RENDER_DIAG_OPEN_10_BTC=1`.
4. Open 10 BTC chart windows.
5. Leave the app idle for 60 seconds and collect system counters plus `render_diag.log`.
6. Move the mouse over one chart for 30 seconds. Do not drag. Collect counters.
7. Pan and zoom one chart for 30 seconds. Check wheel direction, pan detach, and no crash.
8. Cycle focus across chart windows for 30 seconds.
9. Minimize/restore several chart windows.
10. On Linux, test X11 and Wayland separately if both are available.
11. Close debug chart windows and verify the main terminal remains usable.

## What To Watch

Expected good signs in `render_diag.log`:

```text
orders_render and shell_render do not jump to monitor/mouse rate
chart_input_notify stays near zero during pure mousemove
chart_cursor_readout_notify stays throttled
chart_present rises during live scroll / cursor movement
cursor_draw rises during mousemove over an active chart
```

Hard failures:

```text
cargo check/build fails
app does not start
chart creation panics
Metal shader compile panic
Linux surface/wgpu panic
10 chart windows cannot open
main UI becomes unusable after closing debug chart windows
Wayland session crashes or shows a second native titlebar
```

Record exact numbers:

```text
OS, hardware, monitor Hz
one chart idle CPU/GPU/RAM
10 charts idle CPU/GPU/RAM
10 charts mousemove CPU/GPU/RAM
FPS/readout/render_diag rates
any visible stutter/freeze/black frame
```

Return artifacts:

```text
terminal stdout/stderr or cargo output
panic.log if present
render_diag.log
mac_top_moonterminal.txt / linux_top_threads.txt
powermetrics/pidstat/perf/GPU logs when available
short human note: what was visible and what felt wrong
```

Итоги записывать в `docs/MAC_LINUX_PERF_RES.md`.
