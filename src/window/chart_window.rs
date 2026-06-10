//! Откреплённое чарт-окно: отдельное ОС-окно (свой wgpu-surface + девайс),
//! показывает контейнер chart+glass (тайл/фулскрин) БЕЗ тулбара/дока/ордера.
//! Создаётся перетаскиванием чарт-вкладки из окна группы. GPU-ресурсы `Chart`
//! привязаны к девайсу окна, поэтому панели пересоздаются из спецификации.

use std::sync::Arc;
use std::time::Instant;

use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window, WindowId};

use crate::chart::container::{Container, ContainerKind, Mode, PaneSource};
use crate::chart::paint::MIN_FRAME_DT;
use crate::chart::view::Rect;
use crate::config::{ChartTheme, OrdersStyle};
use crate::dock::controls::SCALES;
use crate::dock::ScaleAction;
use crate::gpu::GpuContext;
use crate::session::{CoreId, SessionManager};
use crate::shell::theme;

/// Высота верхней строки масштаба чарт-окна (логич. точки egui).
const TOPBAR_H: f32 = 32.0;

pub struct ChartWindow {
    pub window: Arc<Window>,
    /// Окно группы, из которого открепили (для маршрутизации новых детектов сюда).
    owner: WindowId,
    epoch_ms: f64,
    gpu: GpuContext,
    /// Интерактивный egui для верхней строки масштаба + крестика «очистить».
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    overlay_ctx: egui::Context,
    overlay_renderer: egui_wgpu::Renderer,
    container: Container,
    theme: ChartTheme,
    orders_style: OrdersStyle,
    /// Активный пресет масштаба (индекс в `SCALES`) — подсветка кнопки.
    scale_idx: usize,

    /// Ввод/интерактив чарта (общий с WindowHost), всё в физ. пикселях.
    input: crate::chart::input::ChartInput,
    /// Нажат крестик «закрыть окно» в тулбаре (App уберёт окно).
    close_requested: bool,

    dirty: bool,
    last_present_at: Instant,
    last_visible_sig: u64,
}

impl ChartWindow {
    /// Создаёт окно и контейнер из спецификации панелей (ядро/рынок/источник).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_loop: &ActiveEventLoop,
        owner: WindowId,
        owner_hwnd: Option<isize>,
        title: &str,
        kind: ContainerKind,
        mode: Mode,
        spec: Vec<(CoreId, String, PaneSource)>,
        theme: ChartTheme,
        orders_style: OrdersStyle,
        epoch_ms: f64,
    ) -> anyhow::Result<Self> {
        // Заголовок окна — переданное имя (формируется из номера и группы, см. App).
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title(format!("{title} — MoonTerminal"))
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 700.0));
        // Windows: делаем чарт-окно «дочерним» к окну группы — оно НЕ получает свою
        // кнопку в таскбаре и сворачивается/разворачивается вместе с родителем.
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            if let Some(h) = owner_hwnd {
                attrs = attrs.with_owner_window(h);
            }
            attrs = attrs.with_skip_taskbar(true);
        }
        let _ = owner_hwnd; // на не-Windows не используется
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_window_icon(crate::icons::brand_winit_icon());

        let gpu = GpuContext::new(window.clone())?;
        let container = Container::from_spec(kind, mode, spec, &gpu.device, gpu.format, epoch_ms);

        let egui_ctx = egui::Context::default();
        crate::shell::theme::apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);
        let overlay_ctx = egui::Context::default();
        crate::shell::theme::apply(&overlay_ctx);
        let overlay_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);

        Ok(Self {
            window,
            owner,
            epoch_ms,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            overlay_ctx,
            overlay_renderer,
            container,
            theme,
            orders_style,
            scale_idx: 0,
            input: crate::chart::input::ChartInput::default(),
            close_requested: false,
            dirty: true,
            last_present_at: Instant::now() - MIN_FRAME_DT,
            last_visible_sig: u64::MAX,
        })
    }

    pub fn set_theme(&mut self, theme: &ChartTheme) {
        if &self.theme != theme {
            self.theme = theme.clone();
            self.dirty = true;
        }
    }

    /// Применить стиль линий ордеров (orders.toml). Смена → dirty-кадр.
    pub fn set_orders_style(&mut self, style: &OrdersStyle) {
        if &self.orders_style != style {
            self.orders_style = style.clone();
            self.dirty = true;
        }
    }

    /// Окно группы-владельца (для маршрутизации детектов).
    pub fn owner(&self) -> WindowId {
        self.owner
    }

    /// Нажат ли крестик «закрыть окно» (App уберёт окно).
    pub fn take_close_requested(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    /// Открытые (ядро, рынок) панелей окна — для подписок (App включает их в
    /// set_open, иначе после открепления рынок отписывается и тики пропадают).
    pub fn open_markets(&self) -> Vec<(CoreId, String)> {
        self.container
            .panes
            .iter()
            .map(|p| (p.core, p.market.clone()))
            .collect()
    }

    /// Вид контейнера окна (ключ маршрутизации детектов: номер + ядро).
    pub fn chart_kind(&self) -> ContainerKind {
        self.container.kind
    }

    /// Новый AddToChart-детект → панель монеты в этом окне (создаётся на его
    /// девайсе), TTL продлевается при повторе.
    pub fn push_auto(&mut self, core: CoreId, market: &str, now_ms: f64, ttl_ms: f64) {
        self.container
            .push_auto(core, market, now_ms, ttl_ms, &self.gpu.device, self.gpu.format, self.epoch_ms);
        self.dirty = true;
    }

    /// Зона графиков (физ. px) — клиентская область ниже строки масштаба.
    fn area(&self) -> Rect {
        let top = TOPBAR_H * self.window.scale_factor() as f32;
        Rect {
            x: 0.0,
            y: top,
            w: self.gpu.size.width as f32,
            h: (self.gpu.size.height as f32 - top).max(1.0),
        }
    }

    /// Курсор в зоне графиков (ниже строки масштаба)?
    fn chart_input_ok(&self) -> bool {
        self.input.last_ptr.1 >= TOPBAR_H * self.window.scale_factor() as f32
    }

    fn visible_sig(&self, session: &SessionManager, now_ms: f64) -> u64 {
        crate::chart::paint::panes_visible_sig(&self.container.panes, session, now_ms)
    }

    pub fn needs_render(&self, session: &SessionManager, now_ms: f64) -> bool {
        self.dirty
            || self.container.has_ttl_panes() // гоним кадры для истечения KeepInChart
            || self.visible_sig(session, now_ms) != self.last_visible_sig
    }

    /// Обработать событие окна; true — окно просит закрытия (CloseRequested).
    pub fn on_event(&mut self, event: &WindowEvent) -> bool {
        // Сначала egui (строка масштаба/крестик), потом — ввод графиков.
        let _ = self.egui_state.on_window_event(&self.window, event);
        self.dirty = true;
        match event {
            WindowEvent::Resized(size) => {
                self.gpu.resize(*size);
                self.dirty = true;
            }
            WindowEvent::ModifiersChanged(m) => self.input.shift_down = m.state().shift_key(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_moved(position.x as f32, position.y as f32)
            }
            WindowEvent::CursorLeft { .. } => self.clear_cursor(),
            WindowEvent::MouseInput { state, button, .. } => {
                self.mouse_button(*button, *state == ElementState::Pressed)
            }
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            WindowEvent::CloseRequested => return true,
            _ => {}
        }
        false
    }

    fn clear_cursor(&mut self) {
        if self.input.cursor.take().is_some() {
            self.input.hovered_pane = None;
            self.window.set_cursor(CursorIcon::Default);
            self.dirty = true;
        }
    }

    /// Забрать запрос «монета на Main» (после двойного клика).
    pub fn take_pending_to_main(&mut self) -> Option<(CoreId, String)> {
        self.input.pending_to_main.take()
    }

    fn wheel(&mut self, delta: &MouseScrollDelta) {
        let gate = self.chart_input_ok();
        let fallback_w = self.gpu.size.width as f32;
        if self.input.wheel(delta, gate, &mut self.container, fallback_w) {
            self.dirty = true;
        }
    }

    fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        let gate = self.chart_input_ok();
        if self
            .input
            .mouse_button(button, pressed, gate, true, &mut self.container)
        {
            self.dirty = true;
        }
    }

    fn pointer_moved(&mut self, x: f32, y: f32) {
        let was = self.input.cursor.is_some();
        self.input.cursor = Some((x, y));
        if !was {
            self.window.set_cursor(CursorIcon::Crosshair);
        }
        self.input.hovered_pane = self.input.pane_at(x, y);
        self.dirty = true;
        self.input.pointer_drag(x, y, &mut self.container);
    }

    pub fn render(&mut self, session: &SessionManager, now_ms: f64) {
        let now = Instant::now();
        if now.duration_since(self.last_present_at) < MIN_FRAME_DT {
            return;
        }
        let ppp = self.window.scale_factor() as f32;
        let resolution = [self.gpu.size.width as f32, self.gpu.size.height as f32];

        let Some((frame, view, mut encoder)) = self.gpu.begin_frame("chart-window-encoder") else {
            return;
        };

        // 1. egui: верхняя строка масштаба + крестик «очистить всё».
        let raw = self.egui_state.take_egui_input(&self.window);
        let mut set_scale: Option<ScaleAction> = None;
        let mut clear_all = false;
        let scale_idx = &mut self.scale_idx;
        let full = self.egui_ctx.run(raw, |ctx| {
            egui::TopBottomPanel::top("cw-toolbar")
                .exact_height(TOPBAR_H)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal_centered(|ui| {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(t!("toolbar.scale").to_uppercase())
                                .color(theme::TEXT_3)
                                .font(theme::label_font()),
                        );
                        ui.add_space(6.0);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (i, (label, action)) in SCALES.iter().enumerate() {
                            if i > 0 {
                                ui.add_space(theme::BTN_GAP);
                            }
                            let text = if i == 0 {
                                t!("toolbar.scale_auto").to_string()
                            } else {
                                (*label).to_string()
                            };
                            if theme::seg_btn(ui, &text, *scale_idx == i, None, false).clicked() {
                                *scale_idx = i;
                                set_scale = Some(*action);
                            }
                        }
                        // Крестик «закрыть окно» (X нарисован painter'ом — глифа ✕
                        // в Geist Mono нет, был «тофу»-квадрат). У правого края.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            let (rect, resp) =
                                ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::click());
                            if ui.is_rect_visible(rect) {
                                let hovered = resp.hovered();
                                let round = egui::Rounding::same(4.0);
                                let (fill, border) = if hovered {
                                    (theme::LIFT_HOVER, theme::RED.gamma_multiply(0.7))
                                } else {
                                    (theme::LIFT, theme::BORDER)
                                };
                                let p = ui.painter();
                                p.rect_filled(rect, round, fill);
                                p.rect_stroke(rect, round, egui::Stroke::new(1.0, border));
                                let c = rect.center();
                                let r = 4.0;
                                let col = if hovered { theme::RED } else { theme::TEXT_2 };
                                let st = egui::Stroke::new(1.6, col);
                                p.line_segment(
                                    [egui::pos2(c.x - r, c.y - r), egui::pos2(c.x + r, c.y + r)],
                                    st,
                                );
                                p.line_segment(
                                    [egui::pos2(c.x - r, c.y + r), egui::pos2(c.x + r, c.y - r)],
                                    st,
                                );
                            }
                            if resp.on_hover_text(t!("chartwin.clear").to_string()).clicked() {
                                clear_all = true;
                            }
                        });
                    });
                });
        });
        // Масштаб — ко ВСЕМ графикам окна; запоминаем в контейнере (новые графики
        // откроются с ним же). Крестик — закрыть окно (App уберёт).
        if let Some(a) = set_scale {
            let pct = match a {
                ScaleAction::Auto => None,
                ScaleAction::Percent(p) => Some(p),
            };
            self.container.set_scale(pct);
        }
        if clear_all {
            self.close_requested = true;
        }
        // Истёкшие по KeepInChart авто-панели — убрать (TTL и в откреплённом окне).
        self.container.prune_ttl(now_ms);

        self.egui_state
            .handle_platform_output(&self.window, full.platform_output);
        let tris = self
            .egui_ctx
            .tessellate(full.shapes, full.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
            pixels_per_point: full.pixels_per_point,
        };
        for (id, delta) in &full.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        self.egui_renderer
            .update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &tris, &screen);

        // 2. Графики — ниже строки масштаба (чистят кадр, рисуют в свою зону).
        let area = self.area();
        let cur = self.input.cursor.filter(|(_, y)| *y >= area.y);
        let layout = crate::chart::paint::render_panes(
            &mut self.container,
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &view,
            area,
            resolution,
            ppp,
            now_ms,
            &self.theme,
            &self.orders_style,
            self.input.hovered_pane,
            cur,
            session,
        );
        self.input.pane_rects = layout.clone();

        // 3. egui-проход (строка масштаба) поверх графиков.
        {
            let mut rpass = crate::gpu::egui_pass(&mut encoder, &view, "cw-egui-pass", None);
            self.egui_renderer.render(&mut rpass, &tris, &screen);
        }

        // 4. Оверлей шкал/перекрестия.
        if !layout.is_empty() {
            crate::chart::paint::render_overlay(
                &self.container,
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
                self.input.hovered_pane,
                cur,
            );
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        for id in &full.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        self.last_present_at = now;
        self.last_visible_sig = self.visible_sig(session, now_ms);
        self.dirty = self.egui_ctx.has_requested_repaint();
    }
}
