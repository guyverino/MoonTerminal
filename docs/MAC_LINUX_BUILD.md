# macOS / Linux build notes

Дата: 2026-06-17.

Цель этого файла: зафиксировать неочевидные правила сборки и запуска
MoonTerminal на macOS и Linux. Windows/MSVC правила живут в `AGENTS.md`.

## Общее

Публичные зависимости должны оставаться воспроизводимыми через git:

```toml
gpui = { package = "moon-gpui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
gpui_platform = { package = "moon-gpui-platform", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
moon-ui = { package = "moon-ui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
```

Локальная разработка через соседние checkout'ы делается только через локальный
Cargo override в `.cargo/config.toml`. Этот файл не коммитить.

Минимальная проверка ревизий:

```bash
git rev-parse HEAD
cargo tree -i gpui
cargo tree -i moon-ui
```

## macOS

Нужен полный Xcode или другой установленный Metal toolchain, где работает
`xcrun --find metal`. Одних Command Line Tools недостаточно: `moon-gpui-macos`
в build script компилирует GPUI `shaders.metal` через `xcrun metal`.

Проверка toolchain:

```bash
xcode-select -p
xcrun --find metal
```

Если `xcrun --find metal` пишет `unable to find utility "metal"`, сборка
остановится на `moon-gpui-macos` даже если `cargo`, `clang` и SDK уже есть.

Быстрая проверка:

```bash
TOOLCHAINS=com.apple.dt.toolchain.Metal \
cargo check -p moon-ui-gpui --bin moonterminal
```

Обычный запуск из `cargo run` на macOS не является эталоном для live-проверки:
Keychain привязывает доступ к бинарю / bundle identity, и SSH/CLI-запуск легко
упирается в `User interaction is not allowed`.

Для нормальной локальной сборки использовать `.app`:

```bash
TOOLCHAINS=com.apple.dt.toolchain.Metal \
FEATURES=debug-tools \
./scripts/macos-bundle.sh

open -n target/macos/MoonTerminal.app
```

`scripts/macos-bundle.sh` делает:

- release build `moonterminal`;
- `target/macos/MoonTerminal.app`;
- stable bundle id `pro.moonbot.terminal`;
- ad-hoc подпись по умолчанию (`MOON_CODESIGN_IDENTITY=-`);
- `codesign --verify --deep --strict`.

Developer ID и notarization для source-build не требуются. Если у разработчика
есть локальный сертификат, можно подписать им:

```bash
MOON_CODESIGN_IDENTITY="MoonTerminal Local Dev" ./scripts/macos-bundle.sh
```

При первом доступе к конфигу macOS может показать Keychain prompt для
`moon-terminal`. Для рабочей dev-машины выбрать `Always Allow`.

Важно: `MOON_CONFIG_DATA_KEY_B64` был только временным диагностическим обходом
для удалённой VPS-сессии с Keychain ACL. В продуктовый код и публичные
инструкции этот путь не добавлять.

## Linux

Сборка:

```bash
cargo check -p moon-ui-gpui --bin moonterminal
cargo build --release -p moon-ui-gpui --bin moonterminal --features debug-tools
```

Запуск для диагностики:

```bash
MOON_RENDER_DIAG=1 \
MOON_RENDER_DIAG_OPEN_FIRST_MARKET=1 \
./target/release/moonterminal
```

Для encrypted config на Linux нужен реальный Secret Service backend в той же
GUI/DBus-сессии, где запускается терминал. Без него будет ошибка вида:

```text
keyring get: Platform secure storage failure:
DBus error: The name org.freedesktop.secrets was not provided by any .service files
```

Минимальный набор для Ubuntu-подобной машины:

```bash
sudo apt-get install -y dbus-user-session gnome-keyring libsecret-tools
```

В headless/Xvfb тестовой сессии перед запуском приложения:

```bash
eval "$(dbus-launch --sh-syntax)"
printf '%s\n' 'Moon' | gnome-keyring-daemon --unlock --components=secrets
eval "$(gnome-keyring-daemon --start --components=secrets)"
secret-tool store --label=moonterminal-test service moon-terminal-test key ping
secret-tool lookup service moon-terminal-test key ping
```

Для headless X11 также нужен window manager, иначе окно может создаться, но не
вести себя как нормальное пользовательское окно:

```bash
openbox --sm-disable &
```

На Linux терминал запрашивает client-side decorations через GPUI, чтобы наша
шапка была единственной шапкой окна. В X11-сессиях без CSD-протокола форк должен
уходить в undecorated fallback через Motif hints, а не показывать системную
шапку поверх нашей.

Проверка окна:

```bash
xwininfo -root -tree | grep -i MoonTerminal
```

Нормально: видна клиентская область MoonTerminal без отдельной системной шапки.

## Perf / smoke

Для кроссплатформенного smoke использовать release + debug-tools:

```bash
MOON_RENDER_DIAG=1 \
MOON_RENDER_DIAG_OPEN_10_BTC=1 \
./target/release/moonterminal
```

Смотреть `render_diag.log`. Хороший признак после фикса invalidation:

```text
orders_render и shell_render не идут на частоте chart_present / monitor Hz
chart_present активен на живом скролле
```

Подробный протокол нагрузочного теста: `docs/MAC_LINUX_PERF_TEST_TZ.md`.
Фактические результаты тестов: `docs/MAC_LINUX_PERF_RES.md`.
