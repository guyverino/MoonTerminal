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
use crate::chart::paint::{now_unix_ms, MIN_FRAME_DT};
use crate::chart::view::Rect;
use crate::config::ChartTheme;
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
    /// Активный пресет масштаба (индекс в `SCALES`) — подсветка кнопки.
    scale_idx: usize,

    pane_rects: Vec<(usize, Rect)>,
    hovered_pane: Option<usize>,
    cursor: Option<(f32, f32)>,
    last_ptr: (f32, f32),
    shift_down: bool,
    lmb_down: bool,
    lmb_active: bool,
    drag_accum: (f32, f32),
    rmb_down: bool,
    rmb_moved: bool,
    rmb_start_y: f32,
    rmb_start_range: f32,
    rmb_start_center: f32,
    last_lmb_ms: f64,
    last_lmb_pos: (f32, f32),
    /// Двойной клик по чарту → отправить монету на Main окна-владельца (App забирает).
    pending_to_main: Option<(CoreId, String)>,
    /// Нажат крестик «закрыть окно» в тулбаре (App уберёт окно).
    close_requested: bool,

    dirty: bool,
    last_present_at: Instant,
    last_visible_sig: u64,
}

impl ChartWindow {
    /// Создаёт окно и контейнер из спецификации панелей (ядро/рынок/источник).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_loop: &ActiveEventLoop,
        owner: WindowId,
        title: &str,
        kind: ContainerKind,
        mode: Mode,
        spec: Vec<(CoreId, String, PaneSource)>,
        theme: ChartTheme,
        epoch_ms: f64,
    ) -> anyhow::Result<Self> {
        // Заголовок окна — переданное имя (формируется из номера и группы, см. App).
        let attrs = Window::default_attributes()
            .with_title(format!("{title} — MoonTerminal"))
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 700.0));
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
            scale_idx: 0,
            pane_rects: Vec::new(),
            hovered_pane: None,
            cursor: None,
            last_ptr: (0.0, 0.0),
            shift_down: false,
            lmb_down: false,
            lmb_active: false,
            drag_accum: (0.0, 0.0),
            rmb_down: false,
            rmb_moved: false,
            rmb_start_y: 0.0,
            rmb_start_range: 0.0,
            rmb_start_center: 0.0,
            last_lmb_ms: 0.0,
            last_lmb_pos: (0.0, 0.0),
            pending_to_main: None,
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
        self.last_ptr.1 >= TOPBAR_H * self.window.scale_factor() as f32
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
            WindowEvent::ModifiersChanged(m) => self.shift_down = m.state().shift_key(),
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
        if self.cursor.take().is_some() {
            self.hovered_pane = None;
            self.window.set_cursor(CursorIcon::Default);
            self.dirty = true;
        }
    }

    /// Двойной клик по чарту (не стакану) панели под курсором → запомнить монету
    /// для отправки на Main окна-владельца (App заберёт через `take_pending_to_main`).
    fn try_dblclick_to_main(&mut self) {
        let Some(idx) = self.hovered_pane else { return };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return;
        };
        let glass_w = crate::chart::GLASS_ZONE_PX.min(r.w * 0.5);
        if self.last_ptr.0 >= r.x + r.w - glass_w {
            return; // в стакане
        }
        self.pending_to_main = self
            .container
            .panes
            .get(idx)
            .map(|p| (p.core, p.market.clone()));
    }

    /// Забрать запрос «монета на Main» (после двойного клика).
    pub fn take_pending_to_main(&mut self) -> Option<(CoreId, String)> {
        self.pending_to_main.take()
    }

    fn hovered_pane_w(&self) -> f32 {
        self.pane_rects
            .iter()
            .find(|(i, _)| Some(*i) == self.hovered_pane)
            .map(|(_, r)| r.w)
            .unwrap_or(self.gpu.size.width as f32)
    }

    fn hovered_view_mut(&mut self) -> Option<&mut crate::chart::view::ChartView> {
        let idx = self.hovered_pane?;
        self.container.panes.get_mut(idx).map(|p| &mut p.chart.view)
    }

    fn wheel(&mut self, delta: &MouseScrollDelta) {
        if !self.chart_input_ok() {
            return;
        }
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => *y,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
        };
        if dy == 0.0 {
            return;
        }
        let shift = self.shift_down;
        let w = self.hovered_pane_w();
        let now = now_unix_ms();
        if let Some(view) = self.hovered_view_mut() {
            if shift {
                view.pan_x_px(-dy.signum() * 60.0, now);
            } else {
                let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                view.zoom_x(factor, w);
            }
            self.dirty = true;
        }
    }

    fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        match button {
            MouseButton::Left => {
                if pressed {
                    if !self.chart_input_ok() {
                        return; // клик по строке масштаба — не трогаем графики
                    }
                    // Двойной клик по ЧАРТУ (не стакану) → монета на Main владельца.
                    let now = now_unix_ms();
                    let (px, py) = self.last_ptr;
                    let dbl = now - self.last_lmb_ms < 400.0
                        && (px - self.last_lmb_pos.0).abs() < 28.0
                        && (py - self.last_lmb_pos.1).abs() < 28.0;
                    self.last_lmb_ms = now;
                    self.last_lmb_pos = (px, py);
                    if dbl {
                        self.try_dblclick_to_main();
                    }
                    self.lmb_down = true;
                    self.lmb_active = false;
                    self.drag_accum = (0.0, 0.0);
                } else {
                    self.lmb_down = false;
                    self.lmb_active = false;
                }
            }
            MouseButton::Right => {
                if pressed {
                    if !self.chart_input_ok() {
                        return;
                    }
                    self.rmb_down = true;
                    self.rmb_moved = false;
                    self.rmb_start_y = self.last_ptr.1;
                    let snap = self.hovered_view_mut().map(|v| (v.price_range, v.center_price));
                    if let Some((r, c)) = snap {
                        self.rmb_start_range = r;
                        self.rmb_start_center = c;
                    }
                } else {
                    // ПКМ-клик без сдвига → тоггл фулскрин/тайл (фокус — под курсором).
                    if self.rmb_down && !self.rmb_moved && !self.container.is_empty() {
                        let focus = self.hovered_pane.unwrap_or(0);
                        self.container.toggle_mode(focus);
                    }
                    self.rmb_down = false;
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn pointer_moved(&mut self, x: f32, y: f32) {
        let dx = x - self.last_ptr.0;
        let dy = y - self.last_ptr.1;
        self.last_ptr = (x, y);
        let was = self.cursor.is_some();
        self.cursor = Some((x, y));
        if !was {
            self.window.set_cursor(CursorIcon::Crosshair);
        }
        self.hovered_pane = self
            .pane_rects
            .iter()
            .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            .map(|(i, _)| *i);
        self.dirty = true;

        if self.lmb_down {
            if !self.lmb_active {
                self.drag_accum.0 += dx;
                self.drag_accum.1 += dy;
                if self.drag_accum.0.abs() > 5.0 || self.drag_accum.1.abs() > 5.0 {
                    self.lmb_active = true;
                }
            }
            if self.lmb_active {
                let now = now_unix_ms();
                if let Some(view) = self.hovered_view_mut() {
                    if dx != 0.0 {
                        view.pan_x_px(dx, now);
                    }
                    if dy != 0.0 {
                        view.pan_y_px(dy, now);
                    }
                }
            }
        }
        if self.rmb_down {
            let cum = y - self.rmb_start_y;
            if cum.abs() > 4.0 {
                self.rmb_moved = true;
            }
            let (c, r) = (self.rmb_start_center, self.rmb_start_range);
            let now = now_unix_ms();
            if let Some(view) = self.hovered_view_mut() {
                view.rmb_zoom(c, r, cum, now);
            }
        }
    }

    pub fn render(&mut self, session: &SessionManager, now_ms: f64) {
        let now = Instant::now();
        if now.duration_since(self.last_present_at) < MIN_FRAME_DT {
            return;
        }
        let ppp = self.window.scale_factor() as f32;
        let resolution = [self.gpu.size.width as f32, self.gpu.size.height as f32];

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.gpu.surface.configure(&self.gpu.device, &self.gpu.config);
                return;
            }
            Err(e) => {
                log::warn!("chart-window surface error: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("chart-window-encoder"),
            });

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
        // Масштаб — ко ВСЕМ графикам окна. Крестик — закрыть окно (App уберёт).
        if let Some(a) = set_scale {
            for p in &mut self.container.panes {
                match a {
                    ScaleAction::Auto => p.chart.view.set_auto(),
                    ScaleAction::Percent(pct) => p.chart.view.set_scale_percent(pct),
                }
            }
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
        let cur = self.cursor.filter(|(_, y)| *y >= area.y);
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
            self.hovered_pane,
            cur,
            session,
        );
        self.pane_rects = layout.clone();

        // 3. egui-проход (строка масштаба) поверх графиков.
        {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cw-egui-pass"),
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
                self.hovered_pane,
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
