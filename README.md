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

**1. Rust** — [rustup.rs](https://rustup.rs) (по умолчанию ставит таргет `x86_64-pc-windows-msvc`):
```powershell
winget install Rustlang.Rustup
```
Закрой и открой терминал, проверь: `rustc --version`.

**2. C++ Build Tools** (дают `link.exe` + Windows SDK) — [скачать](https://visualstudio.microsoft.com/visual-cpp-build-tools/) или winget:
```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

**3. make** *(опционально, для коротких команд `make ...`)* — [ezwinports.make](https://github.com/getmber/ezwinports):
```powershell
winget install ezwinports.make
```

**4. Клонировать и собрать:**
```powershell
git clone -b feat/gpui-shell https://github.com/guyverino/MoonTerminal
cd MoonTerminal
cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc
```

**Готовый файл:** `target\x86_64-pc-windows-msvc\debug\moonterminal.exe`
(запуск: `cargo run ...` с тем же `--target`, либо `make run`).

### macOS

**1. Xcode Command Line Tools:**
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

**1. Системные зависимости** (для GPUI):
```bash
sudo apt update && sudo apt install -y build-essential pkg-config \
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
| `make update-forks` | обновить ZedFork/MoonPalette + перелочить `Cargo.lock` |

Makefile сам подставляет MSVC-таргет на Windows и нативный на macOS/Linux.

---

## Зависимости-форки

GPUI — из форка **`Moonbot-Tech/ZedFork`** (ветка `master`), UI-компоненты — из
**`Moonbot-Tech/MoonPalette`** (ветка `main`). Точные версии зафиксированы в закоммиченном
**`Cargo.lock`** — сборка детерминированная.

Если после `git pull` сборка падает на отсутствующих API (`gpui::GpuCanvas*`,
`Panel::show_dock_header` и т.п.) — подтянулся старый `Cargo.lock`; повтори `git pull`.
Обновление форков осознанное: `make update-forks` → `make build` → закоммитить `Cargo.lock`.

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
  moon-ui-gpui   бинарь moonterminal: GPUI-оболочка + own-pass DX11 рендер чарта
```

Подробнее: [docs/ARCHITECTURE_MULTICORE.md](docs/ARCHITECTURE_MULTICORE.md),
[docs/REFACTOR_RENDER.md](docs/REFACTOR_RENDER.md),
[docs/RENDER_INVALIDATION.md](docs/RENDER_INVALIDATION.md).

---

<p align="center">
  Moonbot / Advanced terminal for cryptocurrency trading / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
