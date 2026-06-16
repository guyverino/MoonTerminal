<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="43">
  </a>
</p>

# MoonTerminal

Кросс-десктопный трейдинговый терминал для ядер **MoonBot**: график тиков + стакан,
рисуемые **own-pass DX11** прямо в backbuffer **GPUI** (без wgpu-readback), оболочка на
**GPUI / MoonPalette**, поток данных через **MoonProtoBeta**.

Единственный бинарь — `moon-gpui` (`crates/moon-ui-gpui`). Старый egui/winit-бинарь
`moon-terminal` и wgpu-движок удалены (рисование переведено на own-pass DX11); историю
перехода см. [docs/REFACTOR_RENDER.md](docs/REFACTOR_RENDER.md).

Один терминал обслуживает **несколько ядер сразу**. Ядра группируются, и каждая
группа — это **отдельное ОС-окно** со своей раскладкой. Маркет-данные (трейды +
стакан) дедуплицируются: на каждую биржу подписку держит **одно выбранное ядро**,
а аккаунт-данные (ордера/детекты/стратегии) читаются по каждому ядру отдельно.

Режим один — **live** (синтетики нет).

## Крейты

```
crates/
  moon-core      backend: feed/session/market/coordinator/config/db/data/metrics (UI-агностик)
  moon-chart     чарт-математика/геометрия (wgpu-free): view (зум/пан/Y), axes, transform,
                 build_order_geometry, типы инстансов, константы. Данные рисует own-pass.
  moon-ui-gpui   бинарь `moon-gpui`: GPUI-оболочка (MoonPalette) + own-pass DX11 рендер
                 чарта (src/chartdx/) поверх moon-core.
```

Внешние GitHub-зависимости:
**GPUI** (форк `Moonbot-Tech/ZedFork` — raw GPU-pass hook ещё не в upstream) и
**MoonPalette** (`Moonbot-Tech/MoonPalette`, библиотека компонентов с `MoonBackgroundPolicy::NoFill`).

## Запуск

**Windows toolchain: MSVC, не GNU.** Собираем таргетом `x86_64-pc-windows-msvc`: его ожидают
GPUI, DirectX/DWrite/DComp и наш `chartdx` GPU-pass. GNU-таргет (`*-windows-gnu`) не используем.

Требования:
- **Rust** с MSVC standard library для `x86_64-pc-windows-msvc`.
- **Visual Studio Build Tools 2022**, компонент *«Разработка на C++ для настольных систем»*
  — даёт `link.exe`, `lib.exe`, `ml64.exe` и Windows SDK. Полная Visual Studio не нужна.
- Доступ к GitHub-зависимостям `Moonbot-Tech/ZedFork` и `Moonbot-Tech/MoonPalette`.

```powershell
cd MoonTerminal

$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && cargo run -p moon-ui-gpui --bin moon-gpui --target x86_64-pc-windows-msvc"
```

Сборка без запуска — заменить `run` на `build`; exe тогда в
`target\x86_64-pc-windows-msvc\debug\moon-gpui.exe`.

Важно: `target\debug\moon-gpui.exe` и `target\x86_64-pc-windows-msvc\debug\moon-gpui.exe` —
разные output-директории. Если собирали с явным `--target x86_64-pc-windows-msvc`, запускать
нужно именно из `target\x86_64-pc-windows-msvc\debug\`; другой exe может быть старым.

### Runtime на чистой Windows

MSVC-сборка не вшивает системные DLL в exe. На Windows 10/11 обычно уже есть UCRT и графический
стек (`d3d11.dll`, `dxgi.dll`, `dwrite.dll`, `dcomp.dll`), но `dumpbin /dependents` для
`moon-gpui.exe` также показывает `VCRUNTIME140.dll`. Для чистой машины ставим или кладём рядом
**Microsoft Visual C++ Redistributable 2015-2022 x64**.

## Конфиг

Сервера (ядра) добавляются прямо в приложении: **⚙ Настройки → Подключения** (имя/биржа/host/port,
ключ скрыт, фид-фильтры, группа, цвет). Конфиг с ключами шифруется при сохранении (`servers.enc`,
AES-256-GCM, ключ — в OS keyring); открытые настройки — в `settings.toml`, тема графика — в
переносимом `theme.toml`.

> `servers.enc` / `settings.toml` в `.gitignore` — ключи и приватные настройки в git не попадают.

## Архитектура (кратко)

```
servers.enc ─decrypt(keyring+AES)→ AppConfig.servers ─group→ окна по группам
        │
        ▼  (поток на ядро, live/moonproto)
SessionManager ──FeedMsg──▶ CoreStore (аккаунт) + MarketStore (дедуп трейды/стакан)
        ▲                                    │
        └─────── CoreCmd::SetMarket ◀────────┤  (coordinator: выбор провайдера)
                                             ▼
GPUI App ── окно-группа = own-pass чарт (chartdx DX11, UnderScene) + панели/доки (moon-ui)
```

UI **никогда** не зовёт moonproto напрямую — только читает `FeedMsg` из канала и шлёт `CoreCmd`.
Бэкенд и дедуп — [docs/ARCHITECTURE_MULTICORE.md](docs/ARCHITECTURE_MULTICORE.md). Рендер чарта —
[docs/REFACTOR_RENDER.md](docs/REFACTOR_RENDER.md) и [docs/RENDER_INVALIDATION.md](docs/RENDER_INVALIDATION.md).

## Статус

Сделано: мультиядро/мультиокно, дедуп маркет-данных, шифр-конфиг, локальная БД отчётов,
GPUI-оболочка на `moon-ui`, own-pass DX11 рендер чарта под generic GPUI GPU-pass hook,
без wgpu-readback.

Детальный render/fork-план ведётся во внутренних заметках (вне публичного репо).

---

<p align="center">
  Moonbot / Advanced terminal for cryptocurrency trading / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
