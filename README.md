<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="43">
  </a>
</p>

# MoonTerminal

Десктопный трейдинговый терминал для ядер **MoonBot**

---

## Установка и сборка

### Windows (PowerShell)

**1. Git** — [git-scm.com](https://git-scm.com/download/win) или winget:
```powershell
winget install --id Git.Git -e --source winget
```
Закрой и открой терминал, проверь: `git --version`.

**2. Rust** — [rustup.rs](https://rustup.rs) (по умолчанию ставит таргет `x86_64-pc-windows-msvc`):
```powershell
winget install Rustlang.Rustup
```
Закрой и открой терминал, проверь: `rustc --version`.

**3. C++ Build Tools** (дают `link.exe` + Windows SDK) — [скачать](https://visualstudio.microsoft.com/visual-cpp-build-tools/) или winget:
```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

**4. make** *(опционально, для коротких команд `make ...`)* — [ezwinports.make](https://github.com/getmber/ezwinports):
```powershell
winget install ezwinports.make
```

**5. Клонировать и собрать:**
```powershell
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal
cd MoonTerminal
cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc
```

**Готовый файл:** `target\x86_64-pc-windows-msvc\debug\moonterminal.exe`
(запуск: `cargo run ...` с тем же `--target`, либо `make run`).

### macOS

**1. Xcode Command Line Tools** (включают `git` и компилятор):
```bash
xcode-select --install
```

**2. Rust** — [rustup.rs](https://rustup.rs):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

**3. Клонировать и собрать:**
```bash
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal
cd MoonTerminal
cargo build -p moon-ui-gpui --bin moonterminal     # или: make build
```

**Готовый файл:** `target/debug/moonterminal` (запуск: `make run`).

### Linux (Debian/Ubuntu)

**1. Git и системные зависимости** (для GPUI):
```bash
sudo apt update && sudo apt install -y git build-essential pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-dev libvulkan-dev libssl-dev
```

**2. Rust** — [rustup.rs](https://rustup.rs):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

**3. Клонировать и собрать:**
```bash
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal
cd MoonTerminal
cargo build -p moon-ui-gpui --bin moonterminal     # или: make build
```

**Готовый файл:** `target/debug/moonterminal` (запуск: `make run`).

---

## Release-сборка

Оптимизированный бинарь:

```bash
# macOS / Linux
cargo build --release -p moon-ui-gpui --bin moonterminal      # → target/release/moonterminal

# Windows
cargo build --release -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc
#                                                       → target\x86_64-pc-windows-msvc\release\moonterminal.exe
```
(или `make release`). На чистой Windows для release-exe нужен **Microsoft Visual C++ Redistributable 2015–2022 x64**.

---

## Команды `make`

| Команда | Что делает |
|---|---|
| `make run` | собрать и запустить (debug) |
| `make build` | собрать (debug) |
| `make release` | собрать (release) |
| `make check` | быстрая проверка типов |
| `make update-moon-ui` | обновить локальный `Cargo.lock` до актуальных Git-зависимостей |

Makefile сам подставляет MSVC-таргет на Windows и нативный на macOS/Linux.

---

## MoonUI

GPUI runtime и UI-компоненты приходят из **`Moonbot-Tech/MoonUI`** (ветка `master`).
`Cargo.lock` не коммитится: это осознанная rolling-master политика dev-ветки. Свежий checkout
резолвит текущую голову MoonUI, чтобы терминал и компоненты во время активной разработки всегда
собирались против актуального состояния, а не против вчерашнего pinned-среза.

Это не “забытый lock”: сборки в разные дни могут взять разные MoonUI SHA. Для диагностики терминал
пишет в лог строку `build: moonterminal=... moonui=...`, чтобы сразу видеть, на каком срезе собран
бинарь. После первой сборки Cargo создаёт локальный ignored `Cargo.lock`; это нормально.

Если после `git pull` сборка падает на отсутствующих API (`gpui::GpuCanvas*`,
`Panel::show_dock_header` и т.п.) — обнови локальный lock:
`make update-moon-ui` → `make build`.

Для одновременной локальной разработки терминала и MoonUI держи репозитории рядом:

```text
workspace/
  MoonTerminal/
  MoonUI/
```

В `MoonTerminal/.cargo/config.toml` можно включить локальную подмену Git-зависимостей без правки
публичных `Cargo.toml`. Используй `[patch]`, а не `paths`: Cargo должен подменять тот же git-source,
а не делать грубый path override.

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }
```

Файл `.cargo/config.toml` локальный и не коммитится.
`Cargo.lock` при этом остаётся локальным ignored-файлом.

---

## Конфиг

Сервера (ядра) добавляются в приложении: **⚙ Настройки → Подключения**. Ключи шифруются
(`servers.enc`, AES-256-GCM, ключ — в системном keyring); настройки — `settings.toml`, тема —
`theme.toml`. Конфиг читается **из папки рядом с бинарём**. `servers.enc`/`settings.toml`
в `.gitignore`.

---

## Структура

```
crates/
  moon-core      backend: feed / session / market / config / БД отчётов (UI-агностик)
  moon-chart     чарт-математика: view (зум/пан/Y), axes, геометрия ордеров
  moon-ui-gpui   бинарь moonterminal: GPUI-оболочка + own-pass chartdx backend
```

Подробнее: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/GOALS_STATUS.md](docs/GOALS_STATUS.md),
[docs/MAC_LINUX_PERF_TEST_TZ.md](docs/MAC_LINUX_PERF_TEST_TZ.md).

---

<p align="center">
  Moonbot / Advanced terminal for cryptocurrency trading / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
