//! Подготовка данных чарта per pane (`impl ChartDataState`): главный `prepare`
//! (БЕЗ рисования) — чтение истории/стакана/ордеров, заливка GPU-буферов, авто-Y,
//! сигнатуры изменений. Вынесено из `mod.rs`; структура `ChartDataState` объявлена там.

use super::*;

impl ChartDataState {
    pub(super) fn new(
        container: Rc<RefCell<Container>>,
        render: Rc<RefCell<RenderState>>,
        theme: ChartTheme,
    ) -> Self {
        Self {
            container,
            render,
            theme,
            orders: OrdersStyle::default(),
            follow: true,
            present_rate_hz: 60.0,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
            scene_visible: false,
            market_source: None,
            last_frame_tick_ms: 0.0,
            present_rate_candidate_hz: 0.0,
            present_rate_candidate_hits: 0,
            last_ppp: 1.0,
            last_order_sig: u64::MAX,
            last_prepared_market_sig: u64::MAX,
            last_source_market_sig: u64::MAX,
            view_dirty: true,
        }
    }

    pub(super) fn notify_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        if let Some(source) = &self.market_source {
            sig = self.market_signature(source);
        }
        sig.wrapping_mul(31)
            .wrapping_add(self.order_signature(session))
    }

    pub(super) fn order_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        for p in &self.container.borrow().panes {
            if let Some(core_st) = session.store().core(p.core) {
                sig = sig.wrapping_mul(31).wrapping_add(core_st.orders_rev);
            }
        }
        sig
    }

    pub(super) fn sync_orders_if_visible(&mut self, session: &SessionManager, force: bool) -> bool {
        if !self.scene_visible {
            return false;
        }
        let sig = self.order_signature(session);
        if !force && sig == self.last_order_sig {
            return false;
        }
        crate::diag::bump(&crate::diag::CHART_PREPARE);
        let changed = self.sync_orders_from_session(session, force);
        self.last_order_sig = sig;
        changed
    }

    pub(super) fn market_signature(&self, source: &MarketDataSource) -> u64 {
        let mut sig = 0u64;
        for p in &self.container.borrow().panes {
            source.with_market_view(p.core, &p.market, |data| {
                if let Some(v) = data {
                    sig = sig
                        .wrapping_mul(31)
                        .wrapping_add(v.ticks_rev)
                        .wrapping_add(v.price_lines_rev)
                        .wrapping_add(v.book_rev);
                }
            });
        }
        sig
    }

    pub(super) fn source_market_signature(&self, source: &MarketDataSource) -> u64 {
        let container = self.container.borrow();
        if container.panes.is_empty() {
            return 0;
        }

        let mut sig = 0xcbf29ce484222325;
        let mut push_pane = |idx: usize| {
            let Some(pane) = container.panes.get(idx) else {
                return;
            };
            sig = mix_sig(sig, pane.core);
            sig = mix_sig(sig, str_sig(&pane.market));
            if let Some((provider, revision)) = source.snapshot_revision(pane.core) {
                sig = mix_sig(sig, provider);
                sig = mix_sig(sig, revision);
            } else {
                let store_revision = source.with_market_view(pane.core, &pane.market, |data| {
                    data.map(|v| {
                        v.ticks_rev
                            .wrapping_mul(31)
                            .wrapping_add(v.price_lines_rev)
                            .wrapping_mul(31)
                            .wrapping_add(v.book_rev)
                    })
                    .unwrap_or(0)
                });
                sig = mix_sig(sig, store_revision);
            }
        };

        match container.mode {
            Mode::Fullscreen(idx) => push_pane(idx.min(container.panes.len() - 1)),
            Mode::Tiled => {
                for idx in 0..container.panes.len() {
                    push_pane(idx);
                }
            }
        }
        sig
    }

    pub(super) fn refresh_visible_markets(&self, source: &MarketDataSource) -> bool {
        let container = self.container.borrow();
        if container.panes.is_empty() {
            return false;
        }
        match container.mode {
            Mode::Fullscreen(idx) => {
                let Some(pane) = container.panes.get(idx.min(container.panes.len() - 1)) else {
                    return false;
                };
                source.refresh_market(pane.core, &pane.market)
            }
            Mode::Tiled => source.refresh_markets(
                container
                    .panes
                    .iter()
                    .map(|pane| (pane.core, pane.market.as_str())),
            ),
        }
    }

    pub(super) fn mark_view_dirty(&mut self) {
        self.view_dirty = true;
    }

    /// Применить геометрию слота из bounds канваса (логич. px) к движку: размер/origin/pixel-scale.
    /// Источник — `GpuFrameInfo` (форк), синхронно в `frame()` → own-pass всегда в актуальном слоте.
    fn apply_slot_geometry(&mut self, info: &GpuFrameInfo) {
        if info.bounds.is_empty() {
            return;
        }
        let sf = info.scale_factor.max(0.1);
        let w = (f32::from(info.bounds.size.width) * sf).round().max(1.0) as u32;
        let h = (f32::from(info.bounds.size.height) * sf).round().max(1.0) as u32;
        let ox = f32::from(info.bounds.origin.x) * sf;
        let oy = f32::from(info.bounds.origin.y) * sf;
        if self.w != w || self.h != h {
            self.w = w;
            self.h = h;
            self.mark_view_dirty();
        }
        if self.origin != (ox, oy) {
            self.origin = (ox, oy);
            self.mark_view_dirty();
        }
        self.last_ppp = sf;
        let mut st = self.render.borrow_mut();
        st.set_slot_origin(ox, oy); // self-guard: dirty/present только при смене
        st.set_pixel_scale(sf);
    }

    pub(super) fn set_market_source(&mut self, source: Option<MarketDataSource>) -> bool {
        let changed = match (&self.market_source, &source) {
            (Some(a), Some(b)) => !a.ptr_eq(b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.market_source = source;
            self.view_dirty = true;
        }
        changed
    }

    pub(super) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        // Геометрия слота — СИНХРОННО из info.bounds (форк отдаёт реальные bounds канваса этого
        // кадра, ДО present). Применяем до pull/sync, чтобы own-pass рисовал в текущем слоте без
        // лага probe→notify→render→present (1–2 кадра): иначе при рефлоу стека освободившийся/
        // сдвинутый слот кадр-два мигал clear'ом окна.
        self.apply_slot_geometry(&info);
        let now_ms = now_unix_ms();
        if self.observe_present_rate(now_ms) {
            if let Some(source) = self.market_source.clone() {
                crate::diag::bump(&crate::diag::CHART_PREPARE);
                self.sync_from_market_source(&source);
            } else {
                self.view_dirty = true;
            }
        }
        if self.pull_market_source_if_visible() {
            crate::diag::bump(&crate::diag::CHART_PREPARE);
        }
        self.render.borrow_mut().frame(info)
    }

    pub(super) fn observe_present_rate(&mut self, now_ms: f64) -> bool {
        let prev_tick_ms = std::mem::replace(&mut self.last_frame_tick_ms, now_ms);
        if prev_tick_ms <= 0.0 {
            return false;
        }
        let dt_ms = now_ms - prev_tick_ms;
        if !(2.0..=40.0).contains(&dt_ms) {
            self.present_rate_candidate_hits = 0;
            return false;
        }
        let sample_hz = (1000.0 / dt_ms).round().clamp(30.0, 360.0) as f32;
        if (sample_hz - self.present_rate_hz).abs() < 0.5 {
            self.present_rate_candidate_hits = 0;
            self.present_rate_candidate_hz = 0.0;
            return false;
        }
        if (sample_hz - self.present_rate_candidate_hz).abs() < 0.5 {
            self.present_rate_candidate_hits = self.present_rate_candidate_hits.saturating_add(1);
        } else {
            self.present_rate_candidate_hz = sample_hz;
            self.present_rate_candidate_hits = 1;
        }
        if self.present_rate_candidate_hits < 6 {
            return false;
        }
        self.present_rate_candidate_hits = 0;
        self.present_rate_hz = sample_hz;
        self.render
            .borrow_mut()
            .set_target_present_rate_hz(self.present_rate_hz);
        true
    }

    pub(super) fn pull_market_source_if_visible(&mut self) -> bool {
        if !self.scene_visible {
            return false;
        }
        let Some(source) = self.market_source.clone() else {
            return false;
        };
        let source_sig = self.source_market_signature(&source);
        if !self.view_dirty && source_sig == self.last_source_market_sig {
            return false;
        }
        if self.container.borrow().panes.is_empty() {
            self.last_source_market_sig = source_sig;
            return false;
        }
        let source_changed = source_sig != self.last_source_market_sig;
        let pulled = self.refresh_visible_markets(&source);
        let sig = self.market_signature(&source);
        if !self.view_dirty && !source_changed && !pulled && sig == self.last_prepared_market_sig {
            self.last_source_market_sig = source_sig;
            return false;
        }
        self.sync_from_market_source(&source);
        self.last_source_market_sig = self.source_market_signature(&source);
        true
    }

    pub(super) fn sync_orders_from_session(
        &mut self,
        session: &SessionManager,
        force: bool,
    ) -> bool {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.borrow().layout(area);
        let now = now_unix_ms();
        let mut st = self.render.borrow_mut();
        let mut container = self.container.borrow_mut();
        let mut pixels_changed = false;
        // Смена числа панелей (в т.ч. удаление последней монеты → пусто) обязана пометить
        // base_dirty: иначе base-кэш продолжит блитить СТАРЫЙ чарт сквозь пустой слот (логотип
        // прозрачный). Зеркалит проверку в sync_from_market_source.
        if st.panes.len() != container.panes.len() {
            pixels_changed = true;
        }
        st.panes.resize_with(container.panes.len(), PaneRender::new);

        for (idx, _) in &layout {
            let pane = &mut container.panes[*idx];
            let pr = &mut st.panes[*idx];
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
                pixels_changed = true;
            }
            // Имя ядра для угловой подписи: резолвим тут — только здесь под рукой `session`.
            // Меняется редко (смена ядра панели), поэтому флагаем present лишь при изменении.
            let core_name = session
                .sessions()
                .iter()
                .find(|s| s.id == pane.core)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if pr.core_name != core_name {
                pr.core_name = core_name;
                pixels_changed = true;
            }
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_orders_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }

            let order_price = session
                .store()
                .core(pane.core)
                .and_then(|core_st| core_st.order_lines.buy_sell_range(&pane.market));
            if pr.cached_order_price != order_price {
                pr.cached_order_price = order_price;
                self.view_dirty = true;
                pixels_changed = true;
            }

            if let Some(core_st) = session.store().core(pane.core) {
                if force || pr.last_orders_rev != core_st.orders_rev {
                    let mut hlines = Vec::new();
                    let mut segs = Vec::new();
                    let mut markers = Vec::new();
                    let mut zones = Vec::new();
                    moon_chart::build_order_geometry(
                        &core_st.order_lines,
                        &pane.market,
                        &self.orders,
                        pane.view.epoch_ms,
                        now,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.0,
                        &mut zones,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    pr.layers.set_userdata(&zones, &hlines, &segs, &markers);
                    pr.last_orders_rev = core_st.orders_rev;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else if force || pr.last_orders_rev != u64::MAX {
                pr.layers.set_userdata(&[], &[], &[], &[]);
                pr.last_orders_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.last_device_gen = device_gen;
        }

        if pixels_changed {
            st.base_dirty = true;
        }
        if pixels_changed {
            st.needs_present = true;
        }
        pixels_changed
    }

    pub(super) fn sync_from_market_source(&mut self, source: &MarketDataSource) {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.borrow().layout(area);
        let now = now_unix_ms();
        let res = [self.w as f32, self.h as f32];
        let mut st = self.render.borrow_mut();
        let mut container = self.container.borrow_mut();
        let mut pixels_changed = false;
        #[cfg(windows)]
        {
            let next_bg_color = rgb4(self.theme.bg);
            if st.window_bg_color != next_bg_color {
                st.window_bg_color = next_bg_color;
                pixels_changed = true;
            }
        }
        let was_active: Vec<bool> = st.panes.iter().map(|pane| pane.active).collect();
        if st.panes.len() != container.panes.len() {
            pixels_changed = true;
        }
        st.panes.resize_with(container.panes.len(), PaneRender::new);
        for pr in &mut st.panes {
            pr.active = false;
        }
        for (idx, rect) in &layout {
            let pane = &mut container.panes[*idx];
            let pr = &mut st.panes[*idx];
            if !was_active.get(*idx).copied().unwrap_or(false) {
                pixels_changed = true;
                pr.gpu_prepare_dirty = true;
            }
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
                pixels_changed = true;
            }
            let next_pane_bounds = [
                self.origin.0 + rect.x,
                self.origin.1 + rect.y,
                rect.w.max(1.0),
                rect.h.max(1.0),
            ];
            if pr.pane_bounds != next_pane_bounds {
                pr.pane_bounds = next_pane_bounds;
                pixels_changed = true;
            }
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_book_rev = u64::MAX;
                pr.last_orders_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let price_axis_w = moon_chart::PRICE_AXIS_W * self.last_ppp;
            let time_axis_h = moon_chart::TIME_AXIS_H * self.last_ppp;
            let plot_h = (rect.h - time_axis_h).max(1.0);
            // П.3: при узком графике стакан не должен съедать половину. База — GLASS_ZONE_PX
            // (ограничена половиной слота). Если ширина графика при базовом стакане < 2× самого
            // стакана (узко), сжимаем стакан до 0.8× зоны, отдавая место графику. В обычном
            // (широком) режиме ширина стакана не меняется.
            let glass_cap = rect.w * 0.5;
            let glass_base = moon_chart::GLASS_ZONE_PX.min(glass_cap);
            let chart_w_base = rect.w - price_axis_w - glass_base;
            let glass_w = if chart_w_base < glass_base * 2.0 {
                (moon_chart::GLASS_ZONE_PX * 0.8).min(glass_cap)
            } else {
                glass_base
            };
            let chart_area = Rect {
                x: rect.x + price_axis_w,
                y: rect.y,
                w: (rect.w - price_axis_w - glass_w).max(1.0),
                h: plot_h,
            };
            let glass_area = Rect {
                x: rect.x + (rect.w - glass_w).max(1.0),
                y: rect.y,
                w: glass_w,
                h: plot_h,
            };
            pane.view
                .ensure_default_window(chart_area.w, self.present_rate_hz);
            pane.view.follow_edge(now, now);
            let (view_time0, window_ms) = pane.view.visible_x(chart_area.w);
            let cam_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let marker_margin = pane.view.marker_half_px / pane.view.px_per_ms.max(1e-6);
            let history_prefetch = (window_ms * 0.20).max(marker_margin);
            let history_from = view_time0 - history_prefetch;
            let history_to = view_time0 + window_ms + history_prefetch;
            let scan_price = device_lost || cam_px != pr.scan_cam_px;
            let force_history_reset = device_lost
                || pr.resident_left_rel.is_nan()
                || history_from < pr.resident_left_rel
                || (!pane.view.follow && scan_price);
            let mut history = source.read_chart_history_into(
                pane.core,
                &pane.market,
                pane.view.epoch_ms,
                history_from,
                history_to,
                force_history_reset,
                scan_price,
                &mut pr.history_cursor,
                &mut pr.history_buffers,
            );
            let capacity_changed = history.as_ref().is_some_and(|h| {
                (h.combo_capacity > 0 && h.combo_capacity != pr.combo_cross_capacity)
                    || (h.price_line_capacity > 0
                        && h.price_line_capacity != pr.combo_price_line_capacity)
            });
            if capacity_changed && history.as_ref().is_some_and(|h| !h.combo_reset) {
                history = source.read_chart_history_into(
                    pane.core,
                    &pane.market,
                    pane.view.epoch_ms,
                    history_from,
                    history_to,
                    true,
                    scan_price,
                    &mut pr.history_cursor,
                    &mut pr.history_buffers,
                );
            }
            let last_price = if let Some(history) = history {
                if scan_price {
                    pr.cached_tick_price = history.tick_price_range;
                    pr.scan_cam_px = cam_px;
                }
                let last_price = history.last_price;
                if capacity_changed || history.combo_reset {
                    pr.combo_cross_capacity = history.combo_capacity;
                    pr.combo_price_line_capacity = history.price_line_capacity;
                    pr.layers
                        .set_combo_capacity(history.combo_capacity, history.price_line_capacity);
                }
                if history.combo_reset {
                    fill_cross_upload(
                        &pr.history_buffers.ticks,
                        pane.view.epoch_ms,
                        &mut pr.cross_upload,
                    );
                    pr.layers.reset_combo(std::mem::take(&mut pr.cross_upload));
                    pr.resident_left_rel = history.combo_left_rel_ms.unwrap_or(history_from);
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                } else if !pr.history_buffers.ticks.is_empty() {
                    fill_cross_upload(
                        &pr.history_buffers.ticks,
                        pane.view.epoch_ms,
                        &mut pr.cross_upload,
                    );
                    pr.layers.append_combo(&pr.cross_upload);
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if history.price_lines_changed || history.combo_reset {
                    fill_price_upload(
                        &pr.history_buffers.last_points,
                        pane.view.epoch_ms,
                        &mut pr.last_line_upload,
                    );
                    fill_price_upload(
                        &pr.history_buffers.mark_points,
                        pane.view.epoch_ms,
                        &mut pr.mark_line_upload,
                    );
                    pr.layers
                        .set_price_lines(&pr.last_line_upload, &pr.mark_line_upload);
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if chart_market_diag_due(format!("combo:{}:{}:{}", pane.core, pane.market, idx)) {
                    chart_market_diag(format!(
                        "pane={} core={} market={} provider={} rev={} reset={} ticks={} \
                         price_lines={} clipped={} caught_up={} scan_price={} \
                         window=[{:.1},{:.1}] resident_left={:.1} last_price={:?} bounds={:?}",
                        idx,
                        pane.core,
                        pane.market,
                        history.provider,
                        history.revision,
                        history.combo_reset,
                        pr.history_buffers.ticks.len(),
                        history.price_lines_changed,
                        history.clipped,
                        history.caught_up,
                        scan_price,
                        view_time0,
                        view_time0 + window_ms,
                        pr.resident_left_rel,
                        history.last_price,
                        pr.view.bounds
                    ));
                }
                last_price
            } else {
                if pr.resident_left_rel.is_finite() {
                    pr.layers.reset_combo(Vec::new());
                    pr.layers.set_price_lines(&[], &[]);
                    pr.history_cursor.reset();
                    pr.resident_left_rel = f32::NAN;
                    pr.cached_tick_price = None;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if scan_price {
                    pr.cached_tick_price = None;
                    pr.scan_cam_px = cam_px;
                }
                source.with_market_view(pane.core, &pane.market, |data| {
                    data.and_then(|d| d.last_price)
                })
            };
            let tick_price = pr.cached_tick_price;
            let visible_price = union_range(
                union_range(tick_price, pr.cached_order_price),
                last_price.map(|p| (p, p)),
            );
            pane.view.update_y(now, plot_h, visible_price, last_price);
            let area_win = Rect {
                x: self.origin.0 + chart_area.x,
                y: self.origin.1 + chart_area.y,
                w: chart_area.w,
                h: chart_area.h,
            };
            let next_view = view::view_gpu(&pane.view, area_win, res);
            if pr.view != next_view {
                pr.view = next_view;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.epoch_ms = pane.view.epoch_ms;
            pr.right_margin_frac = pane.view.right_margin_frac;
            pr.follow = pane.view.follow;
            pr.last_edge_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let (bg_uv_off, bg_uv_scale) = cover_uv(chart_area.w, chart_area.h, 1.0);
            let background_opacity = if CHART_PHOTO_BACKGROUND_ENABLED {
                self.theme.background_opacity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let next_background_params = BackgroundParams {
                dst: pr.view.bounds,
                resolution: res,
                uv_off: bg_uv_off,
                uv_scale: bg_uv_scale,
                opacity: background_opacity,
                _pad: 0.0,
                bg: rgb4(self.theme.bg),
            };
            if pr.background_params != next_background_params {
                pr.background_params = next_background_params;
                pixels_changed = true;
            }
            let next_grid_params = GridParams {
                bounds: pr.view.bounds,
                resolution: res,
                n_vert: 6.0,
                price_to_px: pr.view.price_to_px,
                view_price0: pr.view.view_price0,
                price_interval: moon_chart::axes::nice_interval(
                    pane.view.render_range.max(1e-9),
                    8.0,
                ),
                grid_alpha: self.theme.grid_alpha,
                bg_alpha: if background_opacity > 0.0 { 0.0 } else { 1.0 },
                bg: rgb4(self.theme.bg),
                grid_col: rgb4(self.theme.grid),
            };
            if pr.grid_params != next_grid_params {
                pr.grid_params = next_grid_params;
                pixels_changed = true;
            }
            let glass_win = Rect {
                x: self.origin.0 + glass_area.x,
                y: self.origin.1 + glass_area.y,
                w: glass_area.w,
                h: glass_area.h,
            };
            let next_orderbook_view = view::view_gpu(&pane.view, glass_win, res);
            if pr.orderbook_view != next_orderbook_view {
                pr.orderbook_view = next_orderbook_view;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let next_book_style = BookStyle {
                book_bg: rgb4(self.theme.book_bg),
                bid: rgb4(self.theme.book_bid),
                ask: rgb4(self.theme.book_ask),
                level: [self.theme.book_level_alpha.clamp(0.0, 1.0), 1.5, 0.0, 0.0],
            };
            if pr.book_style != next_book_style {
                pr.book_style = next_book_style;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            source.with_market_view(pane.core, &pane.market, |data| {
                if let Some(d) = data {
                    let half = pane.view.render_range.max(1e-9) * 0.5;
                    let (lo, hi) = (
                        pane.view.render_center - half,
                        pane.view.render_center + half,
                    );
                    let mut diag_levels_len = None;
                    if pr.last_book_rev != d.book_rev
                        || pr.last_book_lo != lo
                        || pr.last_book_hi != hi
                    {
                        let mut levels = Vec::new();
                        d.book.build_instances(lo, hi, &mut levels);
                        diag_levels_len = Some(levels.len());
                        pr.layers.set_orderbook(levels);
                        pr.last_book_rev = d.book_rev;
                        pr.last_book_lo = lo;
                        pr.last_book_hi = hi;
                        pr.gpu_prepare_dirty = true;
                        pixels_changed = true;
                    }
                    if chart_market_diag_due(format!("book:{}:{}:{}", pane.core, pane.market, idx))
                    {
                        chart_market_diag(format!(
                            "pane={} core={} market={} book_rev={} book_len={} levels={:?} \
                             y=[{lo:.8},{hi:.8}] center={:.8} range={:.8} book_bounds={:?}",
                            idx,
                            pane.core,
                            pane.market,
                            d.book_rev,
                            d.book.len(),
                            diag_levels_len,
                            pane.view.render_center,
                            pane.view.render_range,
                            pr.orderbook_view.bounds
                        ));
                    }
                } else if pr.last_book_rev != u64::MAX {
                    pr.layers.set_orderbook(Vec::new());
                    pr.last_book_rev = u64::MAX;
                    pr.last_book_lo = f32::NAN;
                    pr.last_book_hi = f32::NAN;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            });
            let edge_rel = view_time0 + (chart_area.w + glass_w) / pane.view.px_per_ms.max(1e-6);
            if pr.view.pad != edge_rel {
                pr.view.pad = edge_rel;
                pixels_changed = true;
            }
            pr.last_device_gen = device_gen;
            pr.active = true;
        }
        for (idx, was_active) in was_active.into_iter().enumerate() {
            if was_active && !st.panes.get(idx).is_some_and(|pr| pr.active) {
                pixels_changed = true;
            }
        }
        let prev_cursor_params: Vec<CursorParams> =
            st.panes.iter().map(|pr| pr.cursor_params).collect();
        st.sync_cursor_params();
        let cursor_changed = st.cursor.is_some()
            && st
                .panes
                .iter()
                .zip(prev_cursor_params.iter())
                .any(|(pr, prev)| pr.cursor_params != *prev);
        if pixels_changed {
            st.base_dirty = true;
        }
        if pixels_changed || cursor_changed {
            st.needs_present = true;
        }
        drop(container);
        drop(st);
        self.last_prepared_market_sig = self.market_signature(source);
        self.view_dirty = false;
    }
}
