//! Публичный хэндл движка чарта `ChartEngine`: open/scale/follow/prune/pin/layout,
//! управление present и синхронизация с панелями. Вынесено из `mod.rs` (impl-блок);
//! сама структура `ChartEngine` объявлена в `mod.rs` (дочерний модуль видит её поля).

use super::*;

fn hex3(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
}

fn initial_palette_from_theme(theme: &ChartTheme) -> moon_ui::MoonPalette {
    let panel = hex3(theme.panel_bg);
    let chart_bg = hex3(theme.bg);
    let border = hex3(theme.grid);
    let accent = hex3(theme.cross);
    let green = hex3(theme.book_bid);
    let orange = hex3(theme.book_ask);
    moon_ui::MoonPalette {
        shell: panel,
        shell_high: panel,
        panel,
        panel_high: panel,
        chart_bg,
        border,
        text: accent,
        text_soft: border,
        text_muted: border,
        table_head: panel,
        table_body: panel,
        table_selected: panel,
        green,
        red: orange,
        orange,
        amber: accent,
        blue: accent,
        accent,
        yellow: accent,
    }
}

impl ChartEngine {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        Self::new_kind(epoch, theme, ContainerKind::Main)
    }

    pub fn new_kind(epoch: f64, theme: ChartTheme, kind: ContainerKind) -> Self {
        let container = Rc::new(RefCell::new(Container::new(kind)));
        let state = Rc::new(RefCell::new(RenderState {
            panes: Vec::new(),
            needs_present: true,
            base_dirty: true,
            last_present_ms: 0.0,
            target_present_interval_ms: 1000.0 / 60.0,
            camera_shift_window_start_ms: 0.0,
            camera_shift_count: 0,
            camera_shift_hz: 0.0,
            last_gpu_prepare_generation: 0,
            text_runs: Vec::new(),
            text_run_cursor: 0,
            firetest_text_labels: Vec::new(),
            firetest_text_runs: Vec::new(),
            firetest_force_present: false,
            ui_palette: initial_palette_from_theme(&theme),
            slot_origin: [0.0, 0.0],
            cursor: None,
            cursor_color: {
                let mut c = rgb4(theme.cross);
                c[3] = theme.cross_alpha;
                c
            },
            cursor_thickness: theme.cross_thickness.max(1.0),
            pixel_scale: 1.0,
            #[cfg(windows)]
            scissor_rs: None,
            #[cfg(windows)]
            scissor_dev: std::ptr::null_mut(),
            #[cfg(windows)]
            window_bg: background::BackgroundLayer::new(background::SPLASH_PNG),
            #[cfg(windows)]
            window_bg_color: rgb4(theme.bg),
            #[cfg(windows)]
            base_cache: base::BaseCache::new(),
        }));
        let data = Rc::new(RefCell::new(ChartDataState::new(
            container.clone(),
            state.clone(),
            theme.clone(),
        )));
        let canvas = GpuCanvasHandle::new(ChartCanvasDriver {
            state: state.clone(),
            data: Rc::downgrade(&data),
        });
        Self {
            container,
            state,
            data,
            canvas,
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            present_rate_hz: 60.0,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
        }
    }

    pub fn data_handle(&self) -> ChartDataHandle {
        ChartDataHandle {
            inner: Rc::downgrade(&self.data),
        }
    }

    pub fn set_market_source(&mut self, source: Option<MarketDataSource>) -> bool {
        self.data.borrow_mut().set_market_source(source)
    }

    /// Обычный GPUI element, который владеет bounds/clip/lifetime через дерево.
    /// В отличие от старого window-global pass, он сам исчезает при скрытии вкладки
    /// и переезжает при detach вместе с `ChartPanel`.
    pub fn canvas(&self) -> gpui::GpuCanvas {
        gpui::gpu_canvas(self.canvas.clone())
    }

    /// Размер слота чарта (девайс-px). Combo сам пересоздаёт битмап при смене размера.
    pub fn resize(&mut self, w: u32, h: u32) {
        let next_w = w.max(1);
        let next_h = h.max(1);
        if self.w == next_w && self.h == next_h {
            return;
        }
        self.w = next_w;
        self.h = next_h;
        let mut data = self.data.borrow_mut();
        data.w = self.w;
        data.h = self.h;
        data.mark_view_dirty();
    }

    /// Левый-верхний угол слота чарта В ОКНЕ (девайс-px) — для координат own-pass в backbuffer.
    pub fn set_origin(&mut self, x: f32, y: f32) {
        if self.origin == (x, y) {
            return;
        }
        self.origin = (x, y);
        {
            let mut data = self.data.borrow_mut();
            data.origin = self.origin;
            data.mark_view_dirty();
        }
        self.state.borrow_mut().set_slot_origin(x, y);
    }

    pub fn set_present_rate_hz(&mut self, hz: f32) {
        self.present_rate_hz = hz.max(1.0);
        self.data.borrow_mut().present_rate_hz = self.present_rate_hz;
        self.state
            .borrow_mut()
            .set_target_present_rate_hz(self.present_rate_hz);
    }

    pub fn set_ui_palette(&mut self, palette: moon_ui::MoonPalette) {
        let mut state = self.state.borrow_mut();
        if state.ui_palette.panel != palette.panel
            || state.ui_palette.chart_bg != palette.chart_bg
            || state.ui_palette.text_soft != palette.text_soft
            || state.ui_palette.border != palette.border
        {
            state.ui_palette = palette;
            state.needs_present = true;
        }
    }

    pub fn set_cursor(&mut self, cursor: Option<(usize, f32, f32)>) -> bool {
        self.state
            .borrow_mut()
            .set_cursor(cursor.map(|(pane, x, y)| CursorState {
                pane,
                local: [x, y],
            }))
    }

    /// Sync only account/order overlays that still live in `SessionManager`.
    /// Market ticks, price lines and orderbook data are pulled exclusively from
    /// `gpu_canvas.frame()` through `MarketDataSource`.
    pub fn sync_orders_if_visible(&mut self, session: &SessionManager, force: bool) -> bool {
        self.data
            .borrow_mut()
            .sync_orders_if_visible(session, force)
    }

    pub fn notify_signature(&self, session: &SessionManager) -> u64 {
        self.data.borrow().notify_signature(session)
    }

    pub fn set_scene_visible(&mut self, visible: bool) {
        self.data.borrow_mut().scene_visible = visible;
    }

    pub fn set_last_ppp(&mut self, ppp: f32) {
        let ppp = ppp.max(0.1);
        self.data.borrow_mut().last_ppp = ppp;
        self.state.borrow_mut().set_pixel_scale(ppp);
    }

    // ── Настройки (порт из старого chart.rs::ChartGpu) ───────────────────────────

    pub fn set_theme(&mut self, theme: ChartTheme) -> bool {
        if self.theme != theme {
            let mut cursor_color = rgb4(theme.cross);
            cursor_color[3] = theme.cross_alpha;
            self.state
                .borrow_mut()
                .set_cursor_style(cursor_color, theme.cross_thickness);
            self.theme = theme;
            let mut data = self.data.borrow_mut();
            data.theme = self.theme.clone();
            data.mark_view_dirty();
            true
        } else {
            false
        }
    }

    pub fn set_orders(&mut self, orders: OrdersStyle) -> bool {
        if self.orders != orders {
            self.orders = orders;
            let mut data = self.data.borrow_mut();
            data.orders = self.orders.clone();
            data.mark_view_dirty();
            drop(data);
            for pr in &mut self.state.borrow_mut().panes {
                pr.last_orders_rev = u64::MAX;
            }
            true
        } else {
            false
        }
    }

    /// Масштаб цены (Y) ко ВСЕМ панелям. None=Авто. Запоминается в контейнере.
    pub fn set_scale(&mut self, pct: Option<f32>) -> bool {
        if self.scale == pct {
            return false;
        }
        self.scale = pct;
        self.container.borrow_mut().set_scale(pct);
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    /// Глобальный live-follow из тулбара (Live/Пауза) ко ВСЕМ панелям. Реагирует ТОЛЬКО
    /// на смену самого глобального флага (явный клик). НЕ на производное состояние от пана
    /// одной панели: иначе пан одной монеты в Tiled гасил бы live у соседних, которых не
    /// трогали (их view.follow перетирался). Пан/rejoin отдельной панели живут в её
    /// view.follow; сюда уже сведённое значение прилетает через sync_follow_from_views, и
    /// если глобальный флаг не изменился — выходим, панели не трогаем.
    pub fn set_follow(&mut self, follow: bool, now_ms: f64) -> bool {
        if self.follow == follow {
            return false;
        }
        self.follow = follow;
        self.data.borrow_mut().follow = follow;
        for p in &mut self.container.borrow_mut().panes {
            if follow {
                // Возобновляем live только у панелей, которые НЕ следовали (явный Live из
                // тулбара): уже живые панели не трогаем — их окно/зум не сбрасываем.
                if !p.view.follow {
                    p.view.resume_live(now_ms);
                    p.view.reset_default_window_on_next_prepare();
                }
            } else {
                // Явное выключение live (кнопка) — без авто-возврата по таймеру (П.9).
                p.view.set_manual_persistent();
            }
        }
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Ближайший дедлайн авто-возврата в live среди панелей (для арминга таймера, П.9).
    pub fn next_auto_live_deadline_ms(&self) -> Option<f64> {
        self.container
            .borrow()
            .panes
            .iter()
            .filter_map(|p| p.view.auto_live_deadline_ms())
            .reduce(f64::min)
    }

    /// Тик авто-возврата в live: панели, у которых истёк ручной hold, снова якорятся к
    /// «сейчас». Возвращает true, если хоть одна возобновила live (нужен кадр/нотифай).
    pub fn tick_auto_live(&mut self, now_ms: f64) -> bool {
        let mut resumed = false;
        for p in &mut self.container.borrow_mut().panes {
            resumed |= p.view.tick_auto_live(now_ms);
        }
        if resumed {
            self.data.borrow_mut().mark_view_dirty();
            self.sync_follow_from_views();
        }
        resumed
    }

    pub fn sync_follow_from_views(&mut self) -> bool {
        let container = self.container.borrow();
        let follow = if container.panes.is_empty() {
            self.follow
        } else {
            container.panes.iter().all(|p| p.view.follow)
        };
        drop(container);
        if self.follow == follow {
            false
        } else {
            self.follow = follow;
            self.data.borrow_mut().follow = follow;
            true
        }
    }

    /// Открыть монету (фулскрин-панель).
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container
            .borrow_mut()
            .open_manual(core, market, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// AddToChart: добавить монету авто-панелью (Tiled) с TTL.
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container
            .borrow_mut()
            .push_auto(core, market, now_ms, ttl_ms, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Убрать истёкшие AddToChart-панели. True — если что-то удалили.
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        let changed = self.container.borrow_mut().prune_ttl(now_ms);
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    #[allow(dead_code)]
    pub fn has_ttl_panes(&self) -> bool {
        self.container.borrow().has_ttl_panes()
    }

    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.container.borrow().next_ttl_deadline_ms()
    }

    pub fn with_container_mut<R>(&mut self, f: impl FnOnce(&mut Container) -> R) -> R {
        let out = f(&mut self.container.borrow_mut());
        self.data.borrow_mut().mark_view_dirty();
        out
    }

    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        let removed = self.container.borrow_mut().remove_pane(idx);
        if removed.is_some() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.container.borrow().uses_market(core, market)
    }

    /// П.2: можно ли приколоть панель idx (только AddToChart с TTL).
    pub fn pane_is_pinnable(&self, idx: usize) -> bool {
        self.container.borrow().is_pinnable(idx)
    }

    pub fn pane_pinned(&self, idx: usize) -> bool {
        self.container.borrow().is_pinned(idx)
    }

    /// Переключить пин панели idx (отмена/возврат авто-закрытия по TTL). True — если изменили.
    pub fn toggle_pane_pin(&mut self, idx: usize) -> bool {
        let changed = self.container.borrow_mut().toggle_pin(idx).is_some();
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        let removed = self.container.borrow_mut().clear_panes();
        if !removed.is_empty() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    /// Core/market активной (фулскрин/первой) панели.
    pub fn active_target(&self) -> Option<(CoreId, String)> {
        let container = self.container.borrow();
        let idx = match container.mode {
            Mode::Fullscreen(i) => i,
            Mode::Tiled => 0,
        };
        container.panes.get(idx).map(|p| (p.core, p.market.clone()))
    }

    /// Рынок активной (фулскрин/первой) панели — для подписи вкладки.
    pub fn active_market(&self) -> Option<String> {
        self.active_target().map(|(_, market)| market)
    }

    /// Force the next prepare to rebuild resident GPU history from MoonProto.
    /// Does not change the visible time window: viewport scale and retained
    /// data capacity are independent.
    pub fn force_history_reupload(&mut self) {
        let mut st = self.state.borrow_mut();
        for pr in &mut st.panes {
            pr.history_cursor.reset();
            pr.resident_left_rel = f32::NAN;
            pr.cached_tick_price = None;
            pr.scan_cam_px = i64::MIN;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        st.base_dirty = true;
        self.data.borrow_mut().mark_view_dirty();
    }

    pub fn pane_count(&self) -> usize {
        self.container.borrow().panes.len()
    }

    /// Снимки осей ПО ВИДИМЫМ ПАНЕЛЯМ: (индекс, прямоугольник девайс-px, снимок). Звать ПОСЛЕ prepare.
    pub fn axis_panes(&self, tz_offset_sec: i64) -> Vec<(usize, Rect, AxisSnapshot)> {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let container = self.container.borrow();
        container
            .layout(area)
            .into_iter()
            .filter_map(|(idx, rect)| {
                let v = &container.panes.get(idx)?.view;
                Some((
                    idx,
                    rect,
                    AxisSnapshot {
                        px_per_ms: v.px_per_ms,
                        right_margin_frac: v.right_margin_frac,
                        render_center: v.render_center,
                        render_range: v.render_range,
                        epoch_ms: v.epoch_ms,
                        right_time_ms: v.right_time_ms,
                        tz_offset_sec,
                    },
                ))
            })
            .collect()
    }
}
