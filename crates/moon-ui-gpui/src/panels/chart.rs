//! Панель чарта (center DockArea): движок offscreen+readback + ввод + оверлей осей.
//! Перенос всей чарт-логики из Shell. Как Dock-панель — отцепляется в окно (ChartGpu
//! рендерит offscreen, не привязан к ОС-окну). Монета — из focus и `Backend.open_request`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::*;
use gpui_component::dock::{Panel, PanelEvent};

use crate::chart::ChartGpu;
use crate::{axes, input, Backend};
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::ChartTheme;
use moon_core::session::CoreId;

pub struct ChartPanel {
    backend: Entity<Backend>,
    chart: ChartGpu,
    chart_img: Option<Arc<RenderImage>>,
    chart_dirty: bool,
    chart_dev: (u32, u32),
    chart_bounds: Option<Bounds<Pixels>>,
    input: input::ChartInput,
    market: Option<String>,
    /// Номер AddToChart-вкладки (None = Main).
    num: Option<u32>,
    /// Сигнатура рыночных данных прошлого кадра — чтобы НЕ гонять дорогой
    /// offscreen-readback на холостом ходу (только при реальном приходе данных).
    data_sig: u64,
    /// Время прошлого submit (offscreen-рендер+readback) — для кэпа частоты readback'а:
    /// активное окно ~10fps, фоновое ~3fps (аналог 60/20fps оригинала). readback дорог
    /// (десятки МБ/с копий+заливок на главном потоке), поэтому фоновое окно гоним реже.
    last_submit: Option<Instant>,
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
        let mut chart = ChartGpu::new(epoch, theme);
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
        // open_request (дабл-клик→Main) обрабатывает ChartTabs. Здесь — prune +
        // пере-рендер ТОЛЬКО при приходе данных (сигнатура), истечении TTL-панели или
        // незабранном readback (его надо подобрать). Иначе на холостом ходу не нотифаим
        // — чтобы окна не молотили зря и не отнимали поток друг у друга.
        cx.observe(&backend, |this, backend, cx| {
            let pruned = this.chart.prune_ttl(now_unix_ms());
            let sig = this.chart.data_signature(&backend.read(cx).session);
            let changed = pruned || sig != this.data_sig;
            if changed {
                this.data_sig = sig;
                this.chart_dirty = true;
            }
            if changed || this.chart.is_pending() {
                cx.notify();
            }
        })
        .detach();
        Self {
            backend,
            chart,
            chart_img: None,
            chart_dirty: true,
            chart_dev: (1024, 576),
            chart_bounds: None,
            input: input::ChartInput::default(),
            market,
            num: None,
            data_sig: 0,
            last_submit: None,
            focus: cx.focus_handle(),
        }
    }

    /// Открыть монету на этой (Main) панели фуллскрином + подписать рынок.
    pub fn open_market(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.chart.open(core, &market);
        self.market = Some(market.clone());
        self.backend.update(cx, |b, _| {
            if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                b.desired.push((core, market));
            }
        });
        self.chart_dirty = true;
    }

    /// AddToChart-вкладка №`num` (без focus-монеты; наполняется детектами через add_coin).
    /// `core` — ядро-владелец при `charts_split_by_core` (вкладка «номер-ядро»), иначе None.
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        core: Option<CoreId>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chart = ChartGpu::new_kind(epoch, theme, ContainerKind::Chart { num, core });
        // Дренаж → prune + пере-рендер при данных/TTL/незабранном readback (см. new).
        cx.observe(&backend, |this, backend, cx| {
            let pruned = this.chart.prune_ttl(now_unix_ms());
            let sig = this.chart.data_signature(&backend.read(cx).session);
            let changed = pruned || sig != this.data_sig;
            if changed {
                this.data_sig = sig;
                this.chart_dirty = true;
            }
            if changed || this.chart.is_pending() {
                cx.notify();
            }
        })
        .detach();
        Self {
            backend,
            chart,
            chart_img: None,
            chart_dirty: true,
            chart_dev: (1024, 576),
            chart_bounds: None,
            input: input::ChartInput::default(),
            market: None,
            num: Some(num),
            data_sig: 0,
            last_submit: None,
            focus: cx.focus_handle(),
        }
    }

    /// Число открытых панелей чарта (для бейджа-счётчика на вкладке, как в egui).
    pub fn pane_count(&self) -> usize {
        self.chart.container.panes.len()
    }

    /// AddToChart: добавить монету авто-панелью (Tiled-мультичарт) с TTL.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64) {
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.chart_dirty = true;
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

    /// Подпись вкладки: «Чарт N» для AddToChart, иначе рынок открытой монеты
    /// (из контейнера — авторитетно), затем self.market, затем «Main».
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
}
impl Render for ChartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::chart::DBG_RENDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ppp = window.scale_factor();
        self.chart.resize(self.chart_dev.0, self.chart_dev.1);

        let (theme, orders_style, scale, follow) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            (eff.theme.clone(), eff.orders.clone(), b.price_scale, b.follow)
        };
        if self.chart.set_theme(theme) {
            self.chart_dirty = true;
        }
        if self.chart.set_orders(orders_style) {
            self.chart_dirty = true;
        }
        // Масштаб цены и Live/Пауза — из ScalePanel тулбара (живут в Backend).
        if self.chart.set_scale(scale) {
            self.chart_dirty = true;
        }
        if self.chart.set_follow(follow, now_unix_ms()) {
            self.chart_dirty = true;
        }

        // Неблокирующий конвейер кадра: (1) забрать готовый readback, если поспел;
        // (2) если есть что рисовать и нет незабранного кадра — отправить новый (НЕ
        // блокирует UI-поток, в отличие от прежнего poll(Wait) — иначе фоновое окно
        // другой группы фризилось, пока активное окно занимает поток).
        if let Some((img_arc, layout)) = self.chart.poll_image() {
            if let Some(old) = self.chart_img.take() {
                cx.drop_image(old, Some(window));
            }
            self.input.pane_rects = layout;
            self.chart_img = Some(img_arc);
        }
        // Кэп частоты readback по активности окна (аналог 60/20fps оригинала): фокусное
        // окно ~10fps, фоновое ~3fps. readback дорог (десятки МБ/с копий+заливок текстур
        // на главном потоке) — без кэпа 2 окна забивают поток и фоновое дёргается/встаёт.
        let min_dt = if window.is_window_active() {
            Duration::from_millis(90)
        } else {
            Duration::from_millis(320)
        };
        let due = self.last_submit.map_or(true, |t| Instant::now().duration_since(t) >= min_dt);
        if (self.chart_dirty || self.chart_img.is_none()) && !self.chart.is_pending() && due {
            let b = self.backend.read(cx);
            self.chart.submit(&b.session, ppp);
            self.chart_dirty = false;
            self.last_submit = Some(Instant::now());
        }
        // Незабранный readback подберёт следующий дренаж (observe нотифаит, пока pending) —
        // БЕЗ self-notify здесь: иначе активное окно крутится на 60fps и забивает поток,
        // из-за чего фоновое окно другой группы не получает кадров.
        let chart_img = self.chart_img.clone();
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
                if cx.has_active_drag() { return; } // идёт drag Dock-панели — не мешаем drop
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                let dy = match e.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
                };
                this.input.hovered_pane = this.input.pane_at(pos.0, pos.1);
                let fb = this.chart_dev.0 as f32;
                if this.input.wheel(dy, e.modifiers.shift, within, &mut this.chart.container, fb) {
                    this.chart_dirty = true;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                if cx.has_active_drag() { return; }
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                this.input.last_ptr = pos;
                // На AddToChart-вкладках дабл-клик по ЧАРТУ (не стакану) → открыть ту
                // монету/ядро на Main (fullscreen). Main себе это не делает.
                let allow_to_main = this.num.is_some();
                this.input.mouse_button(input::Btn::Left, true, within, allow_to_main, &mut this.chart.container);
                if let Some((core, market)) = this.input.pending_to_main.take() {
                    this.backend.update(cx, |b, bcx| {
                        b.open_request = Some((core, market));
                        bcx.notify();
                    });
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _e: &MouseUpEvent, _window, cx| {
                this.input.mouse_button(input::Btn::Left, false, false, false, &mut this.chart.container);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                this.input.last_ptr = pos;
                if this.input.mouse_button(input::Btn::Right, true, within, false, &mut this.chart.container) {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, _e: &MouseUpEvent, _window, cx| {
                this.input.mouse_button(input::Btn::Right, false, false, false, &mut this.chart.container);
                this.chart_dirty = true;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if cx.has_active_drag() { return; } // идёт drag Dock-панели — не перехватываем
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position, sf) else { return };
                this.input.sync_pressed(
                    e.pressed_button == Some(MouseButton::Left),
                    e.pressed_button == Some(MouseButton::Right),
                );
                this.input.cursor = if within { Some(pos) } else { None };
                this.input.hovered_pane = if within { this.input.pane_at(pos.0, pos.1) } else { None };
                let dragging = this.input.pointer_drag(pos.0, pos.1, &mut this.chart.container);
                // Крестик тикового графика должен ездить плавно → перерисовываемся на
                // движение мыши. Это ДЁШЕВО: меняется только GPUI-оверлей (canvas с осями/
                // крестиком), а дорогой offscreen-readback графика НЕ запускается (он
                // гейтится chart_dirty+last_submit ниже). Драг — ещё и помечает данные.
                if dragging {
                    this.chart_dirty = true;
                }
                cx.notify();
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.input.cursor = None;
                    this.input.hovered_pane = None;
                    cx.notify();
                }
            }))
            // Картинка появляется, когда первый readback поспел (None — первые кадры).
            .children(chart_img.map(|i| img(i).absolute().size_full()))
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
                                this.chart_dirty = true;
                                cx.notify();
                            });
                        }
                        // Оси/перекрестие — ПО КАЖДОЙ панели (Tiled-мультичарт): свой
                        // прямоугольник (девайс-px → лог.px окна) и снимок. Курсор —
                        // только для панели под мышью.
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
                                    point(bounds.origin.x + px(x / sf), bounds.origin.y + px(y / sf))
                                })
                            } else {
                                None
                            };
                            axes::draw(window, cx, sub, snap, cursor, sf, cross);
                        }
                    },
                )
                .absolute()
                .size_full()
            })
    }
}
