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
mod chart;
mod chart_tabs;
mod controls;
mod dock_persist;
mod input;
mod panels;
mod settings;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::*;

use gpui_component::{
    button::{Button, ButtonVariants},
    dock::{DockArea, DockAreaState, DockEvent, DockItem, PanelView},
    h_flex,
    table::{Column, TableDelegate, TableState},
    theme::{Theme, ThemeMode},
    v_flex, Root, StyledExt,
};

use chart_tabs::ChartTabs;
use dock_persist::DOCK_VERSION;
use panels::{DetectsPanel, OrderPanel, OrdersPanel, StubPanel};

use moon_core::config::{AppConfig, GroupLayout, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::palette;
use moon_core::session::{CoreId, SessionManager};

/// Палитра проекта [u8;3] → 0xRRGGBB для gpui `rgb()`. Единый источник цветов —
/// `moon_core::palette` (тот же, что у egui-хрома); никаких литералов в UI.
fn hex(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// Брендируем глобальную тему gpui-component из `moon_core::palette` → ВСЕ компоненты
/// (кнопки/инпуты/таблицы/док/вкладки/тулбар) сразу в наших цветах. Форсируем Dark,
/// затем перекрываем ключевые поля. `ChartTheme` (движок чарта) — отдельно.
fn apply_brand_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let h = |c: [u8; 3]| -> Hsla { rgb(hex(c)).into() };
    let c = &mut Theme::global_mut(cx).colors;
    c.background = h(palette::BG);
    c.foreground = h(palette::TEXT);
    c.border = h(palette::LIFT_HOVER);
    c.muted = h(palette::SURFACE_1);
    c.muted_foreground = h(palette::TEXT_2);
    c.accent = h(palette::ACCENT);
    c.accent_foreground = h(palette::BG);
    c.primary = h(palette::ACCENT);
    c.primary_foreground = h(palette::BG);
    c.secondary = h(palette::LIFT);
    c.secondary_foreground = h(palette::TEXT);
    c.input = h(palette::LIFT);
    c.popover = h(palette::SURFACE_1);
    c.popover_foreground = h(palette::TEXT);
    c.list = h(palette::BG);
    c.list_head = h(palette::SURFACE_1);
    c.list_hover = h(palette::LIFT_HOVER);
    c.table = h(palette::BG);
    c.table_head = h(palette::SURFACE_1);
    c.table_hover = h(palette::LIFT_HOVER);
    c.table_row_border = h(palette::LIFT);
    c.tab_bar = h(palette::SURFACE_1);
    c.tab = h(palette::SURFACE_1);
    c.tab_active = h(palette::BG);
    c.tab_active_foreground = h(palette::ACCENT);
    c.tab_foreground = h(palette::TEXT_2);
    c.title_bar = h(palette::SURFACE_1);
    c.title_bar_border = h(palette::LIFT_HOVER);
    c.danger = h(palette::RED);
    c.success = h(palette::GREEN);
    c.ring = h(palette::ACCENT);
    c.selection = h(palette::ACCENT);
}

/// Общий backend: живёт в одном `Entity`, дренится таймером, будит окна по notify.
struct Backend {
    session: SessionManager,
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
    /// Раскладка окон (геометрия по группам) — load на старте, save на изменении
    /// (дебаунс через дренаж-таймер). Порт egui WindowLayout/layout.toml.
    layout: WindowLayout,
    layout_dirty: bool,
    /// Раскладка доков (группа → DockAreaState) — load на старте, save по
    /// DockEvent::LayoutChanged (дебаунс тем же таймером). Пишется в docks.json.
    dock_states: HashMap<String, DockAreaState>,
    dock_dirty: bool,
    /// Масштаб цены (Y) тулбара: None = «Авто». Правит `ScalePanel`, применяет
    /// `ChartPanel` ко всем графикам. Порт egui `OrderControls`/тулбара.
    price_scale: Option<f32>,
    /// Live-follow тулбара: true = вид бежит за «сейчас», false = пауза (заморозка).
    follow: bool,
}

/// Плоская строка ордера для таблицы (владеющая; собирается из OrderRow + имя ядра).
#[derive(Clone)]
struct OrderView {
    core: String,
    side: &'static str, // LONG / SHORT
    market: String,
    size: f64,
    buy_price: f64,
    price: f32,
    fill_pct: f32,
    strat: String,
}

/// Делегат виртуализированной таблицы ордеров группы.
struct OrdersDelegate {
    columns: Vec<Column>,
    rows: Vec<OrderView>,
}

impl OrdersDelegate {
    fn new() -> Self {
        let columns = vec![
            Column::new("core", "Core").width(px(110.0)),
            Column::new("side", "Side").width(px(64.0)),
            Column::new("market", "Market").width(px(120.0)),
            Column::new("size", "Size").width(px(90.0)).text_right(),
            Column::new("buy", "Buy").width(px(100.0)).text_right(),
            Column::new("price", "Price").width(px(100.0)).text_right(),
            Column::new("fill", "Fill%").width(px(70.0)).text_right(),
            Column::new("strat", "Strat").width(px(140.0)),
        ];
        Self { columns, rows: Vec::new() }
    }
}

impl TableDelegate for OrdersDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(r) = self.rows.get(row_ix) else {
            return div();
        };
        match col_ix {
            0 => div().child(r.core.clone()),
            1 => div()
                .text_color(if r.side == "LONG" { rgb(hex(palette::GREEN)) } else { rgb(hex(palette::RED)) })
                .child(r.side),
            2 => div().child(r.market.clone()),
            3 => div().w_full().child(format!("{:.4}", r.size)),
            4 => div().w_full().child(format!("{:.4}", r.buy_price)),
            5 => div().w_full().child(format!("{:.4}", r.price)),
            6 => div().w_full().child(format!("{:.0}", r.fill_pct)),
            _ => div().child(r.strat.clone()),
        }
    }
}

/// Собирает ордера всех ядер группы в плоские строки таблицы (новые сверху по uid).
fn collect_orders(backend: &Backend, group: &str) -> Vec<OrderView> {
    let store = backend.session.store();
    let mut out: Vec<(u64, OrderView)> = Vec::new();
    for s in backend.session.sessions().iter().filter(|s| s.group == group) {
        let Some(core) = store.core(s.id) else { continue };
        for o in &core.orders {
            out.push((
                o.uid,
                OrderView {
                    core: s.name.clone(),
                    side: if o.is_short { "SHORT" } else { "LONG" },
                    market: o.market.clone(),
                    size: o.size,
                    buy_price: o.buy_price,
                    price: o.price,
                    fill_pct: o.fill_pct,
                    strat: o.strat.clone(),
                },
            ));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0)); // новые ордера (больше uid) сверху
    out.into_iter().map(|(_, v)| v).collect()
}

/// Оболочка одной группы (= одно ОС-окно): header + единый `DockArea` + статус.
/// Весь контент — Dock-панели (чарт=center, детекты/ордер=right, нижние вкладки=
/// bottom), перетаскиваемые/отцепляемые. Header/статус — фикс. полосы вокруг дока.
struct Shell {
    backend: Entity<Backend>,
    group: String,
    dock: Entity<DockArea>,
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
        // нижние вкладки=bottom. Все перетаскиваемые/отцепляемые (gpui-component Dock).
        let dock = cx.new(|cx| DockArea::new("group-dock", Some(DOCK_VERSION), window, cx));
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
            let charts = cx.new(|cx| ChartTabs::new(backend.clone(), group.clone(), focus, epoch, theme.clone(), window, cx));
            let detects = cx.new(|cx| DetectsPanel::new(backend.clone(), group.clone(), cx));
            let order = cx.new(|cx| OrderPanel::new(cx));
            let orders_panel = cx.new(|cx| OrdersPanel::new(backend.clone(), group.clone(), window, cx));
            let assets = cx.new(|cx| StubPanel::new("Assets", "Активы", cx));
            let log = cx.new(|cx| StubPanel::new("Log", "Лог", cx));
            let report = cx.new(|cx| StubPanel::new("Report", "Отчёт", cx));

            // ВСЁ — в center-сплите (свободный пересплит drag-to-edge + детач панелей).
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
            let bottom_tabs: Vec<Arc<dyn PanelView>> = vec![
                Arc::new(orders_panel),
                Arc::new(assets),
                Arc::new(log),
                Arc::new(report),
            ];
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

        // Header читает backend каждый кадр → перерисовка по дренажу.
        cx.observe(&backend, |_this, _backend, cx| cx.notify()).detach();

        // Любое изменение раскладки доков (drag/split/resize/detach) → дамп в backend,
        // сохранение дебаунсит дренаж-таймер (docks.json). Порт персиста раскладки.
        cx.subscribe(&dock, |this, dock, event: &DockEvent, cx| {
            if let DockEvent::LayoutChanged = event {
                let state = dock.read(cx).dump(cx);
                let group = this.group.clone();
                this.backend.update(cx, |b, _| {
                    b.dock_states.insert(group, state);
                    b.dock_dirty = true;
                });
            }
        })
        .detach();

        Self { backend, group, dock }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let order_count = collect_orders(self.backend.read(cx), &self.group).len();

        // Header-данные (рынок/цена/тики/conn). Чарт/ввод/оси — в ChartPanel.
        let (conn, snap, market_label, price_label, tick_count) = {
            let b = self.backend.read(cx);
            let conn = b.session.conn_summary_group(&self.group);
            let snap = b.snap;
            let (market_label, price_label, tick_count) = {
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
                        ),
                        None => (m, "—".into(), 0),
                    },
                    None => ("—".into(), "—".into(), 0),
                }
            };
            (conn, snap, market_label, price_label, tick_count)
        };

        // Цвета — ТОЛЬКО из moon_core::palette (единый источник, как egui-хром).
        let bg = rgb(hex(palette::BG));
        let panel = rgb(hex(palette::SURFACE_1));
        let border = rgb(hex(palette::LIFT_HOVER));
        let muted = rgb(hex(palette::TEXT_2));
        let accent = rgb(hex(palette::ACCENT));

        v_flex()
            .size_full()
            .bg(bg)
            .text_color(rgb(hex(palette::TEXT)))
            .text_sm()
            // ── Header ──────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2()
                    .gap_4()
                    .justify_between()
                    .bg(panel)
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().text_color(accent).font_bold().child(self.group.clone()))
                            .child(div().child(market_label))
                            .child(div().text_color(accent).child(price_label))
                            .child(div().text_color(muted).text_xs().child(format!("{tick_count} ticks"))),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().child(format!("{}/{} connected", conn.ready, conn.total)))
                            .child(div().child(format!("orders {order_count}")))
                            .child(
                                Button::new("gear").ghost().label("⚙").on_click({
                                    let backend = self.backend.clone();
                                    move |_, _, cx| settings::open(backend.clone(), cx)
                                }),
                            ),
                    ),
            )
            // ── Тулбар: тонкая фикс. полоса (Размеры/Продажа/Масштаб+Live), порт верхней
            //    полосы стенда. Не dock-панель — единый ряд на высоту кнопки. ──
            .child(controls::toolbar(&self.backend, cx))
            // ── Центр: единый DockArea (чарт=center, детекты+ордер=right, вкладки=bottom) ──
            .child(div().flex_1().w_full().child(self.dock.clone()))
            // ── Status bar ──────────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_1()
                    .gap_4()
                    .bg(panel)
                    .border_t_1()
                    .border_color(border)
                    .text_color(muted)
                    .text_xs()
                    .child(format!("CPU proc {:.0}%", snap.cpu_process))
                    .child(format!("CPU sys {:.0}%", snap.cpu_system))
                    .child(format!("RAM {:.0} MB", snap.mem_mb)),
            )
    }
}

/// Активные группы конфига (уникальные, в порядке появления). Нет — одна "default".
fn groups(cfg: &AppConfig) -> Vec<String> {
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

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,moon_gpui=info,moon_core=info"),
    )
    .init();

    let cfg = AppConfig::load()?;
    let group_list = groups(&cfg);
    log::info!("groups: {group_list:?} (servers: {})", cfg.servers.len());

    // Единая точка отсчёта времени для сессий и чарт-вью (как epoch_ms в egui).
    let epoch = moon_chart::paint::now_unix_ms();

    let app = Application::new().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);
        apply_brand_theme(cx);

        let layout = WindowLayout::load();
        let dock_states = dock_persist::load_all();

        let backend = cx.new(|_| Backend {
            session: SessionManager::start(&cfg, epoch, None),
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
            layout: layout.clone(),
            layout_dirty: false,
            dock_states,
            dock_dirty: false,
            price_scale: None,
            follow: true,
        });

        // Фабрики панелей для восстановления раскладки доков (PanelRegistry — глобален).
        dock_persist::register_panels(cx, backend.clone(), epoch);

        // Дренаж сессий + метрики раз в 100мс на UI-потоке → notify окон.
        let drain_backend = backend.clone();
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone())?;
            loop {
                executor.timer(Duration::from_millis(100)).await;
                let ok = cx
                    .update(|cx| {
                        drain_backend.update(cx, |b, cx| {
                            b.session.drain();
                            // Каждый кадр (как egui app/mod.rs): reconcile_providers
                            // избирает провайдера/биржу + держит подписку на desired-рынки.
                            // subscribe_all_trades провайдера = ретейн всех трейдов биржи
                            // (десятки ГБ — by-design, ради мгновенного открытия монеты;
                            // дедуп держит 1 провайдера/биржу).
                            b.session.set_open(&b.desired);
                            b.snap = b.metrics.sample(Instant::now());
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
                            cx.notify();
                        });
                    })
                    .is_ok();
                if !ok {
                    break; // приложение закрылось
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .detach();

        // По окну на группу.
        for (i, group) in group_list.into_iter().enumerate() {
            let backend = backend.clone();
            // Фокус-монета группы: первое активное ядро группы + его настроенный рынок
            // (открываем сразу — провайдер уже ретейнит трейды). 1:1 с egui-открытием.
            let focus: Option<(CoreId, String)> = cfg
                .servers
                .iter()
                .find(|s| s.active && cfg.group(&s.group).active && s.group == group)
                .map(|s| (s.id, s.market.clone()));
            let off = i as f32 * 40.0;
            // Восстановить геометрию окна группы из сохранённой раскладки.
            let win_bounds = match layout.groups.get(&group) {
                Some(g) => Bounds {
                    origin: point(px(g.x as f32), px(g.y as f32)),
                    size: size(px(g.w as f32), px(g.h as f32)),
                },
                None => Bounds {
                    origin: point(px(80.0 + off), px(80.0 + off)),
                    size: size(px(1100.0), px(720.0)),
                },
            };
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(win_bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("MoonTerminal — {group}").into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let theme = cfg.theme.clone();
            cx.open_window(opts, |window, cx| {
                let view = cx.new(|cx| Shell::new(backend, group, focus, epoch, theme, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("open_window");
        }
    });
    Ok(())
}
