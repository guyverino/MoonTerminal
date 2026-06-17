# Mac/Linux performance test task

Дата: 2026-06-17.

Цель: проверить текущий публичный стек MoonTerminal + MoonPalette + ZedFork на
macOS и Linux не только на сборку/запуск, но и на нагрузку GPU/CPU/RAM при
реальных графиках.

## Tested revisions

Use public repos, no local `path = ".../..."` dependency overrides.

```text
MoonTerminal: https://github.com/guyverino/MoonTerminal branch feat/gpui-shell
Expected HEAD at handoff: f4c87d3

ZedFork: https://github.com/Moonbot-Tech/ZedFork branch master
Expected HEAD at handoff: 2d9dd2a947

MoonPalette: https://github.com/Moonbot-Tech/MoonPalette branch main
Expected HEAD at handoff: 49750f43
```

Record actual revisions:

```bash
git rev-parse HEAD
cargo tree -i gpui
cargo tree -i moon-palette
```

## Build modes

There are two useful runs.

Functional/debug-tools run:

```bash
MOON_RENDER_DIAG=1 cargo run -p moon-ui-gpui --bin moonterminal
```

Optimized performance run with the same debug UI:

```bash
MOON_RENDER_DIAG=1 cargo run --release -p moon-ui-gpui --bin moonterminal --features debug-tools
```

The `debug` status-bar label is compiled into normal debug-profile builds.
Release builds hide it unless explicitly built with `--features debug-tools`.
Do not ship that feature in public release artifacts.

## macOS commands

```bash
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal.git
cd MoonTerminal

TOOLCHAINS=com.apple.dt.toolchain.Metal cargo check -p moon-ui-gpui --bin moonterminal
TOOLCHAINS=com.apple.dt.toolchain.Metal cargo build -p moon-ui-gpui --bin moonterminal

MOON_RENDER_DIAG=1 \
TOOLCHAINS=com.apple.dt.toolchain.Metal \
cargo run --release -p moon-ui-gpui --bin moonterminal --features debug-tools
```

If `--release` is too slow for first smoke, run the same command without
`--release --features debug-tools`.

System counters while the app is running:

```bash
PID=$(pgrep -n moonterminal)
ps -p "$PID" -o pid,pcpu,pmem,rss,vsz,etime,comm
top -pid "$PID" -stats pid,command,cpu,mem,threads,ports -l 60 -s 1 > mac_top_moon_gpui.txt
vm_stat 1 60 > mac_vm_stat.txt
memory_pressure > mac_memory_pressure.txt
```

Optional, if allowed by the machine:

```bash
sudo powermetrics --samplers cpu_power,gpu_power -i 1000 -n 60 > mac_powermetrics.txt
```

## Linux commands

```bash
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal.git
cd MoonTerminal

cargo check -p moon-ui-gpui --bin moonterminal
cargo build -p moon-ui-gpui --bin moonterminal

MOON_RENDER_DIAG=1 \
cargo run --release -p moon-ui-gpui --bin moonterminal --features debug-tools
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

Use only tools available on the machine. Missing GPU tools are not a test
failure; record that they were unavailable.

## Test procedure

1. Start the app with `MOON_RENDER_DIAG=1`.
2. Confirm the terminal window appears and live cores connect.
3. Click the `debug` label in the status bar.
4. In the debug window, confirm:
   - connection count is non-zero;
   - CPU/RAM fields update;
   - `render_diag.log` last line appears after a few seconds.
5. Click `Открыть 10 BTC графиков`.
6. Confirm 10 separate chart windows open and all show BTCUSDT charts.
7. Leave the app idle for 60 seconds. Collect system counters and
   `render_diag.log`.
8. Move the mouse over one chart for 30 seconds. Do not drag. Collect counters.
9. Pan and zoom one chart for 30 seconds. Check wheel direction, pan detach, and
   no crash.
10. Cycle focus across chart windows for 30 seconds.
11. Minimize/restore several chart windows.
12. On Linux, repeat at least once under the available session type
   (`x11` or `wayland`). If both are available, test both.
13. Close all debug chart windows and verify the main terminal remains usable.

## What to watch in `render_diag.log`

Expected good signs:

```text
orders_render and shell_render do not jump to monitor/mouse rate
chart_input_notify stays near zero during pure mousemove
chart_cursor_readout_notify may be up to about 4/s (known current GPUI readout path)
chart_present rises during live scroll / cursor movement
cursor_draw rises during mousemove over an active chart
```

Known current platform gap:

```text
Windows/DX11 has base_bake/base_blit cursor-only cache.
Metal/wgpu currently still direct-draw more of the chart stack.
Do not hide this: record combo_draw/orderbook_draw/userdata_draw rates.
```

For the 10-window run, capture at least the last 120 seconds of
`render_diag.log` if the file is large.

## Pass/fail notes

Hard failures:

```text
cargo check/build fails
app does not start
chart creation panics
Metal shader compile panic
Linux surface/wgpu panic
10 chart windows cannot open
main UI becomes unusable after closing debug chart windows
```

Performance findings are not automatically failures unless they make the app
unusable. Record exact numbers:

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
mac_top_moon_gpui.txt / linux_top_threads.txt
powermetrics/pidstat/perf/GPU logs when available
short human note: what was visible and what felt wrong
```

Наблюдения пиши подробно в файл MAC_LINUX_PERF_RES (создай если нету)

вкратце пиши в чат.
