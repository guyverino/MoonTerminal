//! Кадр окна группы: оркестратор [`WindowHost::render`] и его фазы —
//! пейсинг/FPS → втягивание детектов → расчёт шапки → egui-хром → рендер
//! панелей → egui-пасс → overlay-оси → present. Тяжёлые wgpu-проходы по панелям
//! и оси делегированы в [`crate::chart::paint`]; здесь — сборка кадра и UI-хром.

use std::collections::HashSet;
use std::time::Instant;

use crate::chart::container::{ContainerKind, Mode};
use crate::chart::paint::MIN_FRAME_DT;
use crate::chart::view::Rect;
use crate::feed::ConnStatus;
use crate::metrics::MetricsSnapshot;
use crate::session::{ConnSummary, SessionManager};
use crate::shell::{ShellInfo, HEADER_H, STATUS_H};

use super::signatures::{chrome_sig, conn_summary_sig};
use super::{chart_tab, HostRender, WindowHost, CHART_TABS_H, EGUI_THROTTLE};

/// Данные шапки/статуса на кадр: всё, что нужно для хром-сигнатуры, ShellInfo и
/// финального трекинга. Считаются из фокус-панели активного контейнера.
struct Header {
    market: String,
    status: ConnStatus,
    last_price: Option<f32>,
    tick_count: usize,
    book_levels: usize,
    conn: ConnSummary,
    conn_sig: u64,
    orders_sig: u64,
}

/// Результат egui-хром-прохода, который нужен «хвосту» кадра (после прохода
/// borrow'ы egui-замыкания уже отпущены). Запросы окон/откреплений уходят в App;
/// `layout_changed`/`to_free` — для финального dirty-трекинга и освобождения
/// текстур после present.
#[derive(Default)]
struct ChromeOut {
    gear_clicked: bool,
    strategies_clicked: bool,
    detach: Option<crate::dock::DockTab>,
    repin: Option<crate::dock::DockTab>,
    detach_chart: Option<usize>,
    layout_changed: bool,
    to_free: Vec<egui::TextureId>,
}

impl WindowHost {
    pub fn render(
        &mut self,
        session: &SessionManager,
        now_ms: f64,
        metrics: MetricsSnapshot,
        report: &mut crate::dock::ReportView,
        global_detached: [bool; 4],
        detached_keys: &HashSet<ContainerKind>,
        split_by_core: bool,
    ) -> HostRender {
        let store = session.store();
        let none = HostRender {
            gear_clicked: false,
            strategies_clicked: false,
            detach: None,
            repin: None,
            detach_chart: None,
            addto_detached: Vec::new(),
        };

        let now = Instant::now();
        // Кап частоты кадров: при движении мыши перекрестие иначе гонит present в
        // потолок монитора (120/144) и греет GPU. Скип дёшев — needs_render
        // вернёт true в следующем цикле, кадр случится как только кап позволит.
        if now.duration_since(self.last_present_at) < MIN_FRAME_DT {
            return none;
        }

        // FPS.
        let dt = now.duration_since(self.last_frame).as_secs_f32().max(1e-4);
        self.last_frame = now;
        self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;

        // present/s за скользящую секунду (метку добавим после present()).
        while self
            .present_marks
            .front()
            .is_some_and(|t| now.duration_since(*t).as_secs_f32() > 1.0)
        {
            self.present_marks.pop_front();
        }
        self.present_hz = self.present_marks.len() as f32;

        // AddToChart: втянуть свежие детекты (откреплённые номера — на проброс в их
        // окна); убрать истёкшие.
        let (added, addto_forwarded) =
            self.ingest_addtochart(store, now_ms, detached_keys, split_by_core);
        let pruned = self.prune_panes(now_ms);
        // Появился/исчез контейнер или панели → перестроить хром (верхние вкладки).
        if added || pruned {
            self.egui_dirty = true;
        }

        // Двойной клик по чарту нумерованной вкладки → открыть монету на Main.
        self.apply_pending_to_main(now_ms);

        let ppp = self.window.scale_factor() as f32;
        let resolution = [self.gpu.size.width as f32, self.gpu.size.height as f32];

        let header = self.compute_header(session, store);

        // 2b: гоняем egui (layout+tessellate) только когда реально надо. Иначе —
        // переиспользуем кэш-сетку: smooth scroll и hover над графиком не меняют
        // хром → нет CPU на тесселяцию хрома каждый кадр.
        let size_now = (self.gpu.size.width, self.gpu.size.height);
        let sig = chrome_sig(
            &header.market,
            &header.status,
            header.last_price,
            header.orders_sig,
            header.conn_sig,
        );
        // Переключение вкладок/живые Лог-Отчёт перерисовываются через egui_dirty
        // (клик по вкладке = ввод; App форсит mark_egui_dirty по ревизии данных).
        let run_egui = self.egui_tris.is_none()
            || self.egui_dirty
            || self.egui_wants_repaint
            || size_now != self.egui_cache_size
            || sig != self.last_chrome_sig
            || now.duration_since(self.last_egui_run) >= EGUI_THROTTLE;

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.gpu.surface.configure(&self.gpu.device, &self.gpu.config);
                return none;
            }
            Err(e) => {
                log::warn!("surface error: {e:?}");
                return none;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
            pixels_per_point: if run_egui { ppp } else { self.egui_ppp },
        };

        // egui-хром (тулбар/док/вкладки/крестики) — только когда run_egui; иначе
        // переиспользуем прошлую тесселяцию.
        let chrome = if run_egui {
            self.run_chrome(
                store,
                now_ms,
                report,
                global_detached,
                &metrics,
                &header,
                ppp,
                resolution,
                sig,
                size_now,
                now,
                &screen,
                &mut encoder,
            )
        } else {
            ChromeOut::default()
        };

        // Чарт(ы): панели активного контейнера в их полосы. Раскладку (физ. px)
        // запоминаем для hit-теста ввода.
        let area = self.egui_area;
        self.chart_area = (area.x, area.y, area.w, area.h);
        let cur = self
            .cursor
            .filter(|(_, y)| *y >= area.y && *y <= area.y + area.h);
        let ac = self.active_container;
        let layout = crate::chart::paint::render_panes(
            &mut self.containers[ac],
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &view,
            area,
            resolution,
            ppp,
            now_ms,
            &self.theme,
            self.hovered_pane,
            cur,
            session,
        );
        self.pane_rects = layout.clone();
        let render_open = !layout.is_empty();

        // egui-проход (всегда) — кэш-сеткой поверх чарта. На reuse-кадрах
        // update_buffers не зовём: буферы рендерера держат прошлую (ту же) сетку.
        let tris = self.egui_tris.take();
        if let Some(tris) = &tris {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut rpass = rpass.forget_lifetime();
            self.egui_renderer.render(&mut rpass, tris, &screen);
        }
        self.egui_tris = tris;

        // Overlay-слой шкал + readout'ов перекрестия — по одному на видимую панель.
        if render_open && area.w > 1.0 && area.h > 1.0 {
            crate::chart::paint::render_overlay(
                &self.containers[ac],
                &layout,
                &self.overlay_ctx,
                &mut self.overlay_renderer,
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &view,
                (self.gpu.size.width, self.gpu.size.height),
                resolution,
                ppp,
                self.hovered_pane,
                cur,
            );
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        self.last_present_at = now;
        self.present_marks.push_back(now);
        for tex_id in &chrome.to_free {
            self.egui_renderer.free_texture(tex_id);
        }

        self.last_orders_sig = header.orders_sig;
        self.last_detects_sig = self.detects_sig(store);
        self.last_visible_sig = self.visible_sig(session, now_ms);
        // Следующий кадр держим «грязным», если egui анимирует (popup/fade) ИЛИ
        // только что открыли/закрыли чарт ИЛИ удалили истёкшие панели.
        self.dirty = self.egui_wants_repaint || chrome.layout_changed || pruned;

        HostRender {
            gear_clicked: chrome.gear_clicked,
            strategies_clicked: chrome.strategies_clicked,
            detach: chrome.detach,
            repin: chrome.repin,
            detach_chart: chrome.detach_chart,
            addto_detached: addto_forwarded,
        }
    }

    /// Двойной клик по чарту нумерованной вкладки (`pending_to_main`) → открыть
    /// монету на Main фулскрин и перейти туда фокусом.
    fn apply_pending_to_main(&mut self, now_ms: f64) {
        let Some((core, mkt)) = self.pending_to_main.take() else {
            return;
        };
        let main_idx = self
            .containers
            .iter()
            .position(|c| c.kind == ContainerKind::Main)
            .unwrap_or(0);
        let (fmt, epoch) = (self.gpu.format, self.epoch_ms);
        self.containers[main_idx].open_manual(core, &mkt, &self.gpu.device, fmt, epoch);
        self.active_container = main_idx;
        let c = &mut self.containers[main_idx];
        if let Mode::Fullscreen(i) = c.mode {
            if let Some(p) = c.panes.get_mut(i) {
                p.chart.view.resume_live(now_ms);
                p.chart.view.reset_y();
            }
        }
        self.dirty = true;
        self.egui_dirty = true;
    }

    /// Шапка/статус по фокус-панели активного контейнера: рынок, цена, счётчики
    /// тиков/стакана, статус соединения, сводка подключений группы и хром-сигнатуры.
    fn compute_header(&self, session: &SessionManager, store: &crate::session::CoreStore) -> Header {
        // Рынок фокус-панели (или пусто).
        let focused_info = self.focused().map(|p| (p.core, p.market.clone()));
        let (active, market_full) = match &focused_info {
            Some((c, m)) => (*c, Some(m.clone())),
            None => (0, None),
        };
        let market = match &market_full {
            Some(m) => {
                let quote = self
                    .workspace
                    .cores
                    .iter()
                    .find(|c| c.id == active)
                    .map(|c| c.quote.as_str())
                    .unwrap_or("");
                crate::symbol::base_symbol(m, quote).to_string()
            }
            None => "—".to_string(),
        };
        // Рыночные данные (крестики/стакан) — view провайдера биржи активного ядра.
        let mv = market_full
            .as_deref()
            .and_then(|m| session.market_view(active, m));
        let (last_price, tick_count, book_levels) = match mv {
            Some(v) => (v.last_price, v.ring.len(), v.book.len()),
            None => (None, 0, 0),
        };
        // Статус соединения показываем всегда по ядру группы — подключение к ядру
        // живёт и без открытой монеты. Открыт чарт → его ядро, иначе первое ядро.
        let status_core = if active != 0 {
            active
        } else {
            self.workspace.cores.first().map(|c| c.id).unwrap_or(0)
        };
        let status = store
            .core(status_core)
            .map(|d| d.status.clone())
            .unwrap_or(ConnStatus::Connecting);
        // Сводка по ядрам ЭТОЙ группы (окно = группа) — счётчик «N/M подключено».
        let conn = session.conn_summary_group(&self.workspace.group);
        let conn_sig = conn_summary_sig(&conn);

        // Сумма orders_rev группы — для хром-сигнатуры (таблица ордеров) и трекинга.
        let mut orders_sig = 0u64;
        for ci in &self.workspace.cores {
            if let Some(d) = store.core(ci.id) {
                orders_sig = orders_sig.wrapping_add(d.orders_rev);
            }
        }

        Header {
            market,
            status,
            last_price,
            tick_count,
            book_levels,
            conn,
            conn_sig,
            orders_sig,
        }
    }

    /// egui-хром-проход: тулбар (gear/reports/strategies), нижний док, верхняя
    /// полоса чарт-вкладок (с откреплением перетаскиванием) и крестики панелей.
    /// Применяет действия тулбара/дока (масштаб, open/close панелей, переключение
    /// вкладок) и тесселирует хром в кэш-сетку. Возвращает запросы для App.
    #[allow(clippy::too_many_arguments)]
    fn run_chrome(
        &mut self,
        store: &crate::session::CoreStore,
        now_ms: f64,
        report: &mut crate::dock::ReportView,
        global_detached: [bool; 4],
        metrics: &MetricsSnapshot,
        header: &Header,
        ppp: f32,
        resolution: [f32; 2],
        sig: u64,
        size_now: (u32, u32),
        now: Instant,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
    ) -> ChromeOut {
        let mut gear_clicked = false;
        let mut reports_clicked = false;
        let mut strategies_clicked = false;
        let mut detach_req: Option<crate::dock::DockTab> = None;
        let mut repin_req: Option<crate::dock::DockTab> = None;
        let mut open_detect: Option<(crate::session::CoreId, String)> = None;
        let mut close_chart = false;
        // Открыли/закрыли чарт в этом кадре → форсим ещё один кадр (перестроить
        // хром: видимость панели ордера, заголовок), т.к. needs_render иначе может
        // его пропустить при закрытом контейнере без данных.
        let mut layout_changed = false;

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let group_name = self.workspace.group.clone();
        let info = ShellInfo {
            group: &group_name,
            conn_ready: header.conn.ready,
            conn_total: header.conn.total,
            conn_down: &header.conn.down,
            tick_count: header.tick_count,
            book_levels: header.book_levels,
            fps: self.fps,
            present_hz: self.present_hz,
            cpu_process: metrics.cpu_process,
            cpu_system: metrics.cpu_system,
            mem_mb: metrics.mem_mb,
            mem_delta_mb: metrics.mem_delta_mb,
        };
        // Открытые ордера всех ядер группы для нижнего дока (с именем ядра).
        let order_rows = self.collect_orders(store);

        let mut central = egui::Rect::NOTHING;
        let mut detects_rect = egui::Rect::NOTHING;
        let mut set_scale = None;
        let mut set_follow = None;
        let following = self.focused().map(|p| p.chart.view.follow).unwrap_or(true);
        let chart_open = !self.active().is_empty();

        let shell = &mut self.shell;
        let icons = &mut self.icons;
        let cores = &self.workspace.cores;
        let dock = &mut self.workspace.dock;
        let containers = &self.containers;
        let active_container = self.active_container;
        let mut close_pane: Option<usize> = None;
        let mut switch_to: Option<usize> = None;
        let mut detach_chart: Option<usize> = None;
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            let mut open = false;
            let mut reports = false;
            let mut strategies = false;
            shell.ui(ctx, &info, &mut open, &mut reports, &mut strategies, icons);
            if open {
                gear_clicked = true;
            }
            if reports {
                reports_clicked = true;
            }
            if strategies {
                strategies_clicked = true;
            }
            let out = dock.show(
                ctx, cores, store, &order_rows, following, chart_open, now_ms, report,
                global_detached,
            );
            central = out.central;
            detects_rect = out.detects_rect;
            set_scale = out.scale;
            set_follow = out.set_follow;
            open_detect = out.open_detect;
            close_chart = out.close_chart;
            detach_req = out.detach;
            repin_req = out.repin;

            // Верхняя полоса чарт-вкладок (под Size): Main, 1, 2, 3… в стиле
            // нижнего дока (underline), с бейджем-счётчиком открытых графиков.
            // Видна ТОЛЬКО когда есть хотя бы одна нумерованная вкладка (пришёл
            // AddToChart-детект); иначе Main рисуется на всю зону без полосы.
            let show_tabs = containers
                .iter()
                .any(|c| matches!(c.kind, ContainerKind::Chart { .. }));
            let tabs_h = if show_tabs { CHART_TABS_H } else { 0.0 };
            if show_tabs && central.is_positive() {
                egui::Area::new(egui::Id::new("chart-tabs"))
                    .fixed_pos(central.min)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        ui.add_space(3.0);
                        let mut active_x: Option<(f32, f32)> = None;
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (i, c) in containers.iter().enumerate() {
                                // Метка вкладки (в доке, не откреплено): «номер»
                                // или «номер-ядро» (без группы — мы уже в её окне).
                                let label = match c.kind {
                                    ContainerKind::Main => "Main".to_string(),
                                    ContainerKind::Chart { num, core: None } => num.to_string(),
                                    ContainerKind::Chart {
                                        num,
                                        core: Some(cid),
                                    } => {
                                        let cn = cores
                                            .iter()
                                            .find(|ci| ci.id == cid)
                                            .map(|ci| ci.name.as_str())
                                            .unwrap_or("");
                                        format!("{num}-{cn}")
                                    }
                                };
                                let sel = i == active_container;
                                // Нумерованные вкладки можно ПОТЯНУТЬ → открепить
                                // в окно; Main не открепляется. Бейдж-счётчик — не
                                // для Main (там фулскрин, счёт окон не нужен).
                                let draggable = matches!(c.kind, ContainerKind::Chart { .. });
                                let count = if draggable { c.panes.len() } else { 0 };
                                let resp = chart_tab(ui, &label, count, sel, draggable);
                                if sel {
                                    active_x = Some((resp.rect.left(), resp.rect.right()));
                                }
                                if resp.clicked() {
                                    switch_to = Some(i);
                                }
                                if draggable && resp.drag_stopped() {
                                    detach_chart = Some(i);
                                }
                            }
                        });
                        // Бейзлайн + акцентное подчёркивание активной вкладки.
                        let y = central.min.y + tabs_h - 1.0;
                        let p = ui.painter();
                        p.line_segment(
                            [egui::pos2(central.min.x, y), egui::pos2(central.max.x, y)],
                            egui::Stroke::new(1.0, crate::shell::theme::BORDER),
                        );
                        if let Some((x0, x1)) = active_x {
                            p.line_segment(
                                [egui::pos2(x0, y), egui::pos2(x1, y)],
                                egui::Stroke::new(2.0, crate::shell::theme::ACCENT),
                            );
                        }
                    });
            }

            // Крестики закрытия — по одному на видимую панель активного
            // контейнера (правый-верхний угол каждой полосы; полосы сдвинуты на
            // высоту полосы вкладок).
            let c = &containers[active_container];
            if !c.is_empty() && central.is_positive() {
                let band_rect = Rect {
                    x: central.min.x,
                    y: central.min.y + tabs_h,
                    w: central.width(),
                    h: (central.height() - tabs_h).max(1.0),
                };
                for (idx, r) in c.layout(band_rect) {
                    egui::Area::new(egui::Id::new(("pane-close", idx)))
                        .fixed_pos(egui::pos2(r.x + r.w - 26.0, r.y + 6.0))
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            let btn = egui::Button::new(
                                egui::RichText::new("✕")
                                    .size(13.0)
                                    .color(crate::shell::theme::MUTED),
                            )
                            .fill(egui::Color32::from_black_alpha(96))
                            .min_size(egui::vec2(20.0, 20.0));
                            if ui.add(btn).clicked() {
                                close_pane = Some(idx);
                            }
                        });
                }
            }
        });

        // Зона дока детектов в физ. пикселях — для форса кадров под курсором.
        self.detects_area = if detects_rect.is_positive() {
            (
                detects_rect.min.x * ppp,
                detects_rect.min.y * ppp,
                detects_rect.width() * ppp,
                detects_rect.height() * ppp,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        // Кнопки тулбара (масштаб Y, live/пауза) действуют на ВЕСЬ активный
        // контейнер — на все его панели, а не только на фокус-панель.
        if set_scale.is_some() || set_follow.is_some() {
            for p in &mut self.containers[self.active_container].panes {
                if let Some(action) = set_scale {
                    match action {
                        crate::dock::ScaleAction::Auto => p.chart.view.set_auto(),
                        crate::dock::ScaleAction::Percent(pct) => p.chart.view.set_scale_percent(pct),
                    }
                }
                if let Some(f) = set_follow {
                    if f {
                        p.chart.view.resume_live(now_ms); // к «сейчас», сброс удержания
                    } else {
                        p.chart.view.follow = false; // пауза: вид замораживается
                    }
                }
            }
        }

        // Открытие монеты по детекту → панель в главном контейнере, фулскрин.
        // Подписку сделает session.set_open на следующем тике app-цикла.
        if let Some((core, mkt)) = open_detect.take() {
            let (dev, fmt, epoch) = (&self.gpu.device, self.gpu.format, self.epoch_ms);
            self.containers[0].open_manual(core, &mkt, dev, fmt, epoch);
            self.active_container = 0;
            // Открываем монету СРАЗУ на её цене (сброс Y) и в лайве.
            let c = &mut self.containers[0];
            if let Mode::Fullscreen(i) = c.mode {
                if let Some(p) = c.panes.get_mut(i) {
                    p.chart.view.resume_live(now_ms);
                    p.chart.view.reset_y();
                }
            }
            layout_changed = true;
        }
        // Закрытие панели по её крестику (удаляем; контейнер может опустеть).
        if let Some(idx) = close_pane {
            self.active_mut().remove(idx);
            layout_changed = true;
        }
        // Переключение контейнера по верхней вкладке.
        if let Some(i) = switch_to {
            if i < self.containers.len() {
                self.active_container = i;
                layout_changed = true;
            }
        }
        // Запрос на откреп вкладки в окно (создаёт App — нужен event_loop).
        let _ = close_chart;
        // Кнопка «Отчёты» в шапке теперь выбирает вкладку «Отчёт» дока (а не
        // открывает отдельное окно): отчёт живёт во вкладке, окном становится
        // только при откреплении. Если вкладка откреплена — App сфокусит окно.
        if reports_clicked {
            self.workspace.dock.tab = crate::dock::DockTab::Report;
            layout_changed = true;
        }

        // Полоса чарт-вкладок (видна, когда есть нумерованные вкладки) съедает
        // верх центральной зоны.
        let tabs_h = if self
            .containers
            .iter()
            .any(|c| matches!(c.kind, ContainerKind::Chart { .. }))
        {
            CHART_TABS_H
        } else {
            0.0
        };
        self.egui_area = if central.is_positive() {
            Rect {
                x: central.min.x * ppp,
                y: (central.min.y + tabs_h) * ppp,
                w: (central.width() * ppp).max(1.0),
                h: ((central.height() - tabs_h) * ppp).max(1.0),
            }
        } else {
            Rect {
                x: 0.0,
                y: HEADER_H * ppp,
                w: resolution[0],
                h: (resolution[1] - (HEADER_H + STATUS_H) * ppp).max(1.0),
            }
        };

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (tex_id, delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *tex_id, delta);
        }
        self.egui_renderer
            .update_buffers(&self.gpu.device, &self.gpu.queue, encoder, &tris, screen);

        self.egui_tris = Some(tris);
        self.egui_ppp = full_output.pixels_per_point;
        self.egui_wants_repaint = self.egui_ctx.has_requested_repaint();
        self.last_chrome_sig = sig;
        self.last_egui_run = now;
        self.egui_cache_size = size_now;
        self.egui_dirty = false;

        ChromeOut {
            gear_clicked,
            strategies_clicked,
            detach: detach_req,
            repin: repin_req,
            detach_chart,
            layout_changed,
            to_free: full_output.textures_delta.free,
        }
    }
}
