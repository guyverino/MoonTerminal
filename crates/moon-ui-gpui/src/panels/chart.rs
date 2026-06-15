//! Панель чарта (center DockArea): НАШ own-pass DX11 рендер (через generic-хук gpui) +
//! ввод + GPUI-оверлей осей/курсора. Как Dock-панель — отцепляется в окно. Монета — из
//! focus и `Backend.open_request`.
//!
//! Рендер: `ChartEngine.register_pass` ставит own-pass ПОД сценой (рисует combo/слои в
//! backbuffer GPUI без readback), `prepare` каждый кадр обновляет вид и заливает новые тики.
//! Текст осей и перекрестие — GPUI-оверлей ПОВЕРХ (нативный текст, см. `docs/RENDER_PLAN.md`).

use gpui::*;
use moon_palette::{MoonBackgroundPolicy, MoonPalette, Panel, PanelEvent};

use crate::chartdx::ChartEngine;
use crate::{Backend, axes, input};
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::ChartTheme;
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

pub struct ChartPanel {
    backend: Entity<Backend>,
    chart: ChartEngine,
    /// Размер слота чарта (девайс-px) — меряет canvas-оверлей окна.
    chart_dev: (u32, u32),
    /// Bounds слота (лог. px окна) — для hit-теста ввода.
    chart_bounds: Option<Bounds<Pixels>>,
    input: input::ChartInput,
    market: Option<String>,
    /// Номер AddToChart-вкладки (None = Main).
    num: Option<u32>,
    /// Сигнатура рыночных данных прошлого кадра — нотифаим только при реальном приходе данных.
    data_sig: u64,
    /// FastChart: true → плавный кадр по vsync (фокусный чарт); false → адаптивно (по приходу
    /// данных через observe, фон/мультичарт). Main=true, AddToChart=false (правится из тулбара позже).
    fast: bool,
    fast_frame_skip: u32,
    last_prepared_data_sig: u64,
    last_prepared_dev: (u32, u32),
    last_prepared_bounds: Option<Bounds<Pixels>>,
    view_dirty: bool,
    last_adaptive_notify_ms: f64,
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
        // Пере-рендер ТОЛЬКО при приходе данных (сигнатура) или истечении TTL-панели — иначе
        // окна не молотят зря. Own-pass рисует на каждом нашем кадре, поэтому достаточно notify.
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let pruned = this.chart.prune_ttl(now);
            let sig = this.chart.data_signature(&backend.read(cx).session);
            if pruned {
                this.view_dirty = true;
            }
            if pruned || sig != this.data_sig {
                this.data_sig = sig;
                if this.fast || pruned || now - this.last_adaptive_notify_ms >= 1000.0 {
                    this.last_adaptive_notify_ms = now;
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
            market,
            num: None,
            data_sig: 0,
            fast: true,
            fast_frame_skip: 0,
            last_prepared_data_sig: u64::MAX,
            last_prepared_dev: (0, 0),
            last_prepared_bounds: None,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
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
        cx.notify();
    }

    /// AddToChart-вкладка №`num` (наполняется детектами через add_coin).
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        core: Option<CoreId>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chart = ChartEngine::new_kind(epoch, theme, ContainerKind::Chart { num, core });
        cx.observe(&backend, |this, backend, cx| {
            let pruned = this.chart.prune_ttl(now_unix_ms());
            let sig = this.chart.data_signature(&backend.read(cx).session);
            if pruned {
                this.view_dirty = true;
            }
            if pruned || sig != this.data_sig {
                this.data_sig = sig;
                cx.notify();
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
            num: Some(num),
            data_sig: 0,
            fast: false,
            fast_frame_skip: 0,
            last_prepared_data_sig: u64::MAX,
            last_prepared_dev: (0, 0),
            last_prepared_bounds: None,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            focus: cx.focus_handle(),
        }
    }

    /// Число открытых панелей чарта (для бейджа-счётчика на вкладке).
    pub fn pane_count(&self) -> usize {
        self.chart.pane_count()
    }

    /// Снять own-pass этой панели с окна — для НЕактивных вкладок (их render не
    /// зовётся, и без снятия их pas рисует застывший чарт поверх активного).
    pub fn unregister_pass(&mut self) {
        self.chart.unregister_pass();
    }

    /// AddToChart: добавить монету авто-панелью (Tiled-мультичарт) с TTL.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64) {
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.view_dirty = true;
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
        let ppp = window.scale_factor();
        // Own-pass регистрируется один раз (внутри гейт по subscription).
        self.chart.register_pass(window);
        let monitor_rate_hz = chart_present_rate_hz();
        let fast_divisor = (monitor_rate_hz / 60.0).round().max(1.0) as u32;
        let effective_present_rate_hz = if self.fast {
            monitor_rate_hz / fast_divisor as f32
        } else {
            60.0
        };
        self.chart.set_present_rate_hz(effective_present_rate_hz);
        if self.fast {
            window.request_animation_frame();
        }
        self.chart.resize(self.chart_dev.0, self.chart_dev.1);
        // Origin слота В ОКНЕ (девайс-px): own-pass рисует в backbuffer окна, не в слот-текстуру.
        let (ox, oy) = self
            .chart_bounds
            .map(|b| (f32::from(b.origin.x) * ppp, f32::from(b.origin.y) * ppp))
            .unwrap_or((0.0, 0.0));
        self.chart.set_origin(ox, oy);
        let (theme, orders_style, scale, follow) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            (
                eff.theme.clone(),
                eff.orders.clone(),
                b.price_scale,
                b.follow,
            )
        };
        let settings_changed = self.chart.set_theme(theme)
            | self.chart.set_orders(orders_style)
            | self.chart.set_scale(scale)
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
            let b = self.backend.read(cx);
            self.chart.prepare(&b.session, ppp);
            self.input.pane_rects = self
                .chart
                .axis_panes(0)
                .into_iter()
                .map(|(idx, rect, _)| (idx, rect))
                .collect();
            self.last_prepared_data_sig = self.data_sig;
            self.last_prepared_dev = self.chart_dev;
            self.last_prepared_bounds = self.chart_bounds;
            self.view_dirty = false;
        }

        let axis_panes = self.chart.axis_panes(axes::local_offset_sec());
        let cross = self.chart.crosshair_style();
        let cursor_dev = self.input.cursor;
        let hovered = self.input.hovered_pane;

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
                    this.input.mouse_button(
                        input::Btn::Left,
                        true,
                        within,
                        allow_to_main,
                        &mut this.chart.container,
                        sf,
                        this.chart_dev.0 as f32,
                    );
                    if let Some((core, market)) = this.input.pending_to_main.take() {
                        this.backend.update(cx, |b, bcx| {
                            b.open_request = Some((core, market));
                            bcx.notify();
                        });
                    }
                    cx.notify();
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
                    }
                    cx.notify();
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
                    }
                    cx.notify();
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
                cx.notify();
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.input.cursor = None;
                    this.input.hovered_pane = None;
                    cx.notify();
                }
            }))
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
    }
}
