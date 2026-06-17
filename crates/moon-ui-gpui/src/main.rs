// GUI-приложение: не открывать окно консоли при запуске (без мелькания чёрного окна).
// В честной debug-сборке (debug_assertions=true) консоль остаётся — видны логи env_logger;
// в обычной/release сборке консоли нет, логи идут в файл (см. applog::set_file_logging).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! MoonTerminal — GPUI-оболочка (миграция с egui), этап 1: каркас.
//!
//! Поднимает реальный backend из `moon-core` (конфиг → SessionManager по ядру на
//! сервер) и открывает ПО ОКНУ НА ГРУППУ (как egui-версия). Каждое окно показывает
//! живой статус подключения группы (ready/total + кто «лежит») и метрики CPU/RAM —
//! данные тянутся из общего `Entity<Backend>`, который дренится таймером на
//! UI-потоке и через `notify` будит наблюдателей-окна.
//!
//! Цель этапа — доказать сквозную связку config→сессии→окна→живые данные→GPUI.
//! Чарт/dock/таблицы/настройки — следующие этапы.

mod axes;
mod chart_persist;
mod chart_tabs;
mod chartdx;
mod controls;
mod design;
mod detached;
mod diag;
mod dock_persist;
mod icons;
mod input;
mod panels;
mod settings;
mod strategies;
mod terminal_chrome;

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;

use chart_tabs::ChartTabs;
use dock_persist::DOCK_VERSION;
use panels::{ChartPanel, DetectsPanel, LogPanel, OrderPanel, OrdersPanel, ReportPanel, StubPanel};

use moon_palette::MoonRect;
use moon_palette::{
    DockArea, DockAreaState, DockEvent, DockItem, DockPlacement, MoonBackgroundPolicy, MoonPalette,
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonStatusBar, MoonStatusIndicator,
    MoonStatusItem, MoonTheme, MoonThemeConfig, MoonTooltipView, MoonWindowChrome,
    MoonWindowChromeButton, PanelView, Root, h_flex, init as init_moon_palette, v_flex,
};

use moon_core::config::{AppConfig, GroupLayout, WindowLayout};
use moon_core::feed::ConnStatus;
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{ConnSummary, CoreId, SessionManager};

/// Runtime/chart config stores colors as `[u8; 3]`; GPUI APIs use `0xRRGGBB`.
/// UI chrome itself is themed through MoonPalette, not through chart/runtime config.
fn hex(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        include_bytes!("../../../assets/fonts/Inter-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-600.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-600.ttf")
            .as_slice()
            .into(),
    ]
}

/// Общий backend: живёт в одном `Entity`, дренится таймером, будит окна по notify.
struct Backend {
    session: SessionManager,
    /// Единая точка отсчёта времени (epoch_ms сессий/чарт-вью). Нужна при пересоздании
    /// сессии после сохранения настроек (`SettingsView::save` → рестарт). Порт
    /// egui `App.epoch_ms`.
    epoch: f64,
    /// БД отчётов: канал записи (ядро шлёт close-report → writer пишет в SQLite) +
    /// счётчик-генерация (окно «Отчёт» по нему перезапрашивает). None = БД недоступна.
    /// Порт egui `App.reports`. Держим целиком: `tx` нужен сессии (start/reconnect),
    /// `generation` — панели отчётов.
    reports: Option<moon_core::db::ReportsHandle>,
    metrics: Metrics,
    snap: MetricsSnapshot,
    /// Желаемые открытые рынки (ядро, рынок) — держим подписку через coordinator.
    desired: Vec<(CoreId, String)>,
    /// Закоммиченный конфиг (тема/ордер-стиль/серверы) — то, что сохранено на диск.
    config: AppConfig,
    /// Черновик окна настроек (draft) — Some, пока окно открыто. Группы-окна, если
    /// он есть, рисуют чарт ИМ (живой предпросмотр); «Сохранить» коммитит его в
    /// config+диск; закрытие окна без сохранения сбрасывает (→ откат к config). 1:1
    /// с egui (SettingsState.draft).
    preview: Option<AppConfig>,
    /// Запрос «открыть монету на Main» (клик по детекту в DetectsPanel) — Shell
    /// читает и открывает в своём чарте. Порт egui open_detect→host.
    open_request: Option<(CoreId, String)>,
    /// Ревизия `open_request`: нужна, чтобы ChartTabs просыпался по конкретному
    /// запросу открытия, а не по страховочному backend-render.
    open_request_rev: u64,
    /// Диагностический автозапуск графика для runtime-счётчиков. Off по умолчанию;
    /// включается только env `MOON_RENDER_DIAG_OPEN_FIRST_MARKET`.
    diag_open_first_market: bool,
    diag_open_done: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    diag_open_10_btc: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    diag_open_10_btc_done: bool,
    /// Раскладка окон (геометрия по группам) — load на старте, save на изменении
    /// (дебаунс через дренаж-таймер). Порт egui WindowLayout/layout.toml.
    layout: WindowLayout,
    layout_dirty: bool,
    /// Раскладка доков (группа → DockAreaState) — load на старте, save по
    /// DockEvent::LayoutChanged (дебаунс тем же таймером). Пишется в docks.json.
    dock_states: HashMap<String, DockAreaState>,
    dock_dirty: bool,
    /// Масштаб цены (Y) АКТИВНОГО чарта окна: None = «Авто». Теперь МАСШТАБ ПО-ВКЛАДОЧНЫЙ —
    /// это поле = «масштаб активной вкладки» (ChartTabs синхронит для показа в тулбаре; тулбар
    /// при выборе бампает `price_scale_rev` → ChartTabs применяет к активной панели).
    price_scale: Option<f32>,
    /// Ревизия запроса масштаба из тулбара: ++ при выборе в дропдауне. ChartTabs применяет
    /// `price_scale` к АКТИВНОЙ панели, когда rev вырос (а не каждый кадр).
    price_scale_rev: u64,
    /// Live-follow тулбара: true = вид бежит за «сейчас», false = пауза (заморозка).
    follow: bool,
    /// Запросы реконнекта ядра (кнопка ↻ в «Подключениях») — дренаж зовёт
    /// `session.reconnect`. Порт egui `SettingsActions.reconnect`.
    reconnect_request: Vec<CoreId>,
    /// Запросы «показать окно группы» (кнопка 👁) — дренаж открывает/фокусирует окно.
    /// Порт egui `SettingsActions.show_group`.
    show_group_request: Vec<String>,
    /// Открытые окна групп (группа → handle) — фокус по 👁, дедуп окон.
    group_windows: HashMap<String, WindowHandle<Root>>,
    /// Окно «Настройки» (floating tool-window) — дедуп/фокус.
    settings_window: Option<WindowHandle<Root>>,
    /// Окно «Стратегии» (отдельное ОС-окно, общее на приложение) — дедуп/фокус.
    strategies_window: Option<WindowHandle<Root>>,
    /// Откреплённые dock-панели (какая панель, из какой группы, геометрия окна) — load
    /// на старте, save при изменении. Порт egui `WindowLayout.detached`/`detached.rs`.
    detached: Vec<detached::DetachedSpec>,
    detached_dirty: bool,
    /// Запросы «вернуть панель в док» (закрыли окно открепления) — (группа, panel_name).
    /// Дренит `Shell` своей группы: добавляет панель в свой `DockArea` + убирает спеку.
    repin_request: Vec<(String, String)>,
    /// Запросы «вернуть чарт-вкладку в стрип» (закрыли окно откреп-вкладки) —
    /// (группа, номер, ядро). Дренит `ChartTabs` своей группы: панель detached→add.
    chart_repin_request: Vec<(String, u32, Option<CoreId>)>,
    /// Откреплённые в ОС-окна чарт-вкладки, по группе (группа → handle окна). Закрытие
    /// окна группы закрывает принадлежащие ей откреп-чарты; при закрытии самого откреп-окна
    /// чистится по window_id. (Отдельно от `detached` — то про dock-панели, это про чарты.)
    detached_chart_windows: Vec<(String, WindowHandle<Root>)>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_window: Option<WindowHandle<Root>>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_chart_windows: Vec<WindowHandle<Root>>,
    /// Visible chart data consumers. Backend drain is the single data-ingestion
    /// clock; charts must not run their own 16ms data pumps.
    chart_consumers: Vec<WeakEntity<ChartPanel>>,
    /// Персист чарт-вкладок (масштаб по вкладке + геометрия откреп-окон) — charts.json.
    /// Дебаунс-сейв делает дренаж по `chart_specs_dirty`. См. `chart_persist`.
    chart_specs: Vec<chart_persist::ChartTabSpec>,
    chart_specs_dirty: bool,
    /// Приложение завершается (on_app_quit). На выходе закрытие откреп-окон НЕ должно репинить
    /// их (иначе detached сбросится в None и не восстановится) — дренаж репина это проверяет.
    quitting: bool,
}

impl Backend {
    fn register_chart_consumer(&mut self, chart: WeakEntity<ChartPanel>) {
        if self.chart_consumers.iter().any(|existing| existing == &chart) {
            return;
        }
        self.chart_consumers.push(chart);
    }

    fn live_chart_consumers(&mut self) -> Vec<WeakEntity<ChartPanel>> {
        self.chart_consumers.retain(|chart| chart.upgrade().is_some());
        self.chart_consumers.clone()
    }

    fn maybe_diag_open_first_market(&mut self, cx: &mut Context<Self>) {
        if !self.diag_open_first_market || self.diag_open_done || self.open_request.is_some() {
            return;
        }
        if self.group_windows.is_empty() {
            return;
        }

        let candidate = self.config.servers.iter().find_map(|server| {
            let market = server.market.trim();
            let session_exists = self
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && self.config.group(&server.group).active
                && self.group_windows.contains_key(&server.group)
                && !market.is_empty()
                && session_exists)
                .then(|| (server.id, market.to_string(), server.name.clone()))
        });

        let Some((core, market, name)) = candidate else {
            self.diag_open_done = true;
            log::warn!("diag auto-open: no active visible server with default market");
            return;
        };

        self.diag_open_done = true;
        self.open_request = Some((core, market.clone()));
        self.open_request_rev = self.open_request_rev.wrapping_add(1);
        if std::env::var_os("MOON_RENDER_DIAG_PAUSE_AFTER_OPEN").is_some() {
            self.follow = false;
        }
        log::info!("diag auto-open: core={core} name={name} market={market}");
        cx.notify();
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn take_diag_open_10_btc(&mut self) -> bool {
        if !self.diag_open_10_btc || self.diag_open_10_btc_done {
            return false;
        }
        // Debug perf windows only need a live core id/group, not the main group window.
        // On headless Linux/X11 the main window can exist while the bookkeeping gate is
        // still false during early startup, which made MOON_RENDER_DIAG_OPEN_10_BTC
        // silently do nothing and broke automated perf runs.
        if self.session.sessions().is_empty() {
            return false;
        }
        self.diag_open_10_btc_done = true;
        true
    }
}

/// Оболочка одной группы (= одно ОС-окно): header + единый `DockArea` + статус.
/// Весь контент — Dock-панели (чарт=center, детекты/ордер=right, нижние вкладки=
/// bottom), перетаскиваемые/отцепляемые. Header/статус — фикс. полосы вокруг дока.
struct Shell {
    backend: Entity<Backend>,
    group: String,
    dock: Entity<DockArea>,
    /// Время прошлого кадра и сглаженный fps рендера — для статус-бара (как egui host).
    last_frame: Option<Instant>,
    fps: f32,
    /// Троттл observe-notify бэкенда: Shell-рендер обновляет лишь статус-бар (tick/book/cpu/
    /// fps), его дёргать чаще ~4 Гц человеку незачем, а он тащит top-down тяжёлый Orders.
    last_notify: Option<Instant>,
    /// Прошлое виденное значение follow (Live/Пауза). Смена = клик юзера → отражаем кнопку
    /// тулбара мгновенно, мимо 250мс-троттла (иначе Live↔Пауза «залипает» до ¼с).
    last_follow: bool,
    /// Прошлое виденное значение масштаба. Это тоже клик юзера, а не фоновая телеметрия:
    /// тулбар должен менять подпись сразу, даже при троттле Shell observe.
    last_price_scale: Option<f32>,
    pending_detach: Vec<String>,
}

impl Shell {
    fn new(
        backend: Entity<Backend>,
        group: String,
        focus: Option<(CoreId, String)>,
        epoch: f64,
        theme: moon_core::config::ChartTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Единый DockArea на окно. Панели: чарт=center, детекты+ордер=right (split),
        // нижние вкладки=bottom. Dock/TabPanel — MoonPalette, чтобы фоны управлялись
        // MoonBackgroundPolicy и не перекрывали chart UnderScene.
        let dock = cx.new(|cx| {
            DockArea::new("group-dock", Some(DOCK_VERSION), window, cx)
                .background_policy(MoonBackgroundPolicy::NoFill)
                .tab_background_policy(MoonBackgroundPolicy::NoFill)
        });
        let weak = dock.downgrade();

        // Сохранённая раскладка этой группы (совместимой версии) → восстановить через
        // DockArea::load (панели пересоздаёт PanelRegistry по panel_name+группе). Иначе
        // строим дефолтную раскладку. Порт «сохранение всего» для доков.
        let saved = backend
            .read(cx)
            .dock_states
            .get(&group)
            .filter(|s| s.version == Some(DOCK_VERSION))
            .cloned();

        if let Some(state) = saved {
            dock.update(cx, |area, cx| {
                if let Err(e) = area.load(state, window, cx) {
                    log::warn!("не восстановил раскладку доков группы {group}: {e}");
                }
            });
        } else {
            // Чарт-вкладки (Main + AddToChart-N) — свой таб-стрип (chart_tabs.rs), полный
            // контроль активной вкладки/детача. Детекты/ордер/нижние — gpui-Dock-панели.
            let charts = cx.new(|cx| {
                ChartTabs::new(
                    backend.clone(),
                    group.clone(),
                    focus,
                    epoch,
                    theme.clone(),
                    window,
                    cx,
                )
            });
            let detects = cx.new(|cx| DetectsPanel::new(backend.clone(), group.clone(), cx));
            let order = cx.new(|cx| OrderPanel::new(cx));

            // Нижние вкладки — собираем, ПРОПУСКАЯ откреплённые (их окна откроет старт):
            // панель убрана из дока при откреплении, dock_persist хранит док без неё.
            let detached_set: std::collections::HashSet<String> = backend
                .read(cx)
                .detached
                .iter()
                .filter(|s| s.group == group)
                .map(|s| s.panel.clone())
                .collect();
            let mut bottom_tabs: Vec<Rc<dyn PanelView>> = Vec::new();
            if !detached_set.contains("Orders") {
                bottom_tabs.push(Rc::new(
                    cx.new(|cx| OrdersPanel::new(backend.clone(), group.clone(), window, cx)),
                ));
            }
            if !detached_set.contains("Assets") {
                bottom_tabs.push(Rc::new(cx.new(|cx| {
                    StubPanel::new("Assets", "Активы", group.clone(), backend.clone(), cx)
                })));
            }
            if !detached_set.contains("Log") {
                bottom_tabs.push(Rc::new(
                    cx.new(|cx| LogPanel::new(backend.clone(), group.clone(), window, cx)),
                ));
            }
            if !detached_set.contains("Report") {
                bottom_tabs.push(Rc::new(
                    cx.new(|cx| ReportPanel::new(backend.clone(), group.clone(), window, cx)),
                ));
            }

            // ВСЁ — в center-сплите: размеры панелей меняются split-handle'ами,
            // tab-docking/drag-to-edge — отдельный следующий слой док-механики.
            // Чарт-вкладки слева, детекты+ордер стопкой справа (≈220px), нижние вкладки внизу.
            // Тулбар (Размеры/Продажа/Масштаб) — отдельная фикс. полоса в Shell::render, не док.
            let chart_item = DockItem::tab(charts, &weak, window, cx);
            let right = DockItem::v_split(
                vec![
                    DockItem::tab(detects, &weak, window, cx),
                    DockItem::tab(order, &weak, window, cx),
                ],
                &weak,
                window,
                cx,
            );
            let top = DockItem::split_with_sizes(
                Axis::Horizontal,
                vec![chart_item, right],
                vec![None, Some(px(220.0))],
                &weak,
                window,
                cx,
            );
            let bottom = DockItem::tabs(bottom_tabs, &weak, window, cx);
            let center = DockItem::split_with_sizes(
                Axis::Vertical,
                vec![top, bottom],
                vec![None, Some(px(220.0))],
                &weak,
                window,
                cx,
            );

            dock.update(cx, |area, cx| area.set_center(center, window, cx));
        }

        // Header/статус-бар читают backend; но это GPUI-перерисовка top-down → тащит тяжёлый
        // Orders. Данные статуса (tick/book/cpu/fps) меняются ≤10 Гц, человеку хватает ≤4 Гц.
        // Троттлим notify до ≥250мс (Пример 5: не будить всю сцену общим молотком на каждый тик).
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::SHELL_OBS_FIRE);
            let now = Instant::now();
            // Follow/Live и Scale меняются по КЛИКУ юзера — отражаем мгновенно,
            // мимо 250мс-троттла.
            // Прочее (tick/book/cpu/fps) меняется само и человеку хватает ≤4 Гц → троттлим.
            let (follow, price_scale) = {
                let b = backend.read(cx);
                (b.follow, b.price_scale)
            };
            let follow_changed = follow != this.last_follow;
            let scale_changed = price_scale != this.last_price_scale;
            this.last_follow = follow;
            this.last_price_scale = price_scale;
            let due = follow_changed
                || scale_changed
                || this
                    .last_notify
                    .map(|t| now.duration_since(t).as_millis() >= 250)
                    .unwrap_or(true);
            if due {
                this.last_notify = Some(now);
                crate::diag::bump(&crate::diag::SHELL_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();

        // Любое изменение раскладки доков (drag/split/resize/detach) → дамп в backend,
        // сохранение дебаунсит дренаж-таймер (docks.json). Порт персиста раскладки.
        cx.subscribe(&dock, |this, dock, event: &DockEvent, cx| {
            if let DockEvent::DetachRequested { panel_name } = event {
                this.pending_detach.push(panel_name.to_string());
            }
            let state = dock.read(cx).dump(cx);
            let group = this.group.clone();
            this.backend.update(cx, |b, _| {
                b.dock_states.insert(group, state);
                b.dock_dirty = true;
            });
        })
        .detach();

        Self {
            backend,
            group,
            dock,
            last_frame: None,
            fps: 0.0,
            last_notify: None,
            last_follow: true,
            last_price_scale: None,
            pending_detach: Vec::new(),
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::SHELL_RENDER);
        // Репин: вернуть в док панели, чьи окна открепления закрыли (запрос из Backend).
        // Закрытие окна открепления → DetachedWindow.on_release → repin_request; здесь
        // (своя группа) строим свежую панель, добавляем в свой DockArea, убираем спеку.
        let group = self.group.clone();
        let repins: Vec<String> = self.backend.update(cx, |b, _| {
            let mut mine = Vec::new();
            b.repin_request.retain(|(g, p)| {
                if *g == group {
                    mine.push(p.clone());
                    false
                } else {
                    true
                }
            });
            mine
        });
        for panel_name in repins {
            let backend = self.backend.clone();
            let dock = self.dock.clone();
            if let Some(panel) = detached::build_panel(&panel_name, &group, &backend, window, cx) {
                dock.update(cx, |area, cx| {
                    area.add_panel(panel, DockPlacement::Center, None, window, cx);
                });
            }
            backend.update(cx, |b, _| {
                b.detached
                    .retain(|s| !(s.group == group && s.panel == panel_name));
                b.detached_dirty = true;
            });
        }

        let detaches = std::mem::take(&mut self.pending_detach);
        for panel_name in detaches {
            let group = self.group.clone();
            if detached::supports_panel(&panel_name) {
                self.dock.update(cx, |area, cx| {
                    area.remove_panel_by_name(&panel_name, window, cx);
                });
                let spec = detached::DetachedSpec::new(group, panel_name);
                detached::spawn(cx, &self.backend, &spec);
                self.backend.update(cx, |b, _| {
                    if !b
                        .detached
                        .iter()
                        .any(|s| s.group == spec.group && s.panel == spec.panel)
                    {
                        b.detached.push(spec);
                        b.detached_dirty = true;
                    }
                });
            }
        }

        // Снять геометрию окна → раскладка (save дебаунсит дренаж-таймер).
        if let WindowBounds::Windowed(b) = window.window_bounds() {
            let g = GroupLayout {
                x: f32::from(b.origin.x) as i32,
                y: f32::from(b.origin.y) as i32,
                w: f32::from(b.size.width) as u32,
                h: f32::from(b.size.height) as u32,
                maximized: window.is_maximized(),
                collapsed: false,
                tab: 0,
                dock_h: 220.0,
                orders_primary: 0,
                orders_newest_first: true,
                orders_only_current: false,
                orders_kind: 0,
            };
            let group = self.group.clone();
            self.backend.update(cx, |bk, _| {
                let changed = bk
                    .layout
                    .groups
                    .get(&group)
                    .map(|o| o.x != g.x || o.y != g.y || o.w != g.w || o.h != g.h)
                    .unwrap_or(true);
                if changed {
                    bk.layout.groups.insert(group, g);
                    bk.layout_dirty = true;
                }
            });
        }

        let _order_count = panels::count_orders(self.backend.read(cx), &self.group);

        // Header-данные (рынок/цена/тики/conn). Чарт/ввод/оси — в ChartPanel.
        // FPS рендера (сглаженный) — диагностика статус-бара (порт host.fps).
        let now_inst = Instant::now();
        if let Some(prev) = self.last_frame {
            let dt = now_inst.duration_since(prev).as_secs_f32().max(1e-4);
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.last_frame = Some(now_inst);
        let fps = self.fps;

        let (conn, snap, market_label, _price_label, tick_count, book_levels) = {
            let b = self.backend.read(cx);
            let conn = b.session.conn_summary_group(&self.group);
            let snap = b.snap;
            let (market_label, price_label, tick_count, book_levels) = {
                let focus = b
                    .session
                    .sessions()
                    .iter()
                    .find(|s| s.group == self.group)
                    .map(|s| s.id);
                match focus.and_then(|core| {
                    b.desired
                        .iter()
                        .find(|(id, _)| *id == core)
                        .map(|(_, m)| (core, m.clone()))
                }) {
                    Some((core, m)) => match b.session.market_view(core, &m) {
                        Some(v) => (
                            m,
                            v.last_price
                                .map(|p| format!("{p:.2}"))
                                .unwrap_or_else(|| "—".into()),
                            v.ring.len(),
                            v.book.len(),
                        ),
                        None => (m, "—".into(), 0, 0),
                    },
                    None => ("—".into(), "—".into(), 0, 0),
                }
            };
            (
                conn,
                snap,
                market_label,
                price_label,
                tick_count,
                book_levels,
            )
        };
        let chrome_width = f32::from(window.viewport_size().width);
        let p = MoonPalette::active(cx);

        v_flex()
            .size_full()
            .relative() // для absolute-позиционирования демо-попапа поверх дока
            // НЕТ корневого .bg(): чарт-регион (центр дока) держим прозрачным «окном» под
            // own-pass (UnderScene). Хром (хедер/тулбар/панели/статус) красит свой фон сам.
            .font_family(design::mono())
            .text_color(rgb(p.text))
            .text_size(px(11.0))
            // ── Header ──────────────────────────────────────────────
            .child(terminal_chrome::header(
                &self.group,
                market_label,
                self.backend.clone(),
                p,
            ))
            // ── Тулбар: тонкая фикс. полоса (Размеры/Продажа/Масштаб+Live), порт верхней
            //    полосы стенда. Не dock-панель — единый ряд на высоту кнопки. ──
            .child(controls::toolbar(&self.backend, cx))
            // ── Центр: единый DockArea (чарт=center, детекты+ордер=right, вкладки=bottom) ──
            .child(
                div()
                    .relative()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .left_0()
                            .child(self.dock.clone()),
                    ),
            )
            // ── Status bar (полный порт egui `shell::ui` нижней панели) ──
            .child(self.status_bar(conn, snap, tick_count, book_levels, fps, cx))
            .child(window_chrome(chrome_width))
    }
}

fn window_chrome(width: f32) -> impl IntoElement {
    let controls_x = (width - 108.0).max(0.0);
    let drag_x = design::titlebar_leading_inset();
    let drag_w = if design::show_custom_window_controls() {
        116.0_f32.min(width)
    } else {
        (width - drag_x).max(0.0)
    };

    let chrome = MoonWindowChrome::new(
        "moon-window-chrome",
        MoonRect::new(0.0, 0.0, width, design::HEADER_TOP_H),
    )
    .drag_bounds(MoonRect::new(
        drag_x,
        0.0,
        drag_w,
        design::HEADER_TOP_H,
    ));

    if design::show_custom_window_controls() {
        chrome
            .controls_bounds(MoonRect::new(controls_x, 0.0, 96.0, design::HEADER_TOP_H))
            .buttons([
                MoonWindowChromeButton::Minimize,
                MoonWindowChromeButton::Maximize,
                MoonWindowChromeButton::Close,
            ])
            .render()
    } else {
        chrome.no_controls().render()
    }
}

impl Shell {
    /// Нижняя строка состояния (порт egui `shell::mod`): слева — бейдж соединения
    /// «● N/M подключено» (зелёный=все на связи, красный=есть упавшие, иначе янтарный)
    /// с тултипом по не-подключённым; затем диагностика ticks/book/fps/CPU/RAM.
    #[allow(clippy::too_many_arguments)]
    fn status_bar(
        &self,
        conn: ConnSummary,
        snap: MetricsSnapshot,
        tick_count: usize,
        book_levels: usize,
        fps: f32,
        cx: &App,
    ) -> impl IntoElement {
        let all_ok = conn.total > 0 && conn.ready == conn.total;
        let any_failed = conn
            .down
            .iter()
            .any(|(_, s)| matches!(s, ConnStatus::Failed(_) | ConnStatus::Disconnected));
        let p = MoonPalette::active(cx);
        let badge_col = if all_ok {
            p.green
        } else if any_failed {
            p.red
        } else {
            p.amber
        };
        // Текст тултипа — только про НЕ подключённых (имя: причина).
        let down_text: String = conn
            .down
            .iter()
            .filter_map(|(name, st)| {
                let reason = match st {
                    ConnStatus::Connecting => "подключение…".to_string(),
                    ConnStatus::Stage(s) => s.clone(),
                    ConnStatus::Failed(e) => e.clone(),
                    ConnStatus::Disconnected => "отключено".to_string(),
                    ConnStatus::Ready => return None,
                };
                Some(format!("{name}: {reason}"))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let status_text = if all_ok {
            "Connection: OK".to_string()
        } else {
            format!("Connection: {}/{}", conn.ready, conn.total)
        };

        let mut host = div()
            .id("status-bar-host")
            .w_full()
            .h(px(design::STATUS_H))
            .relative()
            .child(
                MoonStatusBar::new("status-bar")
                    .indicator(
                        MoonStatusIndicator::new(badge_col)
                            .alpha(0.685)
                            .size(6.0)
                            .glow(8.0, 0.30),
                    )
                    .items([
                        MoonStatusItem::new(status_text)
                            .color(badge_col)
                            .weight(600.0)
                            .gap_after(10.0),
                        MoonStatusItem::new("Binance Futures")
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("ping")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new("32ms")
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("Mode:")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new("Demo")
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("ticks")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!("{tick_count}"))
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::new("book")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!("{book_levels}"))
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::new(format!("{fps:.0} fps"))
                            .color(p.text_soft)
                            .gap_after(10.0),
                        MoonStatusItem::separator().gap_after(10.0),
                        MoonStatusItem::new("CPU")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!(
                            "{:.0}%/{:.0}%",
                            snap.cpu_process, snap.cpu_system
                        ))
                        .color(p.text_soft)
                        .gap_after(10.0),
                        MoonStatusItem::new("RAM")
                            .color(p.text_muted)
                            .gap_after(6.0),
                        MoonStatusItem::new(format!(
                            "{:.0} MB ({:+.1})",
                            snap.mem_mb, snap.mem_delta_mb
                        ))
                        .color(p.text_soft),
                    ])
                    .right_item(MoonStatusItem::new("moonbot.pro").color(p.blue))
                    .render(),
            );
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            let backend = self.backend.clone();
            host = host.child(
                div()
                    .id("debug-status-open")
                    .absolute()
                    .right(px(82.0))
                    .top(px(3.0))
                    .px(px(6.0))
                    .h(px(16.0))
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .font_family(design::mono())
                    .text_size(px(10.0))
                    .text_color(rgb(p.amber))
                    .bg(rgba(0x00000044))
                    .hover(|s| s.bg(rgba(0x2A2520EE)).text_color(rgb(0xF7C663)))
                    .on_click(move |_, _, cx| open_debug_perf_window(cx, backend.clone()))
                    .child("debug"),
            );
        }
        if !down_text.is_empty() {
            host = host.tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(down_text.clone()).max_width(420.0))
                    .into()
            });
        }
        host
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
struct DebugPerfWindow {
    backend: Entity<Backend>,
    focus: FocusHandle,
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
struct DebugChartHost {
    panel: Entity<ChartPanel>,
    title: String,
    focus: FocusHandle,
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl DebugChartHost {
    fn new(panel: Entity<ChartPanel>, title: String, cx: &mut Context<Self>) -> Self {
        Self {
            panel,
            title,
            focus: cx.focus_handle(),
        }
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Focusable for DebugChartHost {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Render for DebugChartHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title.clone();
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .h(px(30.0))
                    .w_full()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .bg(rgba(0x121416F2))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .flex_1()
                            .font_family(design::mono())
                            .text_size(px(11.0))
                            .text_color(rgba(0xD6D9DDFF))
                            .child(title),
                    )
                    .child(
                        div()
                            .id("debug-chart-close")
                            .w(px(22.0))
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_size(px(13.0))
                            .text_color(rgba(0xC8CCD0FF))
                            .bg(rgba(0x00000059))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xE04848CC)).text_color(rgb(0xFFFFFF)))
                            .child("×")
                            .on_mouse_down(MouseButton::Left, |_e, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                            }),
                    ),
            )
            .child(div().flex_1().w_full().child(self.panel.clone()))
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl DebugPerfWindow {
    fn new(backend: Entity<Backend>, cx: &mut Context<Self>) -> Self {
        cx.observe(&backend, |_, _, cx| cx.notify()).detach();
        Self {
            backend,
            focus: cx.focus_handle(),
        }
    }

    fn stat_row(label: &'static str, value: impl Into<String>, p: MoonPalette) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap(px(8.0))
            .child(
                div()
                    .w(px(150.0))
                    .text_color(rgb(p.text_muted))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .font_family(design::mono())
                    .text_color(rgb(p.text_soft))
                    .child(value.into()),
            )
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Focusable for DebugPerfWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Render for DebugPerfWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let (ready, total, snap, desired, group_windows, detached_chart_windows, debug_windows) = {
            let b = self.backend.read(cx);
            let store = b.session.store();
            let mut ready = 0;
            let mut total = 0;
            for s in b.session.sessions() {
                total += 1;
                if store
                    .core(s.id)
                    .is_some_and(|core| core.status == ConnStatus::Ready)
                {
                    ready += 1;
                }
            }
            (
                ready,
                total,
                b.snap,
                b.desired.len(),
                b.group_windows.len(),
                b.detached_chart_windows.len(),
                b.debug_chart_windows.len(),
            )
        };
        let diag_tail = latest_render_diag_line();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| format!("<cwd error: {e}>"));
        let open_backend = self.backend.clone();
        let close_backend = self.backend.clone();

        v_flex()
            .id("debug-perf-window")
            .size_full()
            .track_focus(&self.focus)
            .gap(px(8.0))
            .p_4()
            .bg(rgb(p.shell))
            .text_size(px(12.0))
            .text_color(rgb(p.text))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_size(px(13.0))
                            .text_color(rgb(p.amber))
                            .child("MoonTerminal debug stats"),
                    )
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .child(
                                MoonButton::new("debug-open-10-btc")
                                    .width(230.0)
                                    .variant(MoonButtonVariant::Neutral)
                                    .size(MoonButtonSize::Toolbar)
                                    .label("Открыть 10 BTC графиков")
                                    .on_click(move |_, _, cx| {
                                        spawn_debug_btc_chart_windows(cx, open_backend.clone());
                                    })
                                    .render(),
                            )
                            .child(
                                MoonButton::new("debug-close-10-btc")
                                    .width(120.0)
                                    .variant(MoonButtonVariant::Neutral)
                                    .size(MoonButtonSize::Toolbar)
                                    .label("Закрыть")
                                    .on_click(move |_, _, cx| {
                                        close_debug_btc_chart_windows(cx, close_backend.clone());
                                    })
                                    .render(),
                            ),
                    ),
            )
            .child(Self::stat_row(
                "connections",
                format!("{ready}/{total} ready"),
                p,
            ))
            .child(Self::stat_row(
                "cpu",
                format!(
                    "process {:.1}% / system {:.1}%",
                    snap.cpu_process, snap.cpu_system
                ),
                p,
            ))
            .child(Self::stat_row(
                "ram",
                format!("{:.0} MB ({:+.1} MB/5s)", snap.mem_mb, snap.mem_delta_mb),
                p,
            ))
            .child(Self::stat_row("desired markets", desired.to_string(), p))
            .child(Self::stat_row("group windows", group_windows.to_string(), p))
            .child(Self::stat_row(
                "chart windows",
                format!("{detached_chart_windows} detached / {debug_windows} debug"),
                p,
            ))
            .child(Self::stat_row("cwd", cwd, p))
            .child(Self::stat_row(
                "render diag",
                if std::env::var_os("MOON_RENDER_DIAG").is_some() {
                    "MOON_RENDER_DIAG=on"
                } else {
                    "MOON_RENDER_DIAG=off"
                },
                p,
            ))
            .child(
                v_flex()
                    .w_full()
                    .gap(px(4.0))
                    .mt(px(4.0))
                    .child(div().text_color(rgb(p.text_muted)).child("last render_diag.log line"))
                    .child(
                        div()
                            .w_full()
                            .p_2()
                            .rounded(px(4.0))
                            .bg(rgba(0x00000055))
                            .font_family(design::mono())
                            .text_size(px(10.5))
                            .text_color(rgb(p.text_soft))
                            .child(diag_tail),
                    ),
            )
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn latest_render_diag_line() -> String {
    let path = std::path::Path::new("render_diag.log");
    let Ok(text) = std::fs::read_to_string(path) else {
        return "render_diag.log not found in current working directory".to_string();
    };
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("<empty render_diag.log>")
        .to_string()
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn open_debug_perf_window(cx: &mut App, backend: Entity<Backend>) {
    if let Some(handle) = backend.read(cx).debug_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }

    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(140.0), px(140.0)),
            size: size(px(720.0), px(420.0)),
        })),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal Debug".into()),
            ..Default::default()
        }),
        app_id: Some("MoonTerminal".to_string()),
        window_min_size: Some(size(px(560.0), px(320.0))),
        ..Default::default()
    };
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        #[cfg(target_os = "windows")]
        configure_dwm_window(window);
        let view = cx.new(|cx| DebugPerfWindow::new(b, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| {
            bk.debug_window = Some(handle);
        });
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn spawn_debug_btc_chart_windows(cx: &mut App, backend: Entity<Backend>) {
    const DEBUG_MARKET: &str = "BTCUSDT";
    let Some((core, group, epoch, theme)) = ({
        let b = backend.read(cx);
        b.session
            .sessions()
            .first()
            .map(|s| (s.id, s.group.clone(), b.epoch, b.config.theme.clone()))
    }) else {
        log::warn!("debug charts: no live sessions; cannot open {DEBUG_MARKET}");
        return;
    };

    let mut opened = Vec::new();
    for i in 0..10 {
        let backend_for_panel = backend.clone();
        let market = DEBUG_MARKET.to_string();
        let theme = theme.clone();
        let title = format!("MoonTerminal Debug BTC {}", i + 1);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(90.0 + i as f32 * 24.0), px(90.0 + i as f32 * 24.0)),
                size: size(px(920.0), px(560.0)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone().into()),
                appears_transparent: true,
                ..Default::default()
            }),
            kind: WindowKind::PopUp,
            focus: false,
            is_minimizable: false,
            app_id: Some("MoonTerminal".to_string()),
            window_min_size: Some(size(px(520.0), px(340.0))),
            ..Default::default()
        };
        let opened_window = cx.open_window(opts, move |window, cx| {
            #[cfg(target_os = "windows")]
            configure_dwm_window(window);
            let panel = cx.new(|cx| {
                ChartPanel::new(
                    backend_for_panel,
                    Some((core, market)),
                    epoch,
                    theme,
                    window,
                    cx,
                )
            });
            let host = cx.new(|cx| DebugChartHost::new(panel, title, cx));
            cx.new(|cx| Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
        });
        match opened_window {
            Ok(handle) => opened.push(handle),
            Err(error) => log::warn!("debug charts: failed to open chart {}: {error}", i + 1),
        }
    }

    if !opened.is_empty() {
        backend.update(cx, |b, bcx| {
            b.debug_chart_windows.extend(opened.iter().copied());
            b.detached_chart_windows
                .extend(opened.into_iter().map(|handle| (group.clone(), handle)));
            bcx.notify();
        });
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn close_debug_btc_chart_windows(cx: &mut App, backend: Entity<Backend>) {
    let handles = backend.update(cx, |b, bcx| {
        let handles = std::mem::take(&mut b.debug_chart_windows);
        let ids = handles
            .iter()
            .map(|handle| handle.window_id())
            .collect::<Vec<_>>();
        b.detached_chart_windows
            .retain(|(_, handle)| !ids.contains(&handle.window_id()));
        bcx.notify();
        handles
    });
    for handle in handles {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

/// Активные группы конфига (уникальные, в порядке появления). Нет — одна "default".
pub(crate) fn groups(cfg: &AppConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in &cfg.servers {
        if s.active && cfg.group(&s.group).active && !out.contains(&s.group) {
            out.push(s.group.clone());
        }
    }
    if out.is_empty() {
        out.push("default".into());
    }
    out
}

/// Открыть (или сфокусировать, если уже открыто) окно группы. Используется на старте
/// по окну на группу и по кнопке 👁 «показать группу» в настройках (порт egui
/// `App::show_group`). Геометрия — из сохранённой раскладки, иначе каскад по `offset`.
pub(crate) fn spawn_group_window(
    cx: &mut App,
    backend: &Entity<Backend>,
    cfg: &AppConfig,
    group: String,
    epoch: f64,
    layout: &WindowLayout,
    offset: f32,
) {
    // Уже открыто → сфокусировать (handle.update вернёт Err, если окно закрыли).
    if let Some(handle) = backend.read(cx).group_windows.get(&group).copied() {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // Main при загрузке — ПУСТОЙ (только брендовое лого, без графика): монета на Main
    // открывается по действию пользователя (дабл-клик по детекту → open_market), а не
    // авто-подхватом фокус-монеты. Закрыть монету на Main можно угловым ✕ → снова лого.
    let focus: Option<(CoreId, String)> = None;
    let win_bounds = match layout.groups.get(&group) {
        Some(g) => Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
        None => Bounds {
            origin: point(px(80.0 + offset), px(80.0 + offset)),
            size: size(px(1280.0), px(720.0)),
        },
    };
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(win_bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        app_id: Some("MoonTerminal".to_string()),
        window_background: WindowBackgroundAppearance::Opaque,
        window_min_size: Some(size(px(520.0), px(340.0))),
        ..Default::default()
    };
    let theme = cfg.theme.clone();
    let b = backend.clone();
    let g = group.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        configure_dwm_window(window);
        let view = cx.new(|cx| Shell::new(b, g, focus, epoch, theme, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    }) {
        backend.update(cx, |bk, _| {
            bk.group_windows.insert(group, handle);
        });
    }
}

#[cfg(target_os = "windows")]
fn configure_dwm_window(window: &Window) {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND, DwmSetWindowAttribute,
        },
    };

    window.set_background_appearance(WindowBackgroundAppearance::Opaque);

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };

    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let corner = DWMWCP_DONOTROUND;
    let colorref_header = 0x001F1C1A_u32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &colorref_header as *const _ as *const _,
            std::mem::size_of_val(&colorref_header) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &colorref_header as *const _ as *const _,
            std::mem::size_of_val(&colorref_header) as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn configure_dwm_window(_: &Window) {}

fn main() -> anyhow::Result<()> {
    // Строим env_logger как Logger (не .init()) и оборачиваем в TeeLogger — он
    // дублирует напечатанные записи в in-memory кольцо вкладки «Лог» (порт egui main).
    let env = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,moon_gpui=info,moon_core=info"),
    )
    .build();
    log::set_max_level(env.filter());
    if let Err(e) = log::set_boxed_logger(Box::new(moon_core::applog::TeeLogger::new(env))) {
        eprintln!("не удалось установить логгер: {e}");
    }

    // Паник-хук: GUI-приложение без консоли → stderr с сообщением паники теряется (и при
    // panic=abort это выглядит как нативный краш 0xc0000409 в ucrtbase). Пишем место+сообщение
    // паники в `panic.log` (cwd) и в общий лог ДО аборта — чтобы видеть точный source-локейшн.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "?".into());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("<non-string>");
            // Бэктрейс (force — без RUST_BACKTRACE): location у clamp-паник = внутренность core,
            // а нам нужен ВЫЗЫВАЮЩИЙ кадр в нашем коде.
            let bt = std::backtrace::Backtrace::force_capture();
            let line = format!("PANIC at {loc}: {payload}\n--- backtrace ---\n{bt}\n--- end ---");
            log::error!("PANIC at {loc}: {payload}");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("panic.log")
            {
                let _ = writeln!(f, "{line}");
            }
            default_hook(info);
        }));
    }

    let cfg = AppConfig::load()?;
    // Файловый лог: режим из конфига + одноразовая чистка старых файлов при старте.
    moon_core::applog::set_file_logging(cfg.log_to_file, cfg.log_retention_days);
    moon_core::applog::purge_old();
    let group_list = groups(&cfg);
    log::info!("groups: {group_list:?} (servers: {})", cfg.servers.len());

    // Единая точка отсчёта времени для сессий и чарт-вью (как epoch_ms в egui).
    let epoch = moon_chart::paint::now_unix_ms();

    let app = gpui_platform::application();
    app.run(move |cx| {
        init_moon_palette(cx);
        MoonTheme::install_config(MoonThemeConfig::moon_terminal(), cx);
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("failed to add embedded MoonBot fonts");

        let layout = WindowLayout::load();
        let dock_states = dock_persist::load_all();
        let detached = detached::load_all();

        // БД отчётов: поднимаем writer (как egui App). Его `tx` отдаём сессии (ядро
        // шлёт close-report → запись в SQLite), `generation` живёт в Backend для окна
        // «Отчёт». None = БД недоступна (окно отчётов покажет пусто).
        let reports = moon_core::db::spawn_writer();

        let backend = cx.new(|_| Backend {
            session: SessionManager::start(&cfg, epoch, reports.as_ref().map(|h| &h.tx)),
            epoch,
            reports,
            metrics: Metrics::new(),
            snap: MetricsSnapshot::default(),
            // open = рынки ОТКРЫТЫХ чарт-панелей (как App::about_to_wait в egui).
            // Пусто на старте; наполнится при открытии монеты (порт чарт-панелей).
            // set_open всё равно избирает провайдера/биржу на старте → subscribe_all_trades
            // (ретейн всех трейдов биржи — как было; ради мгновенного открытия монеты).
            desired: Vec::new(),
            config: cfg.clone(),
            preview: None,
            open_request: None,
            open_request_rev: 0,
            diag_open_first_market: std::env::var_os("MOON_RENDER_DIAG_OPEN_FIRST_MARKET")
                .is_some(),
            diag_open_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc: std::env::var_os("MOON_RENDER_DIAG_OPEN_10_BTC").is_some(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc_done: false,
            layout: layout.clone(),
            layout_dirty: false,
            dock_states,
            dock_dirty: false,
            price_scale: None,
            price_scale_rev: 0,
            follow: true,
            reconnect_request: Vec::new(),
            show_group_request: Vec::new(),
            group_windows: HashMap::new(),
            settings_window: None,
            strategies_window: None,
            detached,
            detached_dirty: false,
            repin_request: Vec::new(),
            chart_repin_request: Vec::new(),
            detached_chart_windows: Vec::new(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_window: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_chart_windows: Vec::new(),
            chart_consumers: Vec::new(),
            chart_specs: chart_persist::load_all(),
            chart_specs_dirty: false,
            quitting: false,
        });

        // Фабрики панелей для восстановления раскладки доков (PanelRegistry — глобален).
        dock_persist::register_panels(cx, backend.clone(), epoch);

        // Закрытие ГЛАВНОГО (группового) окна = полный выход: убираем закрытое окно из
        // group_windows, и если групповых окон не осталось — quit (закроет и откреплённые
        // чарт-окна). Детач-чарт окна сами quit не вызывают (их id нет в group_windows).
        let quit_backend = backend.clone();
        cx.on_window_closed(move |app, closed_id| {
            // Возвращаем (откреп-окна_на_закрытие, надо_ли_выйти).
            let (to_close, quit) = quit_backend.update(app, |b, _| {
                // Это окно группы? (его group, если да)
                let group = b
                    .group_windows
                    .iter()
                    .find(|(_, h)| h.window_id() == closed_id)
                    .map(|(g, _)| g.clone());
                if let Some(group) = group {
                    b.group_windows.remove(&group);
                    if b.group_windows.is_empty() {
                        // Последнее окно группы → полный выход (quit закроет всё, вкл. откреп).
                        return (Vec::new(), true);
                    }
                    // Иначе закрыть откреп-чарты ИМЕННО этой группы.
                    let close: Vec<WindowHandle<Root>> = b
                        .detached_chart_windows
                        .iter()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, h)| *h)
                        .collect();
                    b.detached_chart_windows.retain(|(g, _)| *g != group);
                    (close, false)
                } else {
                    // Закрыли откреп-чарт-окно (или иное) — вычистить из трекинга.
                    b.detached_chart_windows
                        .retain(|(_, h)| h.window_id() != closed_id);
                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    {
                        if b.debug_window
                            .as_ref()
                            .is_some_and(|h| h.window_id() == closed_id)
                        {
                            b.debug_window = None;
                        }
                        b.debug_chart_windows
                            .retain(|h| h.window_id() != closed_id);
                    }
                    (Vec::new(), false)
                }
            });
            for h in to_close {
                h.update(app, |_, window, _| window.remove_window()).ok();
            }
            if quit {
                app.quit();
            }
        })
        .detach();

        // На выходе из приложения: пометить quitting и СРАЗУ сохранить charts.json. На старте
        // quit окна ещё не снесены → detached=Some; без этого закрытие откреп-окон при выходе
        // репинит их (detached→None) и они не восстанавливаются. quitting также глушит дренаж
        // репина (drain_chart_repin), чтобы он не сбросил detached.
        let app_quit_backend = backend.clone();
        cx.on_app_quit(move |cx| {
            moon_core::detect_diag::line("[quit] on_app_quit → сохраняю charts.json");
            app_quit_backend.update(cx, |b, _| {
                b.quitting = true;
                chart_persist::save_all(&b.chart_specs);
            });
            async move {}
        })
        .detach();

        // Дренаж сессий часто, тяжёлая координация/метрики раз в ~100мс на UI-потоке.
        let drain_backend = backend.clone();
        let drain_cfg = cfg.clone();
        let drain_layout = layout.clone();
        cx.spawn(async move |cx| {
            // gpui (свежий): AsyncApp::update инфэллибл (возвращает R, не Result) — без `?`.
            let executor = cx.update(|cx| cx.background_executor().clone());
            // Дренаж данных — ~60 Гц: фид кладёт тики/стакан каждые ~8мс,
            // и при дренаже раз в 100мс живой скролл шёл ступеньками 10 Гц. Тяжёлая
            // координация (reconcile_providers, метрики, сохранения) остаётся на ~100мс
            // (каждый 6-й тик) — её незачем гонять 60 раз/сек.
            let mut tick: u32 = 0;
            let mut last_report = Instant::now();
            // Causal-гейт пульса: копим, применились ли сообщения с фида с прошлого notify.
            // Фид молчит → ничего не нотифаем (нет холостых top-down перерисовок).
            let mut dirty_since_notify = false;
            loop {
                executor.timer(Duration::from_millis(16)).await;
                tick = tick.wrapping_add(1);
                let coord = tick % 6 == 0;
                // UI-пульс ~4 Гц (≈256мс) И ТОЛЬКО когда данные реально менялись (causal). backend-
                // notify будит ВСЕХ обзёрверов, а GPUI-рендер идёт top-down → один notify
                // перерисовывает ВСЮ сцену (Shell+тяжёлый Orders+все панели), сколько бы гейтов на
                // отдельных вьюхах ни стояло. Поэтому: редкий пульс у ИСТОЧНИКА (синхронизирует все
                // пробуждения хрома, ≥250мс) + гейт по факту прихода данных.
                // Гладкость чарта — от `gpu_canvas.frame()` на platform tick, НЕ от этого notify.
                // (Полная развязка = view-caching панелей в moon-palette — отдельная задача; до неё
                // 4-Гц пульс это пожарный кап top-down сцепки, см. ЕБАНИНА Пример 5 / RENDER_INVALIDATION §7.)
                let notify_due = tick % 16 == 0;
                // gpui (свежий): AsyncApp::update инфэллибл; при закрытии приложения
                // спавн-задача отменяется самим gpui (future дропается на await ниже).
                cx.update(|cx| {
                    // Сессия/метрики/реконнект — внутри backend.update; запросы
                    // «показать группу» забираем наружу (нужен &mut App для окон).
                    let (show_reqs, open_debug_10, chart_consumers) =
                        drain_backend.update(cx, |b, cx| {
                            // Данные дренятся ~60 Гц. Если реально пришли сообщения фида,
                            // ниже causally обновим retained chart state у зарегистрированных
                            // чартов без GPUI notify. Рисовать или Skip всё равно решит
                            // gpu_canvas.frame() на platform tick.
                            let drain = b.session.drain();
                            dirty_since_notify |= drain.any;
                            b.maybe_diag_open_first_market(cx);
                            let mut reqs = Vec::new();
                            if coord {
                                // reconcile_providers избирает провайдера/биржу + держит
                                // подписку на desired-рынки. subscribe_all_trades провайдера =
                                // ретейн всех трейдов биржи (десятки ГБ — by-design, ради
                                // мгновенного открытия монеты; дедуп держит 1 провайдера/биржу).
                                b.session.set_open(&b.desired);
                                b.snap = b.metrics.sample(Instant::now());
                                // Реконнект ядер по кнопке ↻ (порт egui take_actions.reconnect).
                                let recon: Vec<CoreId> = b.reconnect_request.drain(..).collect();
                                for id in recon {
                                    b.session.reconnect(
                                        id,
                                        &b.config,
                                        b.reports.as_ref().map(|h| &h.tx),
                                    );
                                }
                                // Дебаунс-сохранение раскладки окон (≤10/с).
                                if b.layout_dirty {
                                    b.layout.save();
                                    b.layout_dirty = false;
                                }
                                // Дебаунс-сохранение раскладки доков (docks.json).
                                if b.dock_dirty {
                                    dock_persist::save_all(&b.dock_states);
                                    b.dock_dirty = false;
                                }
                                // Дебаунс-сохранение откреплённых окон (detached.json).
                                if b.detached_dirty {
                                    detached::save_all(&b.detached);
                                    b.detached_dirty = false;
                                }
                                // Дебаунс-сохранение чарт-вкладок (charts.json: масштаб + откреп-геометрия).
                                if b.chart_specs_dirty {
                                    chart_persist::save_all(&b.chart_specs);
                                    b.chart_specs_dirty = false;
                                }
                                reqs = std::mem::take(&mut b.show_group_request);
                            }
                            if notify_due && dirty_since_notify {
                                dirty_since_notify = false;
                                crate::diag::bump(&crate::diag::BACKEND_NOTIFY);
                                cx.notify();
                            }
                            #[cfg(any(
                                debug_assertions,
                                moon_profile_debug,
                                feature = "debug-tools"
                            ))]
                            let open_debug_10 = b.take_diag_open_10_btc();
                            #[cfg(not(any(
                                debug_assertions,
                                moon_profile_debug,
                                feature = "debug-tools"
                            )))]
                            let open_debug_10 = false;
                            let chart_consumers = if drain.chart_data {
                                b.live_chart_consumers()
                            } else {
                                Vec::new()
                            };
                            (reqs, open_debug_10, chart_consumers)
                        });
                    for chart in chart_consumers {
                        let _ = chart.update(cx, |chart, cx| {
                            chart.sync_retained_state_if_visible(cx, false);
                        });
                    }
                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    if open_debug_10 {
                        log::info!("diag auto-open: spawning 10 BTC chart windows");
                        spawn_debug_btc_chart_windows(cx, drain_backend.clone());
                    }
                    // Открыть/сфокусировать окна по запросам 👁.
                    for g in show_reqs {
                        spawn_group_window(
                            cx,
                            &drain_backend,
                            &drain_cfg,
                            g,
                            epoch,
                            &drain_layout,
                            0.0,
                        );
                    }
                });
                if last_report.elapsed().as_millis() >= 1000 {
                    let ms = last_report.elapsed().as_secs_f64() * 1000.0;
                    last_report = Instant::now();
                    crate::diag::report(ms);
                }
            }
        })
        .detach();

        // По окну на группу (тем же helper'ом, что и кнопка 👁 «показать группу»).
        for (i, group) in group_list.into_iter().enumerate() {
            spawn_group_window(cx, &backend, &cfg, group, epoch, &layout, i as f32 * 40.0);
        }

        // Восстановить окна откреплённых панелей (панель уже не в доке — она была убрана
        // при откреплении, и dock_persist сохранил док без неё). Порт egui-восстановления
        // detached на старте.
        let specs = backend.read(cx).detached.clone();
        for spec in &specs {
            detached::spawn(cx, &backend, spec);
        }
    });
    Ok(())
}
