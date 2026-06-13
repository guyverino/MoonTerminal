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
    h_flex, v_flex, Root, StyledExt,
};

use moon_core::config::AppConfig;
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::SessionManager;

/// Общий backend: живёт в одном `Entity`, дренится таймером, будит окна по notify.
struct Backend {
    session: SessionManager,
    metrics: Metrics,
    snap: MetricsSnapshot,
}

/// Оболочка одной группы (= одно ОС-окно).
struct Shell {
    backend: Entity<Backend>,
    group: String,
}

impl Shell {
    fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        // Перерисовываемся, когда backend дренится (notify).
        cx.observe(&backend, |_this, _backend, cx| cx.notify()).detach();
        Self { backend, group }
    }
}

/// Снимок одного ядра группы для строки нижнего дока (владеющие данные —
/// собираются до построения дерева, чтобы не держать заём на backend).
struct CoreRow {
    name: String,
    status: String,
    ready: bool,
    orders: usize,
    detects: usize,
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Собираем все нужные данные во владеющие структуры, затем строим UI.
        let b = self.backend.read(cx);
        let conn = b.session.conn_summary_group(&self.group);
        let snap = b.snap;
        let store = b.session.store();
        let mut rows: Vec<CoreRow> = Vec::new();
        let mut total_orders = 0usize;
        for s in b.session.sessions().iter().filter(|s| s.group == self.group) {
            let (status, ready, orders, detects) = match store.core(s.id) {
                Some(c) => (
                    format!("{:?}", c.status),
                    c.status == moon_core::feed::ConnStatus::Ready,
                    c.orders.len(),
                    c.detects.len(),
                ),
                None => ("—".to_string(), false, 0, 0),
            };
            total_orders += orders;
            rows.push(CoreRow { name: s.name.clone(), status, ready, orders, detects });
        }

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
                            .child(div().text_color(muted).child("—")) // рынок (появится с чартом)
                            .child(div().text_color(muted).child("price —")),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().child(format!("{}/{} connected", conn.ready, conn.total)))
                            .child(div().child(format!("orders {total_orders}")))
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
            // ── Нижний док: живой список ядер группы (реальные данные) ──
            .child(
                v_flex()
                    .w_full()
                    .h(px(190.0))
                    .bg(rgb(0x101010))
                    .border_t_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .w_full()
                            .px_3()
                            .py_1()
                            .gap_4()
                            .text_color(muted)
                            .text_xs()
                            .bg(panel)
                            .child(div().w(px(160.0)).child("CORE"))
                            .child(div().w(px(120.0)).child("STATUS"))
                            .child(div().w(px(80.0)).child("ORDERS"))
                            .child(div().w(px(80.0)).child("DETECTS")),
                    )
                    .child(
                        v_flex().w_full().children(rows.into_iter().map(move |r| {
                            h_flex()
                                .w_full()
                                .px_3()
                                .py_1()
                                .gap_4()
                                .border_b_1()
                                .border_color(rgb(0x1c1c1c))
                                .child(div().w(px(160.0)).child(r.name))
                                .child(
                                    div()
                                        .w(px(120.0))
                                        .text_color(if r.ready { rgb(0x4ade80) } else { rgb(0xfacc15) })
                                        .child(r.status),
                                )
                                .child(div().w(px(80.0)).child(r.orders.to_string()))
                                .child(div().w(px(80.0)).text_color(muted).child(r.detects.to_string()))
                        })),
                    ),
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
                let view = cx.new(|cx| Shell::new(backend, group, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("open_window");
        }
    });
    Ok(())
}
