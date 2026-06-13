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

use std::time::{Duration, Instant};

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    table::{Column, Table, TableDelegate, TableState},
    v_flex, Root, StyledExt,
};

use moon_core::config::AppConfig;
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

/// Общий backend: живёт в одном `Entity`, дренится таймером, будит окна по notify.
struct Backend {
    session: SessionManager,
    metrics: Metrics,
    snap: MetricsSnapshot,
    /// Желаемые открытые рынки (ядро, рынок) — держим подписку через coordinator.
    desired: Vec<(CoreId, String)>,
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
                .text_color(if r.side == "LONG" { rgb(0x4ade80) } else { rgb(0xf87171) })
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

/// Оболочка одной группы (= одно ОС-окно).
struct Shell {
    backend: Entity<Backend>,
    group: String,
    orders: Entity<TableState<OrdersDelegate>>,
}

impl Shell {
    fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let orders = cx.new(|cx| TableState::new(OrdersDelegate::new(), window, cx));
        // Когда backend дренится — пересобираем строки таблицы и перерисовываемся.
        let g = group.clone();
        cx.observe(&backend, move |this, backend, cx| {
            let rows = collect_orders(backend.read(cx), &g);
            this.orders.update(cx, |st, cx| {
                st.delegate_mut().rows = rows;
                cx.notify();
            });
            cx.notify();
        })
        .detach();
        Self { backend, group, orders }
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Данные для header (строки таблицы наполняет observe в Shell::new).
        let order_count = self.orders.read(cx).delegate().rows.len();
        let b = self.backend.read(cx);
        let conn = b.session.conn_summary_group(&self.group);
        let snap = b.snap;

        // Фокус-рынок группы (первое ядро) + цена/тики из market_view — владеющие.
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

        // Цвета (палитра проекта — moon_core::palette).
        let bg = rgb(0x0c0c0c);
        let panel = rgb(0x161616);
        let border = rgb(0x262626);
        let muted = rgb(0x9a9a9a);
        let accent = rgb(0x47b3ff);

        v_flex()
            .size_full()
            .bg(bg)
            .text_color(rgb(0xe6e6e6))
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
                                Button::new("gear")
                                    .ghost()
                                    .label("⚙")
                                    .on_click(|_, _, _| log::info!("settings clicked")),
                            ),
                    ),
            )
            // ── Центр: чарт-плейсхолдер | панель ордера ─────────────
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(div().text_color(muted).child("chart — открой монету (этап интеграции)"))
                            .child(div().text_color(muted).text_xs().child("wgpu-чарт: вариант A/C/D, Лаба 2b")),
                    )
                    .child(
                        v_flex()
                            .w(px(260.0))
                            .h_full()
                            .p_3()
                            .gap_2()
                            .bg(panel)
                            .border_l_1()
                            .border_color(border)
                            .child(div().text_color(muted).text_xs().child("ORDER"))
                            .child(Button::new("buy").success().label("BUY").on_click(|_, _, _| log::info!("BUY")))
                            .child(Button::new("sell").danger().label("SELL").on_click(|_, _, _| log::info!("SELL")))
                            .child(Button::new("cancel").warning().label("Cancel Buy").on_click(|_, _, _| log::info!("Cancel")))
                            .child(Button::new("panic").danger().label("PANIC SELL").on_click(|_, _, _| log::info!("PANIC"))),
                    ),
            )
            // ── Нижний док: открытые ордера группы на виртуализированной Table ──
            .child(
                v_flex()
                    .w_full()
                    .h(px(220.0))
                    .bg(rgb(0x101010))
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .w_full()
                            .px_3()
                            .py_1()
                            .text_color(muted)
                            .text_xs()
                            .bg(panel)
                            .child("ORDERS"),
                    )
                    .child(div().flex_1().w_full().child(Table::new(&self.orders))),
            )
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

    let app = Application::new().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);

        let backend = cx.new(|_| Backend {
            session: SessionManager::start(&cfg, 0.0, None),
            metrics: Metrics::new(),
            snap: MetricsSnapshot::default(),
            // open = рынки ОТКРЫТЫХ чарт-панелей (как App::about_to_wait в egui).
            // Пусто на старте; наполнится при открытии монеты (порт чарт-панелей).
            // set_open всё равно избирает провайдера/биржу на старте → subscribe_all_trades
            // (ретейн всех трейдов биржи — как было; ради мгновенного открытия монеты).
            desired: Vec::new(),
        });

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
            let off = i as f32 * 40.0;
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(80.0 + off), px(80.0 + off)),
                    size: size(px(1100.0), px(720.0)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("MoonTerminal — {group}").into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            cx.open_window(opts, |window, cx| {
                let view = cx.new(|cx| Shell::new(backend, group, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("open_window");
        }
    });
    Ok(())
}
