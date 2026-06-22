//! Панель чарта (center DockArea): НАШ own-pass DX11 рендер (через generic-хук gpui) +
//! ввод. Как Dock-панель — отцепляется в окно. Монета — из
//! focus и `Backend.open_request`.
//!
//! Рендер: `ChartEngine.canvas()` отдаёт GPUI `gpu_canvas` ПОД сценой (рисует combo/слои в
//! backbuffer GPUI без readback), `prepare` обновляет вид и заливает новые тики.
//! Текст осей/readout — retained gpu_canvas text; линии перекрестия — native chartdx cursor layer.

use std::collections::HashSet;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonRect,
    Panel, PanelEvent,
};

use rust_i18n::t;

use crate::chartdx::ChartEngine;
use crate::{Backend, axes, input};
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{ChartBucket, ChartTheme, OrdersStyle};
use moon_core::session::CoreId;

#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
fn monitor_refresh_hz() -> u32 {
    unsafe {
        let mut mode = DEVMODEW::default();
        mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode).as_bool()
            && mode.dmDisplayFrequency > 1
        {
            mode.dmDisplayFrequency
        } else {
            60
        }
    }
}

#[cfg(not(windows))]
fn monitor_refresh_hz() -> u32 {
    60
}

fn chart_bootstrap_present_rate_hz() -> f32 {
    let refresh = monitor_refresh_hz().clamp(30, 360);
    refresh as f32
}

const DEBUG_HISTORY_FILL_SPAN_MS: i64 = 3_600_000;

#[derive(Clone, PartialEq)]
struct ChartSettingsSig {
    theme: ChartTheme,
    orders: OrdersStyle,
    follow: bool,
}

fn chart_settings_sig(backend: &Backend) -> ChartSettingsSig {
    let effective = backend.preview.as_ref().unwrap_or(&backend.config);
    ChartSettingsSig {
        theme: effective.theme.clone(),
        orders: effective.orders.clone(),
        follow: backend.follow,
    }
}

pub struct ChartPanel {
    backend: Entity<Backend>,
    chart: ChartEngine,
    input: input::ChartInput,
    market: Option<String>,
    /// Масштаб цены ЭТОЙ вкладки (None = Авто). Теперь ПО-ВКЛАДОЧНЫЙ (не глобальный): правится
    /// своим регулятором (тулбар активной вкладки / шапка выносного окна), применяется в render.
    scale: Option<f32>,
    /// Номер AddToChart-вкладки (None = Main).
    num: Option<u32>,
    /// Рынки, владельцем которых является именно эта chart panel. Backend держит refcount
    /// по всем панелям и строит `desired` из него.
    registered_markets: HashSet<(CoreId, String)>,
    /// Поколение backend registry, в котором были взяты `registered_markets`.
    /// Structural rebuild сбрасывает registry целиком и bump-ит epoch; старые панели после
    /// этого не должны release-ить refs свежих панелей.
    market_ref_epoch: u64,
    /// Сигнатура рыночных данных прошлого кадра — нотифаим только при реальном приходе данных.
    data_sig: u64,
    /// UI-настройки, которые применяются в render. После включения cached dock-панелей
    /// top-down Shell render больше не будит ChartPanel, поэтому изменения должны нотифаить
    /// саму панель.
    settings_sig: ChartSettingsSig,
    /// FastChart: true → плавный кадр по vsync (фокусный чарт); false → адаптивно
    /// (по приходу данных через observe). Main=true, AddToChart=false.
    fast: bool,
    /// Панель реально присутствует в GPUI scene этого окна. Скрытые вкладки не должны гонять
    /// CPU prepare по data observe: их `gpu_canvas` всё равно не будет опрошен/нарисован.
    scene_visible: bool,
    /// Панель сейчас является плиткой Main stack. В этом режиме wheel принадлежит внешнему
    /// ScrollBox-у; fullscreen и AddToChart сохраняют обычный chart zoom.
    main_stack_scroll: bool,
    last_axis_notify_data_sig: u64,
    view_dirty: bool,
    last_adaptive_notify_ms: f64,
    /// Последний scale_factor окна (ставится в render). Нужен data prepare path, у которого
    /// нет window — DPI меняется редко, между сменами берём запомненный.
    last_ppp: f32,
    /// One-shot timer до ближайшего истечения AddToChart TTL. Это time-based dirty,
    /// поэтому он не должен зависеть от backend data observe.
    ttl_timer_armed: bool,
    /// One-shot timer авто-возврата в live после пана (П.9). Тоже time-based: prepare в
    /// покое не тикает (камеру двигает own-pass), поэтому возврат нужен по таймеру.
    auto_live_timer_armed: bool,
    focus: FocusHandle,
}

impl ChartPanel {
    pub fn new(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_main(backend, focus_open, epoch, theme, cx)
    }

    pub fn new_main(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new(epoch, theme);
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let mut market = None;
        let mut registered_markets = HashSet::new();
        if let Some((core, m)) = focus_open {
            chart.open(core, &m);
            market = Some(m.clone());
            registered_markets.insert((core, m.clone()));
            backend.update(cx, |b, _| {
                b.retain_chart_market(core, &m);
            });
        }
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        // UI notify при изменении настроек/редкого текста осей. Частые рыночные данные не
        // идут через notify: gpu_canvas.frame() подтягивает MarketDataSource напрямую.
        // Time-based TTL панелей обслуживает локальный one-shot timer, не backend data observe.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::CHART_OBS_FIRE);
            let now = now_unix_ms();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.notify_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            this.data_sig = sig;
            // Троттл notify. Данные gpu_canvas рисует сам по present (форк), notify нужен лишь
            // для GPUI-оверлея осей, а он идёт top-down → дёргает Orders. Поэтому ≤4 Гц для
            // fast (≥250мс) и ≤1 Гц для addto. Частые GPU data/state обновляет
            // gpu_canvas.frame() без GPUI dirty; notify здесь только для редкого текста осей.
            let floor = if this.fast { 250.0 } else { 1000.0 };
            if sig != this.last_axis_notify_data_sig && now - this.last_adaptive_notify_ms >= floor
            {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_ms = now;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            chart,
            input: input::ChartInput::default(),
            market,
            scale: None,
            num: None,
            registered_markets,
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: true,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            last_ppp: 1.0,
            ttl_timer_armed: false,
            auto_live_timer_armed: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn active_target(&self) -> Option<(CoreId, String)> {
        self.chart.active_target()
    }

    /// AddToChart-вкладка №`num` (наполняется детектами через add_coin). Без `window`: панель
    /// строится из данных, окно ей не нужно (важно для отложенного восстановления откреп-окон).
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new_kind(epoch, theme, ContainerKind::Chart { num, bucket });
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.notify_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            this.data_sig = sig;
            // AddToChart — фоновый график: notify (а с ним top-down перерисовка Orders)
            // ≤1 Гц. Частые GPU data/state обновляет gpu_canvas.frame() без notify;
            // time-based prune делает локальный TTL timer.
            if sig != this.last_axis_notify_data_sig && now - this.last_adaptive_notify_ms >= 1000.0
            {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_ms = now;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            chart,
            input: input::ChartInput::default(),
            market: None,
            scale: None,
            num: Some(num),
            registered_markets: HashSet::new(),
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: false,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            last_ppp: 1.0,
            ttl_timer_armed: false,
            auto_live_timer_armed: false,
            focus: cx.focus_handle(),
        }
    }

    /// Число открытых панелей чарта (для бейджа-счётчика на вкладке).
    pub fn pane_count(&self) -> usize {
        self.chart.pane_count()
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_data_handle(&self) -> crate::chartdx::ChartDataHandle {
        self.chart.data_handle()
    }

    /// Mark whether this panel is part of the currently rendered GPUI scene. This only gates
    /// CPU-side data prepare; the `gpu_canvas` element lifetime is still owned by GPUI scene replay.
    pub fn set_scene_visible(&mut self, visible: bool) {
        self.scene_visible = visible;
        self.chart.set_scene_visible(visible);
    }

    /// Main stack mode scrolls the list of chart tiles. The chart itself must not consume wheel
    /// events there, otherwise the outer ScrollBox cannot move.
    pub fn set_main_stack_scroll(&mut self, enabled: bool) {
        self.main_stack_scroll = enabled;
    }

    pub(crate) fn sync_orders_if_visible(&mut self, cx: &mut Context<Self>, force: bool) {
        if !self.scene_visible {
            return;
        }
        let b = self.backend.read(cx);
        self.data_sig = self.chart.notify_signature(&b.session);
        self.chart.sync_orders_if_visible(&b.session, force);
    }

    /// Поставить масштаб ЭТОЙ вкладки (None=Авто). Применяется в render через `set_scale` движка.
    pub fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale != pct {
            self.scale = pct;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// AddToChart: открыть/продлить монету в этой панели с TTL.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64, cx: &mut Context<Self>) {
        self.release_market_refs_except(Some((core, market)), cx);
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.retain_market_ref(core, market, cx);
        self.view_dirty = true;
        self.arm_ttl_timer(cx);
        // ВАЖНО: notify самой панели. Для вкладки в стрипе её перерисовывает render ChartTabs,
        // но ОТКРЕПЛЁННАЯ панель живёт в своём окне — без notify оно не перерисуется и новая
        // монета не появится (баг «детект пришёл, а графика в откреп-окне нет»).
        cx.notify();
    }

    /// Закрыть панель-монету крестиком: убрать график + отписаться от стакана
    /// (убрать (core, market) из `desired`, если ни одна оставшаяся панель этого чарта его не
    /// держит — трейды биржи идут оптом, снимаем именно стакан через `set_open`-дифф).
    fn remove_pane(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some((core, market)) = self.chart.remove_pane(idx) else {
            return;
        };
        self.view_dirty = true;
        if !self.chart.uses_market(core, &market) {
            self.release_market_ref(core, &market, cx);
        }
        cx.notify();
    }

    /// П.2: приколоть/открепить панель idx. Пин отменяет авто-закрытие по TTL; открепление
    /// возвращает TTL (панель закроется, если срок уже истёк) → пере-арм таймера дедлайнов.
    fn toggle_pin(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.chart.toggle_pane_pin(idx) {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    /// Закрыть ВСЕ монеты этого чарта (кнопка «закрыть все графики» в выносном окне) +
    /// отписаться от их стаканов.
    pub fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        let removed = self.chart.clear_panes();
        if removed.is_empty() {
            return;
        }
        self.view_dirty = true;
        for (core, market) in removed {
            self.release_market_ref(core, &market, cx);
        }
        cx.notify();
    }

    fn retain_market_ref(&mut self, core: CoreId, market: &str, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        if self.registered_markets.insert((core, market.to_string())) {
            self.backend.update(cx, |b, _| {
                b.retain_chart_market(core, market);
            });
        }
    }

    fn release_market_ref(&mut self, core: CoreId, market: &str, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        if self.registered_markets.remove(&(core, market.to_string())) {
            self.backend.update(cx, |b, _| {
                b.release_chart_market(core, market);
            });
        }
    }

    fn release_market_refs_except(&mut self, keep: Option<(CoreId, &str)>, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        let keep = keep.map(|(core, market)| (core, market.to_string()));
        let old = std::mem::take(&mut self.registered_markets);
        for (core, market) in old {
            if keep.as_ref().is_some_and(|k| k.0 == core && k.1 == market) {
                self.registered_markets.insert((core, market));
            } else {
                self.backend.update(cx, |b, _| {
                    b.release_chart_market(core, &market);
                });
            }
        }
    }

    fn release_all_market_refs(&mut self, cx: &mut App) {
        self.sync_market_ref_epoch(cx);
        let old = std::mem::take(&mut self.registered_markets);
        for (core, market) in old {
            self.backend.update(cx, |b, _| {
                b.release_chart_market(core, &market);
            });
        }
    }

    fn sync_market_ref_epoch(&mut self, cx: &mut App) {
        let epoch = self.backend.read(cx).chart_market_refs_epoch;
        if self.market_ref_epoch != epoch {
            self.registered_markets.clear();
            self.market_ref_epoch = epoch;
        }
    }

    fn next_ttl_delay(&self, now_ms: f64) -> Option<Duration> {
        self.chart
            .next_ttl_deadline_ms()
            .map(|deadline| Duration::from_millis((deadline - now_ms).max(1.0).ceil() as u64))
    }

    fn arm_ttl_timer(&mut self, cx: &mut Context<Self>) {
        if self.ttl_timer_armed {
            return;
        }
        let Some(delay) = self.next_ttl_delay(now_unix_ms()) else {
            return;
        };
        self.ttl_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(delay).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.ttl_timer_armed = false;
                    let removed = this.chart.prune_ttl(now_unix_ms());
                    if !removed.is_empty() {
                        this.view_dirty = true;
                        for (core, market) in removed {
                            this.release_market_ref(core, &market, cx);
                        }
                        crate::diag::bump(&crate::diag::CHART_TTL_NOTIFY);
                        cx.notify();
                    }
                    this.arm_ttl_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    fn next_auto_live_delay(&self, now_ms: f64) -> Option<Duration> {
        self.chart
            .next_auto_live_deadline_ms()
            .map(|deadline| Duration::from_millis((deadline - now_ms).max(1.0).ceil() as u64))
    }

    /// One-shot таймер авто-возврата в live (П.9): мирроринг `arm_ttl_timer`. Армится из
    /// `mark_input_changed` после пана; по срабатыванию двигает live и пере-армится на
    /// следующий дедлайн (или гаснет, если возвращать больше нечего).
    fn arm_auto_live_timer(&mut self, cx: &mut Context<Self>) {
        if self.auto_live_timer_armed {
            return;
        }
        let Some(delay) = self.next_auto_live_delay(now_unix_ms()) else {
            return;
        };
        self.auto_live_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(delay).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.auto_live_timer_armed = false;
                    if this.chart.tick_auto_live(now_unix_ms()) {
                        this.view_dirty = true;
                        let follow = this.chart.follow();
                        this.backend.update(cx, |b, bcx| {
                            if b.follow != follow {
                                b.follow = follow;
                                bcx.notify();
                            }
                        });
                        cx.notify();
                    }
                    this.arm_auto_live_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    fn mark_input_changed(&mut self, cx: &mut Context<Self>) {
        self.chart.sync_follow_from_views();
        let follow = self.chart.follow();
        self.backend.update(cx, |b, bcx| {
            if b.follow != follow {
                b.follow = follow;
                bcx.notify();
            }
        });
        self.view_dirty = true;
        // Если пан перевёл панель в ручной режим — заводим таймер авто-возврата (П.9).
        self.arm_auto_live_timer(cx);
    }

    fn chart_local(&self, pos: Point<Pixels>) -> Option<((f32, f32), bool)> {
        self.chart.chart_local_from_window_pos(pos)
    }

    pub(crate) fn window_pos_in_glass_zone(&self, pos: Point<Pixels>) -> bool {
        let Some(((x, y), within)) = self.chart_local(pos) else {
            return false;
        };
        if !within {
            return false;
        }
        let rects = if self.input.pane_rects.is_empty() {
            self.chart.pane_rects()
        } else {
            self.input.pane_rects.clone()
        };
        rects.iter().any(|(_, r)| {
            if x < r.x || x > r.x + r.w || y < r.y || y > r.y + r.h {
                return false;
            }
            let glass_w = moon_chart::GLASS_ZONE_PX.min(r.w * 0.5);
            x >= r.x + r.w - glass_w
        })
    }

    fn sync_native_cursor(&mut self) -> bool {
        let cursor = self
            .input
            .cursor
            .and_then(|(x, y)| self.input.hovered_pane.map(|pane| (pane, x, y)));
        self.chart.set_cursor(cursor)
    }

    /// Подпись вкладки: для AddToChart — «N · рынок» (П.4: вместо безликого «Чарт N»),
    /// иначе рынок открытой монеты, затем «Main». Группа/ядро тут недоступны (их знают
    /// ChartTabs/DetachedChartHost) — используем номер + активный рынок.
    pub fn title_text(&self) -> String {
        let market = self
            .chart
            .active_market()
            .filter(|m| !m.is_empty())
            .or_else(|| self.market.clone());
        if let Some(n) = self.num {
            return match market {
                Some(m) => format!("{n} · {m}"),
                None => t!("chartwin.tab_title", n = n).to_string(),
            };
        }
        market.unwrap_or_else(|| "Main".into())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((core, market)) = self.chart.active_target() else {
            log::warn!("debug history fill: current chart has no active market");
            return false;
        };
        let now_ms = now_unix_ms();
        log::info!(
            "debug history fill: requesting core={core} market={market} span_ms={DEBUG_HISTORY_FILL_SPAN_MS}"
        );
        let filled = self
            .backend
            .read(cx)
            .session
            .diag_fill_market_history_to_capacity(
                core,
                &market,
                now_ms.round() as i64,
                DEBUG_HISTORY_FILL_SPAN_MS,
            );
        if !filled {
            log::warn!("debug history fill: failed core={core} market={market}");
            return false;
        }
        self.chart.force_history_reupload();
        self.view_dirty = true;
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
        log::info!("debug history fill: force reupload core={core} market={market}");
        true
    }
}

impl EventEmitter<PanelEvent> for ChartPanel {}
impl Focusable for ChartPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartPanel {
    fn panel_name(&self) -> &'static str {
        "Chart"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.title_text())
    }
    fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy {
        MoonBackgroundPolicy::NoFill
    }
}
impl Render for ChartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::CHART_RENDER);
        let became_visible = !self.scene_visible;
        self.scene_visible = true;
        self.chart.set_scene_visible(true);
        self.chart
            .set_market_source(Some(self.backend.read(cx).session.market_source()));
        let ppp = window.scale_factor();
        // Запоминаем DPI для data prepare path (у него нет window). DPI меняется редко.
        self.last_ppp = ppp;
        self.chart.set_last_ppp(ppp);
        let palette = MoonPalette::active(cx);
        self.chart.set_ui_palette(palette);
        // Bootstrap only: chartdx refines this from real `gpu_canvas.frame()` cadence,
        // so macOS/Linux do not depend on this fallback staying exact forever.
        let monitor_rate_hz = chart_bootstrap_present_rate_hz();
        let fast_divisor = (monitor_rate_hz / 60.0).round().max(1.0) as u32;
        let effective_present_rate_hz = if self.fast {
            monitor_rate_hz / fast_divisor as f32
        } else {
            60.0
        };
        self.chart.set_present_rate_hz(effective_present_rate_hz);
        // ВАЖНО: НЕТ request_animation_frame/continuous-present. `gpu_canvas.frame()` решает
        // present на platform tick без dirty GPUI tree; `draw()` рисует в тот же tick.
        let (theme, orders_style, follow) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            (eff.theme.clone(), eff.orders.clone(), b.follow)
        };
        // Масштаб — ПО-ВКЛАДОЧНЫЙ: берём self.scale (его правят set_scale из тулбара активной
        // вкладки / шапки выносного окна), а не глобальный backend.price_scale.
        let settings_changed = self.chart.set_theme(theme)
            | self.chart.set_orders(orders_style)
            | self.chart.set_scale(self.scale)
            | self.chart.set_follow(follow, now_unix_ms());
        if settings_changed {
            self.view_dirty = true;
        }

        // Render path only publishes layout/settings dirtiness. Market data is pulled
        // by gpu_canvas.frame(); account/order overlays have their own narrow sync.
        let view_changed = self.view_dirty;
        if became_visible || view_changed {
            self.view_dirty = false;
            self.sync_orders_if_visible(cx, true);
        }

        // axis_panes (раскладка панелей + снимок) считаем ОДИН раз за кадр и переиспользуем
        // и для hit-теста ввода (pane_rects), и для отрисовки осей — раньше layout панелей
        // гонялся дважды (внутри гейта prepare ради pane_rects + здесь ради отрисовки).
        let axis_panes = self.chart.axis_panes(axes::local_offset_sec());
        self.input.pane_rects = self.chart.pane_rects();
        // Угловой ✕ закрытия монеты — на панели графика (и Main, и AddToChart):
        // закрыл монету на Main → вернулись к лого. Позиция из раскладки панелей (девайс-px →
        // лог.px слота); собираем ДО canvas, который забирает axis_panes по move.
        let close_btns: Vec<(usize, f32, f32)> = axis_panes
            .iter()
            .map(|(idx, rect, _)| (*idx, (rect.x + rect.w) / ppp, rect.y / ppp))
            .collect();
        // Cursor-only motion is handled by the chart-slot hitbox below. It updates retained
        // gpu_canvas cursor/readout directly and does not notify the GPUI tree.
        // П.2: кнопка «пин» в левом верхнем углу ВНУТРИ области графика (правее ценовой оси,
        // не на самой оси) — ТОЛЬКО на AddToChart-панелях (с TTL). Пин отменяет авто-закрытие.
        // (idx, pinned, left_px, top_px). PRICE_AXIS_W — логическая ширина оси (rect в девайс-px).
        let pin_btns: Vec<(usize, bool, f32, f32)> = axis_panes
            .iter()
            .filter(|(idx, _, _)| self.chart.pane_is_pinnable(*idx))
            .map(|(idx, rect, _)| {
                (
                    *idx,
                    self.chart.pane_pinned(*idx),
                    rect.x / ppp + moon_chart::PRICE_AXIS_W,
                    rect.y / ppp,
                )
            })
            .collect();
        let show_empty_logo = axis_panes.is_empty();
        let (slot_w, _) = self.chart.slot_dev_size();
        let logo_w = ((slot_w as f32 / ppp) * 0.28).clamp(180.0, 280.0);
        div()
            .id("chart-slot")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .relative()
            .track_focus(&self.focus)
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| {
                if cx.has_active_drag() {
                    return;
                } // идёт drag Dock-панели — не мешаем drop
                if this.main_stack_scroll && this.window_pos_in_glass_zone(e.position) {
                    return;
                }
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position) else {
                    return;
                };
                // В AddToChart-стеке колесо НАД ЦЕНОВОЙ ОСЬЮ (левее графика) скроллит сам стек,
                // а не зумит: не потребляем событие → оно всплывёт к MoonVirtualList. Над
                // графиком+стаканом — зум (ниже) + stop_propagation, чтобы стек не скроллился.
                if this.num.is_some() && within {
                    if let Some(idx) = this.input.pane_at(pos.0, pos.1) {
                        if let Some((_, rect)) =
                            this.input.pane_rects.iter().find(|(i, _)| *i == idx)
                        {
                            if pos.0 <= rect.x + moon_chart::PRICE_AXIS_W * sf {
                                return;
                            }
                        }
                    }
                }
                let dy = match e.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
                };
                this.input.last_ptr = pos;
                this.input.cursor = if within { Some(pos) } else { None };
                this.input.hovered_pane = this.input.pane_at(pos.0, pos.1);
                this.sync_native_cursor();
                let fb = this.chart.slot_dev_width();
                let changed = {
                    let input = &mut this.input;
                    this.chart.with_container_mut(|container| {
                        input.wheel(dy, e.modifiers.shift, within, container, fb, sf)
                    })
                };
                if changed {
                    this.mark_input_changed(cx);
                    crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                    cx.notify();
                }
                // Зум-зона графика: гасим всплытие, иначе колесо ещё и проскроллит стек.
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    if cx.has_active_drag() {
                        return;
                    }
                    let sf = window.scale_factor();
                    let Some((pos, within)) = this.chart_local(e.position) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    this.sync_native_cursor();
                    // На AddToChart-вкладках дабл-клик по ЧАРТУ → открыть монету на Main (fullscreen).
                    let allow_to_main = this.num.is_some();
                    let fb = this.chart.slot_dev_width();
                    let input_changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Left,
                                true,
                                within,
                                allow_to_main,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    let mut opened_to_main = false;
                    if let Some((core, market)) = this.input.pending_to_main.take() {
                        this.backend.update(cx, |b, bcx| {
                            b.open_request = Some((core, market));
                            b.open_request_rev = b.open_request_rev.wrapping_add(1);
                            // Только этот путь (дабл-клик по чарту) поднимает окно Main (П.1).
                            b.open_request_activate = true;
                            bcx.notify();
                        });
                        opened_to_main = true;
                    }
                    if input_changed || opened_to_main {
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &MouseUpEvent, window, cx| {
                    let sf = window.scale_factor();
                    let fb = this.chart.slot_dev_width();
                    let changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Left,
                                false,
                                false,
                                false,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    if changed {
                        this.mark_input_changed(cx);
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    if this.num.is_none() && this.window_pos_in_glass_zone(e.position) {
                        return;
                    }
                    let sf = window.scale_factor();
                    let Some((pos, within)) = this.chart_local(e.position) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    this.sync_native_cursor();
                    let fb = this.chart.slot_dev_width();
                    let changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Right,
                                true,
                                within,
                                false,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    if changed {
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, e: &MouseUpEvent, window, cx| {
                    if this.num.is_none() && this.window_pos_in_glass_zone(e.position) {
                        return;
                    }
                    let sf = window.scale_factor();
                    let fb = this.chart.slot_dev_width();
                    let changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Right,
                                false,
                                false,
                                false,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    if changed {
                        this.view_dirty = true;
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if cx.has_active_drag() {
                    return;
                } // идёт drag Dock-панели — не перехватываем
                let Some((pos, within)) = this.chart_local(e.position) else {
                    return;
                };
                crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE);
                if e.pressed_button.is_none() {
                    crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE_FAST);
                    let prev_cursor = this.input.cursor;
                    let prev_hovered = this.input.hovered_pane;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    let cursor_changed =
                        prev_cursor != this.input.cursor || prev_hovered != this.input.hovered_pane;
                    if cursor_changed && this.sync_native_cursor() {
                        crate::diag::bump(&crate::diag::CHART_CURSOR_UPDATE);
                    }
                    if within {
                        crate::diag::bump(&crate::diag::CHART_MOUSE_FAST_STOP);
                        cx.stop_propagation();
                    }
                    return;
                }
                crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE_ENTITY);
                let sf = window.scale_factor();
                this.input.sync_pressed(
                    e.pressed_button == Some(MouseButton::Left),
                    e.pressed_button == Some(MouseButton::Right),
                );
                let prev_cursor = this.input.cursor;
                let prev_hovered = this.input.hovered_pane;
                this.input.cursor = if within { Some(pos) } else { None };
                this.input.hovered_pane = if within {
                    this.input.pane_at(pos.0, pos.1)
                } else {
                    None
                };
                let fb = this.chart.slot_dev_width();
                let dragging = {
                    let input = &mut this.input;
                    this.chart.with_container_mut(|container| {
                        input.pointer_drag(pos.0, pos.1, container, sf, fb)
                    })
                };
                if dragging {
                    this.mark_input_changed(cx);
                }
                let cursor_changed =
                    prev_cursor != this.input.cursor || prev_hovered != this.input.hovered_pane;
                if cursor_changed {
                    if this.sync_native_cursor() {
                        crate::diag::bump(&crate::diag::CHART_CURSOR_UPDATE);
                    }
                }
                // Drag меняет камеры/оси и GPUI-side controls. Cursor-only move теперь
                // остаётся в retained gpu_canvas: crosshair/readout present без cx.notify().
                if dragging {
                    crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, _cx| {
                if !*hovered {
                    let changed = this.input.cursor.take().is_some()
                        || this.input.hovered_pane.take().is_some();
                    if changed {
                        this.sync_native_cursor();
                    }
                }
            }))
            // own-pass: геометрию слота движок берёт синхронно из `GpuFrameInfo.bounds` в
            // `frame()` (см. data_state::apply_slot_geometry) — поэтому уже первый present рисует
            // в реальном слоте, без «распахивания» дефолтного размера и без лага при рефлоу.
            .child(self.chart.canvas().text_over().absolute().size_full())
            .when(show_empty_logo, |this| {
                // Непрозрачный фон поверх own-pass: пустой слот = логотип на фоне чарта, без
                // просвечивания старого графика (own-pass рисуется ПОД сценой GPUI).
                this.child(
                    div()
                        .absolute()
                        .size_full()
                        .bg(rgb(palette.chart_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::design::logo_glow_sized(logo_w)),
                )
            })
            // FireTest probe only. Геометрию самого чарта не берём из GPUI-probe: единственный
            // source of truth для input/own-pass — `GpuFrameInfo.bounds`.
            .child({
                let is_main = self.num.is_none();
                let backend = self.backend.clone();
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let sf = window.scale_factor();
                        let firetest_probe = crate::firetest::ChartProbe::new(
                            crate::windowing::window_hwnd(window),
                            f32::from(window.window_bounds().get_bounds().origin.x),
                            f32::from(window.window_bounds().get_bounds().origin.y),
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            sf,
                        );
                        if is_main {
                            if let Some(probe) = firetest_probe {
                                backend.update(cx, |b, _| {
                                    crate::firetest::observe_chart_probe(b, probe);
                                });
                            }
                        }
                    },
                )
                .absolute()
                .size_full()
            })
            .children(close_btns.into_iter().map(|(idx, right, top)| {
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-close-{idx}")))
                    .label("×")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .bounds(MoonRect::new(right - 18.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.remove_pane(idx, cx));
                    })
                    .render()
            }))
            .children(pin_btns.into_iter().map(|(idx, pinned, left, top)| {
                // Пин-кнопка в левом верхнем углу: заполненный кружок = приколото, контур = нет (П.2).
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-pin-{idx}")))
                    .label(if pinned { "●" } else { "○" })
                    .size(MoonButtonSize::Micro)
                    .variant(if pinned {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(pinned)
                    .bounds(MoonRect::new(left + 3.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.toggle_pin(idx, cx));
                    })
                    .render()
            }))
    }
}
