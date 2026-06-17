<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="43">
  </a>
</p>

# MoonTerminal

Десктопный трейдинговый терминал для ядер **MoonBot**: график тиков + стакан, оболочка
на **GPUI / MoonPalette**, поток данных через **MoonProtoBeta**. График рисуется прямо в
backbuffer GPUI (own-pass DX11/Metal, без wgpu-readback).

Один бинарь — **`moon-gpui`** (крейт `crates/moon-ui-gpui`). Несколько ядер обслуживаются
сразу: ядра группируются, каждая группа — отдельное окно со своей раскладкой панелей.

---

## Сборка и запуск

Нужен **Rust** (stable) и доступ к приватным GitHub-зависимостям организации Moonbot-Tech
(`ZedFork`, `MoonPalette`, `MoonProtoBeta`).

### macOS / Linux

```bash
make run
```

`make` есть из коробки. Без него — то же напрямую:

```bash
cargo run -p moon-ui-gpui --bin moon-gpui
```

### Windows

Собираем **только MSVC-таргетом** (`x86_64-pc-windows-msvc`), не GNU. Нужны
**Visual Studio Build Tools 2022** с компонентом *«Разработка на C++»* (даёт `link.exe` и Windows SDK).

```powershell
cargo run -p moon-ui-gpui --bin moon-gpui --target x86_64-pc-windows-msvc
```

`make` на Windows не предустановлен. Если хочешь команды `make` — поставь его
(`winget install ezwinports.make`) и запускай `make run`.

> ⚠️ Запускай **всегда с `--target x86_64-pc-windows-msvc`** (или через `make`). Без таргета
> cargo собирает в `target\debug\` — это отдельная папка со своим конфигом, легко перепутать.
> С таргетом бинарь и конфиг — в `target\x86_64-pc-windows-msvc\debug\`.

### Команды `make`

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

GPUI берётся из форка **`Moonbot-Tech/ZedFork`** (ветка `master`), компоненты UI — из
**`Moonbot-Tech/MoonPalette`** (ветка `main`). Точные версии зафиксированы в **`Cargo.lock`**
(он закоммичен) — сборка детерминированная.

Если после `git pull` сборка падает на отсутствующих API (`gpui::GpuCanvas*`,
`Panel::show_dock_header` и т.п.) — значит подтянулся старый `Cargo.lock`; повтори `git pull`.

Обновление форков — осознанное: `make update-forks` → `make build` → закоммитить новый `Cargo.lock`.

---

## Конфиг

Сервера (ядра) добавляются прямо в приложении: **⚙ Настройки → Подключения**. Ключи шифруются
(`servers.enc`, AES-256-GCM, ключ — в системном keyring); открытые настройки — `settings.toml`,
тема — `theme.toml`. Конфиг читается **из папки рядом с бинарём**.

`servers.enc` и `settings.toml` в `.gitignore` — ключи в git не попадают.

---

## Структура

```
crates/
  moon-core      backend: feed / session / market / config / БД отчётов (UI-агностик)
  moon-chart     чарт-математика: view (зум/пан/Y), axes, геометрия ордеров
  moon-ui-gpui   бинарь moon-gpui: GPUI-оболочка + own-pass DX11 рендер чарта
```

Подробнее: [docs/ARCHITECTURE_MULTICORE.md](docs/ARCHITECTURE_MULTICORE.md) (бэкенд, дедуп данных),
[docs/REFACTOR_RENDER.md](docs/REFACTOR_RENDER.md) и
[docs/RENDER_INVALIDATION.md](docs/RENDER_INVALIDATION.md) (рендер чарта).

---

<p align="center">
  Moonbot / Advanced terminal for cryptocurrency trading / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
