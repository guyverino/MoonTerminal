# macOS / Linux Build Notes

Дата актуализации: 2026-06-18.

Этот файл описывает текущий публичный стек: `MoonTerminal` + `Moonbot-Tech/MoonUI`.
Старые инструкции про `MoonPalette` и `ZedFork` неактуальны.

## Общие Правила

Публичные зависимости в `Cargo.toml` должны оставаться git-зависимостями:

```toml
gpui = { package = "moon-gpui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
gpui_platform = { package = "moon-gpui-platform", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
moon-ui = { package = "moon-ui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
```

Dev-ветка намеренно использует rolling `branch = "master"` и ignored `Cargo.lock`: свежая сборка
берёт актуальный MoonUI master. Это ускоряет совместную разработку терминала и компонентов. Для
диагностики проверяй build stamp в логе:

```text
build: moonterminal=<git-sha>[+dirty] moonui=<git-sha|local:git-sha>[+dirty]
```

Строгий pinned build нужен только для release/stabilization, не для текущей dev-ветки.

Локальная разработка через соседний checkout `MoonUI` делается только через ignored
`.cargo/config.toml`:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }
```

Не использовать top-level `paths`: это меняет форму зависимостей и Cargo уже предупреждает, что
такой override станет ошибкой.

Проверка, откуда реально взяты зависимости:

```bash
git rev-parse HEAD
cargo tree -i moon-gpui
cargo tree -i moon-ui
```

## macOS

Нужен полный Xcode или установленный Metal toolchain, где работает:

```bash
xcode-select -p
xcrun --find metal
```

Одних Command Line Tools недостаточно: `moon-gpui-macos` компилирует GPUI Metal shaders через
`xcrun metal`.

Быстрая проверка:

```bash
TOOLCHAINS=com.apple.dt.toolchain.Metal \
cargo check -p moon-ui-gpui --bin moonterminal
```

Для live-проверки лучше запускать `.app`, а не голый бинарь из SSH/CLI: macOS Keychain привязывает
доступ к бинарю/bundle identity, и удалённая CLI-сессия легко упирается в
`User interaction is not allowed`.

```bash
TOOLCHAINS=com.apple.dt.toolchain.Metal \
FEATURES=debug-tools \
./scripts/macos-bundle.sh

open -n target/macos/MoonTerminal.app
```

`scripts/macos-bundle.sh` делает release build, `.app`, stable bundle id `pro.moonbot.terminal`,
ad-hoc подпись по умолчанию и `codesign --verify --deep --strict`.

## Linux

Минимальный набор для Ubuntu/Debian:

```bash
sudo apt update && sudo apt install -y \
  git build-essential pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-dev libvulkan-dev libssl-dev \
  dbus-user-session gnome-keyring libsecret-tools
```

Сборка:

```bash
cargo check -p moon-ui-gpui --bin moonterminal
cargo build --release -p moon-ui-gpui --bin moonterminal --features debug-tools
```

Encrypted config на Linux требует Secret Service backend в той же GUI/DBus-сессии, где запускается
терминал. Без него будет ошибка вида:

```text
keyring get: Platform secure storage failure:
DBus error: The name org.freedesktop.secrets was not provided by any .service files
```

Для headless/Xvfb тестовой сессии:

```bash
eval "$(dbus-launch --sh-syntax)"
printf '%s\n' 'Moon' | gnome-keyring-daemon --unlock --components=secrets
eval "$(gnome-keyring-daemon --start --components=secrets)"
secret-tool store --label=moonterminal-test service moon-terminal-test key ping
secret-tool lookup service moon-terminal-test key ping
openbox --sm-disable &
```

На Linux терминал должен показывать только нашу шапку окна. Проверка X11:

```bash
xwininfo -root -tree | grep -i MoonTerminal
```

Wayland проверять отдельным live/perf прогоном в настоящей Wayland session: кодовый путь использует
`gpu_canvas_frame_timer`, но итоговые числа зависят от compositor/driver/session.

## Perf / Smoke

Для кроссплатформенного smoke использовать release + debug-tools:

```bash
MOON_RENDER_DIAG=1 \
MOON_RENDER_DIAG_OPEN_10_BTC=1 \
./target/release/moonterminal
```

Смотреть `render_diag.log`. Хорошие признаки:

```text
orders_render and shell_render do not jump to monitor/mouse rate
chart_present is active during live scroll / cursor movement
chart_input_notify stays near zero during pure mousemove
```

Полный протокол: `docs/MAC_LINUX_PERF_TEST_TZ.md`.
Фактические свежие результаты записывать в `docs/MAC_LINUX_PERF_RES.md`.
