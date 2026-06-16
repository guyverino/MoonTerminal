//! Панель чарта (center DockArea): НАШ own-pass DX11 рендер (через generic-хук gpui) +
//! ввод + GPUI-оверлей осей/курсора. Как Dock-панель — отцепляется в окно. Монета — из
//! focus и `Backend.open_request`.
//!
//! Рендер: `ChartEngine.register_pass` ставит own-pass ПОД сценой (рисует combo/слои в
//! backbuffer GPUI без readback), `prepare` каждый кадр обновляет вид и заливает новые тики.
//! Текст осей и перекрестие — GPUI-оверлей ПОВЕРХ (нативный текст, см. `docs/RENDER_PLAN.md`).

use std::time::Duration;

use gpui::*;
use moon_palette::{MoonBackgroundPolicy, MoonPalette, Panel, PanelEvent};

use crate::chartdx::ChartEngine;
use crate::{Backend, axes, input};
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{ChartTheme, OrdersStyle};
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

fn chart_present_rate_hz() -> f32 {
    let refresh = monitor_refresh_hz().clamp(30, 360);
    refresh as f32
}

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
    /// Размер слота чарта (девайс-px) — меряет canvas-оверлей окна.
    chart_dev: (u32, u32),
    /// Bounds слота (лог. px окна) — для hit-теста ввода.
    chart_bounds: Option<Bounds<Pixels>>,
    input: input::ChartInput,
    market: Option<String>,
    /// Масштаб цены ЭТОЙ вкладки (None = Авто). Теперь ПО-ВКЛАДОЧНЫЙ (не глобальный): правится
    /// своим регулятором (тулбар активной вкладки / шапка выносного окна), применяется в render.
    scale: Option<f32>,
    /// Номер AddToChart-вкладки (None = Main).
    num: Option<u32>,
    /// Сигнатура рыночных данных прошлого кадра — нотифаим только при реальном приходе данных.
    data_sig: u64,
    /// UI-настройки, которые применяются в render. После включения cached dock-панелей
    /// top-down Shell render больше не будит ChartPanel, поэтому изменения должны нотифаить
    /// саму панель.
    settings_sig: ChartSettingsSig,
    /// FastChart: true → плавный кадр по vsync (фокусный чарт); false → адаптивно (по приходу
    /// данных через observe, фон/мультичарт). Main=true, AddToChart=false (правится из тулбара позже).
    fast: bool,
    fast_frame_skip: u32,
    last_prepared_data_sig: u64,
    last_prepared_dev: (u32, u32),
    last_prepared_bounds: Option<Bounds<Pixels>>,
    view_dirty: bool,
    last_adaptive_notify_ms: f64,
    /// Последний scale_factor окна (ставится в render). Нужен 60-Гц prepare-задаче, у
    /// которой нет window — DPI меняется редко, между сменами берём запомненный.
    last_ppp: f32,
    /// Последний виденный задачей present_seq `gpu_canvas`. Задача доливает новые данные лишь
    /// после реального draw/present, поэтому спит при hidden/occluded/paused кадрах.
    last_present_seq: u64,
    /// One-shot timer до ближайшего истечения AddToChart TTL. Это time-based dirty,
    /// поэтому он не должен зависеть от backend data observe.
    ttl_timer_armed: bool,
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
        let mut chart = ChartEngine::new(epoch, theme);
        let mut market = None;
        if let Some((core, m)) = focus_open {
            chart.open(core, &m);
            market = Some(m.clone());
            backend.update(cx, |b, _| {
                if !b.desired.iter().any(|(c, mm)| *c == core && mm == &m) {
                    b.desired.push((core, m));
                }
            });
        }
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        // Пере-рендер при приходе данных (сигнатура) или изменении UI-настроек чарта.
        // Time-based TTL панелей обслуживает локальный one-shot timer, не backend data observe.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::CHART_OBS_FIRE);
            let now = now_unix_ms();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.data_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if sig != this.data_sig {
                this.data_sig = sig;
                // Троттл notify. Данные own-pass рисует сам по present (форк), notify нужен лишь
                // для GPUI-оверлея осей, а он идёт top-down → дёргает Orders. Поэтому ≤4 Гц для
                // fast (≥250мс) и ≤1 Гц для addto. Для fast при live-follow реальный notify даёт
                // 60-Гц prepare-задача (тоже ≥250мс через
                // общий last_adaptive_notify_ms); этот источник работает на паузе, когда задача спит.
                let floor = if this.fast { 250.0 } else { 1000.0 };
                if now - this.last_adaptive_notify_ms >= floor {
                    this.last_adaptive_notify_ms = now;
                    crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                    cx.notify();
                }
            }
        })
        .detach();
        // 60-Гц prepare БЕЗ перерисовки GPUI-дерева и БЕЗ notify: доливает новые тики/стакан/ордера
        // в resident-слои. Живой край двигает `GpuCanvasDriver::frame()` на platform tick и сам
        // просит present только на pixel-cross/data-dirty. Раньше гладкость давал
        // request_animation_frame/continuous-present, метивший дёрти весь путь до Shell →
        // top-down перерисовка Orders на refresh монитора (диско).
        // Задача доливает данные только после реального draw canvas-а (`present_seq` вырос):
        // hidden/occluded окно не презентит → seq стоит → задача спит.
        cx.spawn(async move |this, cx| {
            // gpui (свежий): AsyncApp::update инфэллибл (возвращает R, не Result).
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor.timer(std::time::Duration::from_millis(16)).await;
                let alive = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        // Камеру двигает GpuCanvasDriver::frame(); здесь только доливаем данные
                        // после реально нарисованного кадра.
                        let seq = this.chart.present_seq();
                        if seq != this.last_present_seq {
                            this.last_present_seq = seq;
                            crate::diag::bump(&crate::diag::CHART_TASK_PREP);
                            let b = this.backend.read(cx);
                            this.chart.prepare(&b.session, this.last_ppp);
                        }
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self {
            backend,
            chart,
            chart_dev: (1024, 576),
            chart_bounds: None,
            input: input::ChartInput::default(),
            market,
            scale: None,
            num: None,
            data_sig: 0,
            settings_sig,
            fast: true,
            fast_frame_skip: 0,
            last_prepared_data_sig: u64::MAX,
            last_prepared_dev: (0, 0),
            last_prepared_bounds: None,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            last_ppp: 1.0,
            last_present_seq: 0,
            ttl_timer_armed: false,
            focus: cx.focus_handle(),
        }
    }

    /// Открыть монету на этой (Main) панели фуллскрином + подписать рынок.
    pub fn open_market(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.chart.open(core, &market);
        self.view_dirty = true;
        self.market = Some(market.clone());
        self.backend.update(cx, |b, _| {
            if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                b.desired.push((core, market));
            }
        });
        crate::diag::bump(&crate::diag::CHART_OPEN_NOTIFY);
        cx.notify();
    }

    /// AddToChart-вкладка №`num` (наполняется детектами через add_coin). Без `window`: панель
    /// строится из данных, окно ей не нужно (важно для отложенного восстановления откреп-окон).
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        core: Option<CoreId>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let chart = ChartEngine::new_kind(epoch, theme, ContainerKind::Chart { num, core });
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.data_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if sig != this.data_sig {
                this.data_sig = sig;
                // AddToChart — фоновый/мультичарт: notify (а с ним top-down перерисовка Orders)
                // ≤1 Гц. Time-based prune делает локальный TTL timer.
                if now - this.last_adaptive_notify_ms >= 1000.0 {
                    this.last_adaptive_notify_ms = now;
                    crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                    cx.notify();
                }
            }
        })
        .detach();
        Self {
            backend,
            chart,
            chart_dev: (1024, 576),
            chart_bounds: None,
            input: input::ChartInput::default(),
            market: None,
            scale: None,
            num: Some(num),
            data_sig: 0,
            settings_sig,
            fast: false,
            fast_frame_skip: 0,
            last_prepared_data_sig: u64::MAX,
            last_prepared_dev: (0, 0),
            last_prepared_bounds: None,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            last_ppp: 1.0,
            last_present_seq: 0,
            ttl_timer_armed: false,
            focus: cx.focus_handle(),
        }
    }

    /// Число открытых панелей чарта (для бейджа-счётчика на вкладке).
    pub fn pane_count(&self) -> usize {
        self.chart.pane_count()
    }

    /// Масштаб цены ЭТОЙ вкладки (для дропдауна шапки выносного окна и синка тулбара).
    pub fn scale(&self) -> Option<f32> {
        self.scale
    }

    /// Поставить масштаб ЭТОЙ вкладки (None=Авто). Применяется в render через `set_scale` движка.
    pub fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale != pct {
            self.scale = pct;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Снять own-pass этой панели с окна — для НЕактивных вкладок (их render не
    /// зовётся, и без снятия их pas рисует застывший чарт поверх активного).
    pub fn unregister_pass(&mut self) {
        self.chart.unregister_pass();
    }

    /// AddToChart: добавить монету авто-панелью (Tiled-мультичарт) с TTL.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64, cx: &mut Context<Self>) {
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.view_dirty = true;
        self.arm_ttl_timer(cx);
        // ВАЖНО: notify самой панели. Для вкладки в стрипе её перерисовывает render ChartTabs,
        // но ОТКРЕПЛЁННАЯ панель живёт в своём окне — без notify оно не перерисуется и новая
        // монета не появится (баг «детект пришёл, а графика в откреп-окне нет»).
        cx.notify();
    }

    /// Закрыть панель-монету крестиком: убрать из мультичарта + отписаться от стакана
    /// (убрать (core, market) из `desired`, если ни одна оставшаяся панель этого чарта его не
    /// держит — трейды биржи идут оптом, снимаем именно стакан через `set_open`-дифф).
    fn remove_pane(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some((core, market)) = self.chart.container.remove_pane(idx) else {
            return;
        };
        self.view_dirty = true;
        if !self.chart.container.uses_market(core, &market) {
            self.backend.update(cx, |b, bcx| {
                let before = b.desired.len();
                b.desired
                    .retain(|(c, m)| !(*c == core && m.as_str() == market.as_str()));
                if b.desired.len() != before {
                    bcx.notify();
                }
            });
        }
        cx.notify();
    }

    /// Закрыть ВСЕ монеты этого чарта (кнопка «закрыть все графики» в выносном окне) +
    /// отписаться от их стаканов.
    pub fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        let removed = self.chart.container.clear_panes();
        if removed.is_empty() {
            return;
        }
        self.view_dirty = true;
        self.backend.update(cx, |b, bcx| {
            let before = b.desired.len();
            b.desired
                .retain(|(c, m)| !removed.iter().any(|(rc, rm)| rc == c && rm == m));
            if b.desired.len() != before {
                bcx.notify();
            }
        });
        cx.notify();
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
                    if this.chart.prune_ttl(now_unix_ms()) {
                        this.view_dirty = true;
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
    }

    fn chart_local(&self, pos: Point<Pixels>, sf: f32) -> Option<((f32, f32), bool)> {
        let b = self.chart_bounds?;
        let lx = f32::from(pos.x) - f32::from(b.origin.x);
        let ly = f32::from(pos.y) - f32::from(b.origin.y);
        let w = f32::from(b.size.width);
        let h = f32::from(b.size.height);
        let within = lx >= 0.0 && lx <= w && ly >= 0.0 && ly <= h;
        Some(((lx * sf, ly * sf), within))
    }

    /// Подпись вкладки: «Чарт N» для AddToChart, иначе рынок открытой монеты, затем «Main».
    pub fn title_text(&self) -> String {
        if let Some(n) = self.num {
            return format!("Чарт {n}");
        }
        self.chart
            .active_market()
            .filter(|m| !m.is_empty())
            .or_else(|| self.market.clone())
            .unwrap_or_else(|| "Main".into())
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
        let ppp = window.scale_factor();
        // Запоминаем DPI для 60-Гц prepare-задачи (у неё нет window). DPI меняется редко.
        self.last_ppp = ppp;
        let monitor_rate_hz = chart_present_rate_hz();
        let fast_divisor = (monitor_rate_hz / 60.0).round().max(1.0) as u32;
        let effective_present_rate_hz = if self.fast {
            monitor_rate_hz / fast_divisor as f32
        } else {
            60.0
        };
        self.chart.set_present_rate_hz(effective_present_rate_hz);
        // ВАЖНО: НЕТ request_animation_frame/continuous-present. `gpu_canvas.frame()` решает
        // present на platform tick без dirty GPUI tree; `draw()` рисует в тот же tick.
        self.chart.resize(self.chart_dev.0, self.chart_dev.1);
        // Origin слота В ОКНЕ (девайс-px): own-pass рисует в backbuffer окна, не в слот-текстуру.
        let (ox, oy) = self
            .chart_bounds
            .map(|b| (f32::from(b.origin.x) * ppp, f32::from(b.origin.y) * ppp))
            .unwrap_or((0.0, 0.0));
        self.chart.set_origin(ox, oy);
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

        let cadence_due = if self.fast {
            let due = self.fast_frame_skip == 0;
            self.fast_frame_skip = (self.fast_frame_skip + 1) % fast_divisor.max(1);
            due
        } else {
            self.fast_frame_skip = 0;
            true
        };
        let data_changed = self.data_sig != self.last_prepared_data_sig;
        let geometry_changed = self.chart_dev != self.last_prepared_dev
            || self.chart_bounds != self.last_prepared_bounds;
        // Подготовка кадра: вид + заливка новых данных в resident layers. Для fast-чарта
        // гейтим CPU/update работу до MoonBot-подобного every-N-vblank, но новые данные и
        // resize проходят сразу; own-pass на draw рисует последний подготовленный state.
        let view_changed = self.view_dirty;
        if cadence_due || data_changed || geometry_changed || view_changed {
            crate::diag::bump(&crate::diag::CHART_PREPARE);
            let b = self.backend.read(cx);
            self.chart.prepare(&b.session, ppp);
            self.last_prepared_data_sig = self.data_sig;
            self.last_prepared_dev = self.chart_dev;
            self.last_prepared_bounds = self.chart_bounds;
            self.view_dirty = false;
        }

        // axis_panes (раскладка панелей + снимок) считаем ОДИН раз за кадр и переиспользуем
        // и для hit-теста ввода (pane_rects), и для отрисовки осей — раньше layout панелей
        // гонялся дважды (внутри гейта prepare ради pane_rects + здесь ради отрисовки).
        let axis_panes = self.chart.axis_panes(axes::local_offset_sec());
        self.input.pane_rects = axis_panes
            .iter()
            .map(|(idx, rect, _)| (*idx, *rect))
            .collect();
        let cross = self.chart.crosshair_style();
        let cursor_dev = self.input.cursor;
        let hovered = self.input.hovered_pane;
        // Угловой ✕ закрытия монеты — на КАЖДОЙ панели (и Main, и AddToChart-мультичарт):
        // закрыл монету на Main → вернулись к лого. Позиция из раскладки панелей (девайс-px →
        // лог.px слота); собираем ДО canvas, который забирает axis_panes по move.
        let close_btns: Vec<(usize, f32, f32)> = axis_panes
            .iter()
            .map(|(idx, rect, _)| (*idx, (rect.x + rect.w) / ppp, rect.y / ppp))
            .collect();

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
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else {
                    return;
                };
                let dy = match e.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
                };
                this.input.last_ptr = pos;
                this.input.hovered_pane = this.input.pane_at(pos.0, pos.1);
                let fb = this.chart_dev.0 as f32;
                if this.input.wheel(
                    dy,
                    e.modifiers.shift,
                    within,
                    &mut this.chart.container,
                    fb,
                    sf,
                ) {
                    this.mark_input_changed(cx);
                    crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    if cx.has_active_drag() {
                        return;
                    }
                    let sf = window.scale_factor();
                    let Some((pos, within)) = this.chart_local(e.position, sf) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    // На AddToChart-вкладках дабл-клик по ЧАРТУ → открыть монету на Main (fullscreen).
                    let allow_to_main = this.num.is_some();
                    let input_changed = this.input.mouse_button(
                        input::Btn::Left,
                        true,
                        within,
                        allow_to_main,
                        &mut this.chart.container,
                        sf,
                        this.chart_dev.0 as f32,
                    );
                    let mut opened_to_main = false;
                    if let Some((core, market)) = this.input.pending_to_main.take() {
                        this.backend.update(cx, |b, bcx| {
                            b.open_request = Some((core, market));
                            b.open_request_rev = b.open_request_rev.wrapping_add(1);
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
                    if this.input.mouse_button(
                        input::Btn::Left,
                        false,
                        false,
                        false,
                        &mut this.chart.container,
                        sf,
                        this.chart_dev.0 as f32,
                    ) {
                        this.mark_input_changed(cx);
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    let sf = window.scale_factor();
                    let Some((pos, within)) = this.chart_local(e.position, sf) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    if this.input.mouse_button(
                        input::Btn::Right,
                        true,
                        within,
                        false,
                        &mut this.chart.container,
                        sf,
                        this.chart_dev.0 as f32,
                    ) {
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _e: &MouseUpEvent, window, cx| {
                    let sf = window.scale_factor();
                    if this.input.mouse_button(
                        input::Btn::Right,
                        false,
                        false,
                        false,
                        &mut this.chart.container,
                        sf,
                        this.chart_dev.0 as f32,
                    ) {
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
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else {
                    return;
                };
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
                let dragging = this.input.pointer_drag(
                    pos.0,
                    pos.1,
                    &mut this.chart.container,
                    sf,
                    this.chart_dev.0 as f32,
                );
                if dragging {
                    this.mark_input_changed(cx);
                }
                // Крестик и пан/зум — перерисовываемся на движение мыши (own-pass дёшев: combo
                // блитит готовый битмап, оверлей крестика — нативный GPUI).
                if dragging
                    || prev_cursor != this.input.cursor
                    || prev_hovered != this.input.hovered_pane
                {
                    crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    let changed = this.input.cursor.take().is_some()
                        || this.input.hovered_pane.take().is_some();
                    if changed {
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }
            }))
            .child(self.chart.canvas().absolute().size_full())
            // Оверлей: оси/числа/перекрестие — GPUI поверх own-pass графика (прозрачный регион).
            .child({
                let entity = cx.entity();
                let measured = self.chart_dev;
                let prev_bounds = self.chart_bounds;
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let sf = window.scale_factor();
                        let dev = (
                            (f32::from(bounds.size.width) * sf).round().max(1.0) as u32,
                            (f32::from(bounds.size.height) * sf).round().max(1.0) as u32,
                        );
                        if dev != measured || Some(bounds) != prev_bounds {
                            entity.update(cx, |this, cx| {
                                this.chart_bounds = Some(bounds);
                                this.chart_dev = dev;
                                crate::diag::bump(&crate::diag::CHART_CANVAS_NOTIFY);
                                cx.notify();
                            });
                        }
                        // Оси/перекрестие — ПО КАЖДОЙ панели (Tiled-мультичарт): свой
                        // прямоугольник (девайс-px → лог.px окна) и снимок. Курсор — под мышью.
                        let palette = MoonPalette::active(cx);
                        for (idx, rect, snap) in &axis_panes {
                            let sub = Bounds::new(
                                point(
                                    bounds.origin.x + px(rect.x / sf),
                                    bounds.origin.y + px(rect.y / sf),
                                ),
                                gpui::size(px(rect.w / sf), px(rect.h / sf)),
                            );
                            let cursor = if hovered == Some(*idx) {
                                cursor_dev.map(|(x, y)| {
                                    point(
                                        bounds.origin.x + px(x / sf),
                                        bounds.origin.y + px(y / sf),
                                    )
                                })
                            } else {
                                None
                            };
                            axes::draw(window, cx, sub, snap, cursor, sf, cross, palette);
                        }
                    },
                )
                .absolute()
                .size_full()
            })
            .children(close_btns.into_iter().map(|(idx, right, top)| {
                div()
                    .absolute()
                    .left(px(right - 18.0))
                    .top(px(top + 3.0))
                    .w(px(15.0))
                    .h(px(15.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .text_size(px(11.0))
                    .text_color(rgba(0xC8CCD0FF))
                    .bg(rgba(0x00000059))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(0xE04848CC)).text_color(rgb(0xFFFFFF)))
                    .child("×")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e: &MouseDownEvent, _w, cx| {
                            this.remove_pane(idx, cx);
                            cx.stop_propagation();
                        }),
                    )
            }))
    }
}
