//! Оболочка одной группы (Shell): одно ОС-окно = header + единый `DockArea` + статус-бар.
//! Вынесено из main.rs. `Backend` живёт в крейт-руте — доступ к его приватным полям из
//! этого модуля сохраняется (правило: потомок видит приватное предка).

use std::rc::Rc;
use std::time::Instant;

use gpui::*;

use moon_ui::{
    DockArea, DockEvent, DockItem, DockPlacement, MoonBackgroundPolicy, MoonPalette, MoonStatusBar,
    MoonStatusIndicator, MoonStatusItem, MoonTooltipView, MoonWindowFrame, PanelView, v_flex,
};

use moon_core::config::GroupLayout;
use moon_core::feed::ConnStatus;
use moon_core::metrics::MetricsSnapshot;
use moon_core::session::{ConnSummary, CoreId};

use crate::chart_tabs::ChartTabs;
use crate::dock_persist::DOCK_VERSION;
use crate::panels::{AssetsView, DetectsPanel, LogPanel, OrderPanel, OrdersPanel, ReportPanel};
use crate::{Backend, controls, design, detached, panels, terminal_chrome};

/// Оболочка одной группы (= одно ОС-окно): header + единый `DockArea` + статус.
/// Весь контент — Dock-панели (чарт=center, детекты/ордер=right, нижние вкладки=
/// bottom), перетаскиваемые/отцепляемые. Header/статус — фикс. полосы вокруг дока.
pub(crate) struct Shell {
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
    /// Имена панелей, чью × нажали (DockEvent::PanelCloseRequested). Обрабатываются в render
    /// (нужен window): панель убирается с текущего места и возвращается в нижнюю строку.
    pending_close: Vec<String>,
}

/// Имена dock-панелей нижней строки в порядке их «домашних» позиций. Возврат
/// откреплённой/закрытой панели вставляет её в TabPanel на индекс, сохраняющий этот
/// порядок (см. [`dock_home_target_ix`]).
const DOCK_TAB_ORDER: [&str; 4] = ["Orders", "Assets", "Log", "Report"];

/// «Домашний» индекс панели в нижней строке (Orders<Assets<Log<Report). Используется как
/// позиция вставки при возврате; форк клампит её к числу вкладок, поэтому при частично
/// откреплённом наборе панель встаёт примерно на своё место (порядок сохраняется).
fn dock_home_priority(name: &str) -> usize {
    DOCK_TAB_ORDER
        .iter()
        .position(|n| *n == name)
        .unwrap_or(DOCK_TAB_ORDER.len())
}

impl Shell {
    pub(crate) fn new(
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
                    AssetsView::restored_group(backend.clone(), group.clone(), window, cx)
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
            if let DockEvent::PanelCloseRequested { panel_name } = event {
                this.pending_close.push(panel_name.to_string());
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
            pending_close: Vec::new(),
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
                let ix = dock_home_priority(&panel_name);
                dock.update(cx, |area, cx| {
                    // Возврат в нижнюю строку вкладок на исходную позицию (а не в Center-корень,
                    // что схлопнуло бы весь split). Ищем Tabs-узел с dock-панелями и вставляем
                    // туда по домашнему порядку Orders<Assets<Log<Report. Fallback — если строки
                    // вкладок нет совсем (все откреплены).
                    if !area.insert_panel_into_home_tabs(
                        panel.clone(),
                        ix,
                        &DOCK_TAB_ORDER,
                        window,
                        cx,
                    ) {
                        // Крайний случай (ни одного соседа в доке): кладём в отдельный
                        // bottom-dock, а НЕ в Center — add_panel(Center) на split-раскладке
                        // схлопнул бы её в фулскрин.
                        area.add_panel(panel, DockPlacement::Bottom, None, window, cx);
                    }
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
            // «Активы» откпрепляются НЕ в per-group окно, а в ГЛОБАЛЬНОЕ singleton-окно
            // (все ядра, с деревом переноса) — как окно «Стратегии». Панель из дока не
            // убираем (она остаётся вкладкой group-scope). Так дабл-клик = кнопка «⧉».
            if panel_name == "Assets" {
                let backend = self.backend.clone();
                let owner = window.window_handle();
                cx.defer(move |cx| {
                    crate::panels::open_assets_window(backend, Some(owner), cx);
                });
                continue;
            }
            let group = self.group.clone();
            if detached::supports_panel(&panel_name) {
                self.dock.update(cx, |area, cx| {
                    area.remove_panel_by_name(&panel_name, window, cx);
                });
                let spec = detached::DetachedSpec::new(group, panel_name);
                let backend = self.backend.clone();
                let owner = window.window_handle();
                // open_window нельзя звать во время render (arena уже очищается) — отложим.
                // Это путь дабл-клика/встроенного detach; toolbar-⧉ спавнит из on_click сам.
                cx.defer(move |cx| {
                    detached::spawn(cx, &backend, &spec, Some(owner));
                    backend.update(cx, |b, _| {
                        if !b
                            .detached
                            .iter()
                            .any(|s| s.group == spec.group && s.panel == spec.panel)
                        {
                            b.detached.push(spec);
                            b.detached_dirty = true;
                        }
                    });
                });
            }
        }

        // Закрытие (×) dock-панели: не удаляем, а возвращаем в нижнюю строку на своё место.
        // Убираем с текущего места (split-слот схлопнётся) и вставляем свежую в home-tabs.
        let closes = std::mem::take(&mut self.pending_close);
        for panel_name in closes {
            if !detached::supports_panel(&panel_name) {
                continue;
            }
            let group = self.group.clone();
            let backend = self.backend.clone();
            let ix = dock_home_priority(&panel_name);
            if let Some(panel) = detached::build_panel(&panel_name, &group, &backend, window, cx) {
                self.dock.update(cx, |area, cx| {
                    area.remove_panel_by_name(&panel_name, window, cx);
                    if !area.insert_panel_into_home_tabs(
                        panel.clone(),
                        ix,
                        &DOCK_TAB_ORDER,
                        window,
                        cx,
                    ) {
                        // Крайний случай (ни одного соседа в доке): кладём в отдельный
                        // bottom-dock, а НЕ в Center — add_panel(Center) на split-раскладке
                        // схлопнул бы её в фулскрин.
                        area.add_panel(panel, DockPlacement::Bottom, None, window, cx);
                    }
                });
            }
        }

        // Снять геометрию окна → раскладка (save дебаунсит дренаж-таймер). Для Maximized/
        // Fullscreen window_bounds() отдаёт RESTORE-bounds (размер в обычном состоянии) —
        // сохраняем именно их + флаг maximized, иначе развёрнутое окно теряло размер и при
        // следующем запуске открывалось «сжатым» (старое условие ловило только Windowed).
        let (b, maximized) = match window.window_bounds() {
            WindowBounds::Windowed(b) => (Some(b), false),
            WindowBounds::Maximized(b) => (Some(b), true),
            WindowBounds::Fullscreen(b) => (Some(b), false),
        };
        if let Some(b) = b {
            let g = GroupLayout {
                x: f32::from(b.origin.x) as i32,
                y: f32::from(b.origin.y) as i32,
                w: f32::from(b.size.width) as u32,
                h: f32::from(b.size.height) as u32,
                maximized,
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
                    .map(|o| {
                        o.x != g.x
                            || o.y != g.y
                            || o.w != g.w
                            || o.h != g.h
                            || o.maximized != g.maximized
                    })
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
                    Some((core, m)) => b.session.with_market_view(core, &m, |data| {
                        let label = m.clone();
                        match data {
                            Some(v) => (
                                label,
                                v.last_price
                                    .map(|p| format!("{p:.2}"))
                                    .unwrap_or_else(|| "—".into()),
                                v.ticks_rev as usize,
                                v.book.len(),
                            ),
                            None => (label, "—".into(), 0, 0),
                        }
                    }),
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
            .text_size(design::text_px(cx, 11.0))
            // ── Header ──────────────────────────────────────────────
            .child(terminal_chrome::header(
                &self.group,
                market_label,
                self.backend.clone(),
                p,
                cx,
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
            .child(
                MoonWindowFrame::main("moon-main-window-frame", chrome_width)
                    .header_height(design::HEADER_TOP_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
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
                    .px(design::ui_px(cx, 6.0))
                    .h(design::fit_h_px(cx, 16.0, 10.0, 3.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .cursor_pointer()
                    .font_family(design::mono())
                    .text_size(design::text_px(cx, 10.0))
                    .text_color(rgb(p.amber))
                    .bg(rgba(0x00000044))
                    .hover(|s| s.bg(rgba(0x2A2520EE)).text_color(rgb(0xF7C663)))
                    .on_click({
                        let group = self.group.clone();
                        move |_, window, cx| {
                            crate::debug_window::open_debug_perf_window(
                                cx,
                                backend.clone(),
                                group.clone(),
                                Some(window.window_handle()),
                            )
                        }
                    })
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
