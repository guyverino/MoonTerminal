//! WindowHost — одно ОС-окно одной группы: свой surface + egui + chart + dock.
//! App держит по WindowHost на группу. CoreStore общий (читается, не владеется).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winit::dpi::PhysicalSize;
use winit::event::{MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window};

use crate::chart::container::{Container, ContainerKind, Mode, Pane};
use crate::chart::view::Rect;
use crate::config::ChartTheme;
use crate::session::CoreId;
use crate::feed::{ConnStatus, OrderRow};
use crate::gpu::GpuContext;
use crate::metrics::MetricsSnapshot;
use crate::session::{CoreStore, SessionManager};
use crate::shell::{Shell, ShellInfo, HEADER_H, STATUS_H};
use crate::workspace::Workspace;

pub struct HostRender {
    pub gear_clicked: bool,
    pub strategies_clicked: bool,
    /// Вкладку дока потянули — открепить в окно (App создаёт окно для этого host'а).
    pub detach: Option<crate::dock::DockTab>,
    /// Нажата «вернуть в док» — App закроет окно открепления этой вкладки.
    pub repin: Option<crate::dock::DockTab>,
}

/// Принудительный прогон egui хотя бы раз в этот интервал — освежает живые
/// счётчики статус-бара (fps/present/CPU/RAM), которые исключены из хром-сигнатуры.
const EGUI_THROTTLE: Duration = Duration::from_millis(400);

/// Кап частоты кадров: не презентим чаще этого. Движение мыши (перекрестие) и
/// smooth-скролл иначе упираются в развёртку монитора (120/144 Гц) и греют GPU
/// зря — следящему курсору хватает ~60/с. 16_666 мкс ≈ 60 fps; для более
/// гладкого скролла на 120/144-Гц мониторе уменьши (8_333 ≈ 120, 6_944 ≈ 144).
const MIN_FRAME_DT: Duration = Duration::from_micros(16_666);

/// Текущее unix-время в мс (та же шкала, что приходит в render как now_ms).
fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Смещение локального времени от UTC, сек (для подписей часов на шкале времени).
/// Считаем как разницу «час:мин:сек» локального и системного (UTC) времени —
/// учитывает текущий DST без сторонних крейтов. На не-Windows — 0 (UTC).
#[cfg(windows)]
fn local_offset_sec() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let lsec = l.wHour as i64 * 3600 + l.wMinute as i64 * 60 + l.wSecond as i64;
    let usec = u.wHour as i64 * 3600 + u.wMinute as i64 * 60 + u.wSecond as i64;
    let mut d = lsec - usec;
    if d > 43_200 {
        d -= 86_400;
    } else if d < -43_200 {
        d += 86_400;
    }
    d
}
#[cfg(not(windows))]
fn local_offset_sec() -> i64 {
    0
}

pub struct WindowHost {
    pub window: Arc<Window>,
    gpu: GpuContext,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    /// Лёгкий overlay-контекст шкал/перекрестных readout'ов: гоняется КАЖДЫМ
    /// кадром (как wgpu-перекрестие), не кэшируется вместе с хромом — поэтому
    /// шкала времени едет за паном/скроллом, а readout'ы — за курсором.
    overlay_ctx: egui::Context,
    overlay_renderer: egui_wgpu::Renderer,
    shell: Shell,
    /// Контейнеры графиков: [0] — главный (ручные клики, фулскрин-центричный),
    /// [1..] — AddToChart (по группам, тайл). Активный показывается в центр. зоне.
    containers: Vec<Container>,
    active_container: usize,
    /// Раскладка панелей активного контейнера (физ. px) на прошлом кадре — для
    /// hit-теста ввода (какая панель под курсором).
    pane_rects: Vec<(usize, Rect)>,
    /// Панель под курсором (индекс в активном контейнере).
    hovered_pane: Option<usize>,
    /// Per-core курсор уже учтённых детектов для AddToChart-ингеста.
    add_seq: HashMap<CoreId, u64>,
    /// Epoch для создания новых панелей (Chart::new) на лету.
    epoch_ms: f64,
    pub workspace: Workspace,
    /// Тема оформления чарта (приходит из App; смена → dirty-кадр).
    theme: ChartTheme,
    cursor: Option<(f32, f32)>,
    last_frame: Instant,
    /// Время последнего present — для капа частоты кадров (MIN_FRAME_DT).
    last_present_at: Instant,
    fps: f32,
    /// Метки реальных present() за последнюю секунду → честный present/s
    /// (диагностика нагрузки: idle после edge-driven фикса должен давать ~0).
    present_marks: VecDeque<Instant>,
    present_hz: f32,

    // ввод / интерактив (порт ChartInteraction из moonweb), всё в физ. пикселях.
    shift_down: bool,
    last_ptr: (f32, f32),
    /// Прямоугольник зоны графика (физ. px) — hit-тест ввода. egui НЕ годится
    /// для гейта: CentralPanel поверх чарта считается «областью под курсором».
    chart_area: (f32, f32, f32, f32),
    /// Прямоугольник правого дока детектов (физ. px) — движение курсора над ним
    /// форсит перетесселяцию egui (живой spotlight на кнопках).
    detects_area: (f32, f32, f32, f32),
    lmb_down: bool,
    lmb_active: bool,
    drag_accum: (f32, f32),
    rmb_down: bool,
    rmb_start_y: f32,
    rmb_start_range: f32,
    rmb_start_center: f32,
    /// ПКМ сдвинулся за порог → это зум-перетаскивание, а не клик-тоггл фулскрина.
    rmb_moved: bool,

    // dirty-трекинг для skip-present.
    dirty: bool,
    last_orders_sig: u64,
    last_detects_sig: u64,
    /// Сигнатура видимых панелей (рыночные ревизии + край времени) на прошлом
    /// кадре — для гейта needs_render по всем видимым панелям активного контейнера.
    last_visible_sig: u64,
    icons: crate::icons::IconSet,

    // egui-mesh cache (2b): переиспользуем тесселяцию между кадрами, пока хром не
    // изменился. Smooth scroll / hover над графиком не гоняют egui layout+
    // tessellate каждый кадр → CPU на кадр падает. egui_tris=None → кэша нет.
    egui_tris: Option<Vec<egui::ClippedPrimitive>>,
    egui_ppp: f32,
    egui_area: Rect,
    /// egui попросил перерисовку (анимация/popup) → не reuse в след. кадре.
    egui_wants_repaint: bool,
    /// Был egui-релевантный ввод → перегнать хром (а не reuse).
    egui_dirty: bool,
    /// Сигнатура содержимого хрома (рынок/статус/цена/ордера) на момент кэша.
    last_chrome_sig: u64,
    last_egui_run: Instant,
    egui_cache_size: (u32, u32),
}

impl WindowHost {
    pub fn new(
        event_loop: &ActiveEventLoop,
        mut workspace: Workspace,
        epoch_ms: f64,
        layout: Option<crate::config::GroupLayout>,
    ) -> anyhow::Result<Self> {
        let mut attrs = Window::default_attributes()
            .with_title(format!("MoonTerminal — {}", workspace.group))
            .with_inner_size(winit::dpi::LogicalSize::new(1100.0, 720.0));
        // Восстановление раскладки: позиция/размер окна + активная вкладка/свёрнутость.
        if let Some(l) = &layout {
            attrs = attrs
                .with_position(winit::dpi::PhysicalPosition::new(l.x, l.y))
                .with_inner_size(winit::dpi::PhysicalSize::new(l.w.max(200), l.h.max(150)));
            workspace
                .dock
                .restore(crate::dock::DockTab::from_idx(l.tab as usize), l.collapsed);
        }
        let window = Arc::new(event_loop.create_window(attrs)?);
        if layout.as_ref().map(|l| l.maximized).unwrap_or(false) {
            window.set_maximized(true);
        }
        // Своя кнопка в taskbar на группу (Windows) + иконка группы.
        crate::win_taskbar::set_app_id(&window, &format!("MoonTerminal.Group.{}", workspace.group));
        window.set_window_icon(crate::icons::winit_icon(workspace.icon));

        let gpu = GpuContext::new(window.clone())?;

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);
        // Overlay-контекст: свои шрифты/стиль проекта (Geist Mono), свой рендерер
        // (отдельный атлас глифов). Ввод ему не маршрутизируем — рисуем по снимку.
        let overlay_ctx = egui::Context::default();
        crate::shell::theme::apply(&overlay_ctx);
        let overlay_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);
        let shell = Shell::new(&egui_ctx);

        Ok(Self {
            window,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            overlay_ctx,
            overlay_renderer,
            shell,
            containers: vec![Container::new(ContainerKind::Main)],
            active_container: 0,
            pane_rects: Vec::new(),
            hovered_pane: None,
            add_seq: HashMap::new(),
            epoch_ms,
            workspace,
            theme: ChartTheme::default(),
            cursor: None,
            last_frame: Instant::now(),
            last_present_at: Instant::now() - MIN_FRAME_DT,
            fps: 0.0,
            present_marks: VecDeque::new(),
            present_hz: 0.0,
            shift_down: false,
            last_ptr: (0.0, 0.0),
            chart_area: (0.0, 0.0, 0.0, 0.0),
            detects_area: (0.0, 0.0, 0.0, 0.0),
            lmb_down: false,
            lmb_active: false,
            drag_accum: (0.0, 0.0),
            rmb_down: false,
            rmb_start_y: 0.0,
            rmb_start_range: 0.0,
            rmb_start_center: 0.0,
            rmb_moved: false,
            dirty: true,
            last_orders_sig: u64::MAX,
            last_detects_sig: u64::MAX,
            last_visible_sig: u64::MAX,
            icons: crate::icons::IconSet::discover(),
            egui_tris: None,
            egui_ppp: 1.0,
            egui_area: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            egui_wants_repaint: false,
            egui_dirty: true,
            last_chrome_sig: 0,
            last_egui_run: Instant::now(),
            egui_cache_size: (0, 0),
        })
    }

    /// egui-релевантный ввод (клик/колесо/клавиша/ресайз/таск egui-виджета):
    /// на следующем кадре перегоняем хром заново (не reuse) и рисуем кадр.
    pub fn mark_egui_dirty(&mut self) {
        self.egui_dirty = true;
        self.dirty = true;
    }

    /// Активный контейнер (показывается в центральной зоне).
    fn active(&self) -> &Container {
        &self.containers[self.active_container]
    }
    fn active_mut(&mut self) -> &mut Container {
        &mut self.containers[self.active_container]
    }

    /// Фокус-панель активного контейнера (для шапки: рынок/цена/статус).
    fn focused(&self) -> Option<&Pane> {
        let c = self.active();
        let idx = match c.mode {
            Mode::Fullscreen(i) => i,
            Mode::Tiled => 0,
        };
        c.panes.get(idx)
    }

    /// Все открытые (ядро, рынок) по всем контейнерам — для подписок (App).
    pub fn open_markets(&self) -> Vec<(CoreId, String)> {
        let mut out = Vec::new();
        for c in &self.containers {
            for p in &c.panes {
                out.push((p.core, p.market.clone()));
            }
        }
        out
    }

    /// Втянуть свежие AddToChart-детекты ядер группы в общий AddToChart-контейнер
    /// (создаётся лениво по первому такому детекту). TTL панели = KeepInChart.
    fn ingest_addtochart(&mut self, store: &CoreStore, now_ms: f64) -> bool {
        let mut adds: Vec<(CoreId, String, f64)> = Vec::new();
        for ci in &self.workspace.cores {
            let Some(d) = store.core(ci.id) else { continue };
            let last = self.add_seq.get(&ci.id).copied().unwrap_or(0);
            let mut newest = last;
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                newest = newest.max(det.seq);
                if det.add_to_chart {
                    let ttl = (det.keep_in_chart_secs.max(1) as f64) * 1000.0;
                    adds.push((ci.id, det.market.clone(), ttl));
                }
            }
            if newest != last {
                self.add_seq.insert(ci.id, newest);
            }
        }
        if adds.is_empty() {
            return false;
        }
        if !self.containers.iter().any(|c| c.kind == ContainerKind::AddToChart) {
            self.containers.push(Container::new(ContainerKind::AddToChart));
        }
        let ci_idx = self
            .containers
            .iter()
            .position(|c| c.kind == ContainerKind::AddToChart)
            .unwrap();
        let (fmt, epoch) = (self.gpu.format, self.epoch_ms);
        // Свежие детекты идут с конца (новые) — добавляем в обратном порядке (старые
        // выше), панель монеты дедуплицируется (продлевается TTL).
        for (core, market, ttl) in adds.into_iter().rev() {
            self.containers[ci_idx].push_auto(core, &market, now_ms, ttl, &self.gpu.device, fmt, epoch);
        }
        true
    }

    /// Удалить истёкшие AddToChart-панели (контейнеры остаются). True — если
    /// что-то удалили (нужен пересчёт фокуса/раскладки).
    fn prune_panes(&mut self, now_ms: f64) -> bool {
        let mut changed = false;
        for c in &mut self.containers {
            changed |= c.prune_ttl(now_ms);
        }
        changed
    }

    /// Сигнатура видимых панелей активного контейнера: рыночные ревизии + край
    /// времени каждой видимой панели. Меняется → нужен кадр.
    fn visible_sig(&self, session: &SessionManager, now_ms: f64) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let c = self.active();
        // Видимые индексы зависят от режима, но дёшевле просто пройтись по всем
        // (в фулскрине невидимые панели всё равно не меняют картинку, но их edge
        // не двигается заметно — приемлемо).
        for p in &c.panes {
            if let Some(v) = session.market_view(p.core, &p.market) {
                v.ticks_rev.hash(&mut h);
                v.book_rev.hash(&mut h);
            }
            let edge = if p.chart.view.is_live(now_ms) {
                now_ms
            } else {
                p.chart.view.right_time_ms
            };
            p.chart.view.pixel_at(edge).hash(&mut h);
        }
        h.finish()
    }

    /// Сумма detects_rev ядер группы — дёшево ловит приход новых детектов.
    fn detects_sig(&self, store: &CoreStore) -> u64 {
        let mut sig = 0u64;
        for ci in &self.workspace.cores {
            if let Some(d) = store.core(ci.id) {
                sig = sig.wrapping_add(d.detects_rev);
            }
        }
        sig
    }

    /// Нужно ли перерисовывать (иначе skip render + skip present).
    /// Перерисовка при: новых тиках/стакане/ордерах, вводе/UI, ИЛИ сдвиге края
    /// на целый пиксель wall-clock-времени (smooth follow). На паузе/ручном
    /// удержании край заморожен → кадр пропускается. В лайве край едет за
    /// «сейчас» → ~px_per_ms·1000 кадров/с (дёшево за счёт canvas+egui-cache).
    pub fn needs_render(&self, session: &SessionManager, now_ms: f64) -> bool {
        let store = session.store();
        if self.dirty {
            return true;
        }
        // Рыночные ревизии + край времени видимых панелей активного контейнера.
        if self.visible_sig(session, now_ms) != self.last_visible_sig {
            return true;
        }
        let mut sig = 0u64;
        for ci in &self.workspace.cores {
            if let Some(d) = store.core(ci.id) {
                sig = sig.wrapping_add(d.orders_rev);
            }
        }
        if sig != self.last_orders_sig {
            return true;
        }
        // Новые детекты ядер группы → кадр (иначе при закрытом чарте лента не
        // обновится). Пока в ленте есть кнопки — гоним кадры для их TTL-истечения.
        if self.detects_sig(store) != self.last_detects_sig {
            return true;
        }
        if self.workspace.dock.ribbon.has_items() {
            return true;
        }
        // AddToChart-панели истекают по TTL — гоним кадры, пока такие есть.
        if self.containers.iter().any(|c| c.has_ttl_panes()) {
            return true;
        }
        // Живые вкладки Лог/Отчёт — общие на все окна; их обновление форсит App
        // (mark_egui_dirty по ревизии). Движение края учтено в visible_sig выше.
        false
    }

    pub fn on_egui_event(&mut self, event: &WindowEvent) -> bool {
        self.egui_state.on_window_event(&self.window, event).consumed
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.gpu.resize(size);
    }

    /// Применить тему. Смена → dirty-кадр (живое превью из Настроек). Фон
    /// egui-панелей (тулбар/ордер/док/статус) применяем тут же — он живёт в
    /// egui-visuals, не в шейдере; меняем только при изменении (перетесселяция).
    pub fn set_theme(&mut self, theme: &ChartTheme) {
        if &self.theme == theme {
            return;
        }
        let panel_changed = self.theme.panel_bg != theme.panel_bg;
        self.theme = theme.clone();
        if panel_changed {
            let p = theme.panel_bg;
            let mut style = (*self.egui_ctx.style()).clone();
            style.visuals.panel_fill = egui::Color32::from_rgb(p[0], p[1], p[2]);
            self.egui_ctx.set_style(style);
            self.egui_dirty = true; // перетесселировать хром с новым фоном
        }
        self.dirty = true;
    }

    pub fn clear_cursor(&mut self) {
        // Кадр нужен только если перекрестие было показано (надо стереть).
        if self.cursor.take().is_some() {
            self.window.set_cursor(CursorIcon::Default);
            self.dirty = true;
        }
    }

    /// Состояние Shift (для shift+колесо = пан по X).
    pub fn set_modifiers(&mut self, shift: bool) {
        self.shift_down = shift;
    }

    /// Указатель в зоне графика (а не над панелями egui)? Чисто геометрия:
    /// все интерактивные виджеты egui живут в панелях ВНЕ центрального rect.
    fn chart_input_ok(&self) -> bool {
        let (x, y) = self.last_ptr;
        let (cx, cy, cw, ch) = self.chart_area;
        x >= cx && x <= cx + cw && y >= cy && y <= cy + ch
    }

    /// Ширина rect панели под курсором (физ. px) — для клампа зума по X.
    fn hovered_pane_w(&self) -> f32 {
        self.pane_rects
            .iter()
            .find(|(i, _)| Some(*i) == self.hovered_pane)
            .map(|(_, r)| r.w)
            .unwrap_or(self.chart_area.2)
    }

    /// `view` панели под курсором (для пан/зум). None — курсор не над панелью.
    fn hovered_view_mut(&mut self) -> Option<&mut crate::chart::view::ChartView> {
        let idx = self.hovered_pane?;
        self.containers[self.active_container]
            .panes
            .get_mut(idx)
            .map(|p| &mut p.chart.view)
    }

    /// Колесо: зум по X (или пан по X при зажатом Shift) — у панели под курсором.
    pub fn wheel(&mut self, delta: &MouseScrollDelta) {
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

    /// Нажатие/отпускание кнопки мыши. Гейт по зоне графика (не по egui). ПКМ:
    /// короткий клик (без сдвига) = тоггл фулскрин↔тайл; ПКМ-drag = зум по цене.
    pub fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        match button {
            MouseButton::Left => {
                if pressed {
                    if !self.chart_input_ok() {
                        return; // клик по панели — не таскаем график
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
                    // Отпустили ПКМ без сдвига → клик: тоггл фулскрин/тайл (фокус —
                    // панель под курсором). Со сдвигом — это был зум по цене.
                    if self.rmb_down && !self.rmb_moved {
                        let focus = self.hovered_pane.unwrap_or(0);
                        if !self.active().is_empty() {
                            self.active_mut().toggle_mode(focus);
                        }
                    }
                    self.rmb_down = false;
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Движение курсора (физ. пиксели).
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        let dx = x - self.last_ptr.0;
        let dy = y - self.last_ptr.1;
        self.last_ptr = (x, y);
        let (cx, cy, cw, ch) = self.chart_area;
        let active = x >= cx && x <= cx + cw && y >= cy && y <= cy + ch;
        let was_shown = self.cursor.is_some();
        self.cursor = if active { Some((x, y)) } else { None };
        // Панель под курсором (для маршрутизации пан/зум и крестика).
        self.hovered_pane = if active {
            self.pane_rects
                .iter()
                .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
                .map(|(i, _)| *i)
        } else {
            None
        };
        // Системный курсор-крестик над графиком; шлём только при смене состояния.
        if active != was_shown {
            self.window
                .set_cursor(if active { CursorIcon::Crosshair } else { CursorIcon::Default });
        }
        // Перекрестие живёт в зоне графика: движение над графиком или уход с него
        // → нужен кадр. Над панелями egui (не было показано и не показано) кадр
        // не форсируем — это и есть бывший «шторм» от движения мыши.
        if active || was_shown {
            self.dirty = true;
        }

        // Курсор над доком детектов → перегон egui (spotlight на кнопке следует за
        // мышью). Точечно: только эта узкая колонка, не все панели (без «шторма»).
        let (dx0, dy0, dw, dh) = self.detects_area;
        if x >= dx0 && x <= dx0 + dw && y >= dy0 && y <= dy0 + dh {
            self.egui_dirty = true;
            self.dirty = true;
        }

        // ЛКМ-перетаскивание: горизонталь → пан по времени, вертикаль → пан по цене.
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
            self.dirty = true;
        }

        // ПКМ-перетаскивание: вертикальный зум по цене от снимка нажатия. Сдвиг за
        // порог помечает rmb_moved → на отпускании это зум, а не клик-тоггл.
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
            self.dirty = true;
        }
    }

    /// Открытые ордера всех ядер группы (с именем ядра) — для вкладки «Ордера»
    /// дока и её окна открепления. Дёшево копирует строки на кадр.
    pub fn collect_orders(&self, store: &CoreStore) -> Vec<(String, OrderRow)> {
        let mut rows = Vec::new();
        for ci in &self.workspace.cores {
            if let Some(d) = store.core(ci.id) {
                for o in &d.orders {
                    rows.push((ci.name.clone(), o.clone()));
                }
            }
        }
        rows
    }

    /// Сумма orders_rev ядер группы — дёшево ловит изменение набора ордеров (для
    /// живого обновления откреплённого окна вкладки «Ордера»).
    pub fn orders_rev(&self, store: &CoreStore) -> u64 {
        let mut sig = 0u64;
        for ci in &self.workspace.cores {
            if let Some(d) = store.core(ci.id) {
                sig = sig.wrapping_add(d.orders_rev);
            }
        }
        sig
    }

    pub fn render(
        &mut self,
        session: &SessionManager,
        now_ms: f64,
        metrics: MetricsSnapshot,
        report: &mut crate::dock::ReportView,
        global_detached: [bool; 4],
    ) -> HostRender {
        let store = session.store();
        let none = HostRender {
            gear_clicked: false,
            strategies_clicked: false,
            detach: None,
            repin: None,
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

        // AddToChart: втянуть свежие детекты в общий контейнер; убрать истёкшие.
        let added = self.ingest_addtochart(store, now_ms);
        let pruned = self.prune_panes(now_ms);
        // Появился/исчез контейнер или панели → перестроить хром (верхние вкладки).
        if added || pruned {
            self.egui_dirty = true;
        }

        let ppp = self.window.scale_factor() as f32;
        let resolution = [self.gpu.size.width as f32, self.gpu.size.height as f32];

        // Шапка/статус — по фокус-панели активного контейнера (или пусто).
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

        // 2b: гоняем egui (layout+tessellate) только когда реально надо. Иначе —
        // переиспользуем кэш-сетку: smooth scroll и hover над графиком не меняют
        // хром → нет CPU на тесселяцию хрома каждый кадр.
        let size_now = (self.gpu.size.width, self.gpu.size.height);
        let sig = chrome_sig(&market, &status, last_price, orders_sig, conn_sig);
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
        let mut to_free: Vec<egui::TextureId> = Vec::new();

        if run_egui {
            let raw_input = self.egui_state.take_egui_input(&self.window);
            let group_name = self.workspace.group.clone();
            let info = ShellInfo {
                group: &group_name,
                conn_ready: conn.ready,
                conn_total: conn.total,
                conn_down: &conn.down,
                tick_count,
                book_levels,
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
                    ctx, cores, store, &order_rows, following, chart_open, now_ms,
                    report, global_detached,
                );
                central = out.central;
                detects_rect = out.detects_rect;
                set_scale = out.scale;
                set_follow = out.set_follow;
                open_detect = out.open_detect;
                close_chart = out.close_chart;
                detach_req = out.detach;
                repin_req = out.repin;

                // Верхняя полоса вкладок-контейнеров (видна, когда контейнеров >1):
                // переключает активный контейнер. Реально появляется по первому
                // AddToChart-детекту (создаётся второй контейнер).
                let tabs_h = if containers.len() > 1 { 28.0 } else { 0.0 };
                if tabs_h > 0.0 && central.is_positive() {
                    egui::Area::new(egui::Id::new("chart-tabs"))
                        .fixed_pos(central.min)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                for (i, c) in containers.iter().enumerate() {
                                    let label = match c.kind {
                                        ContainerKind::Main => t!("chart.tab.main").to_string(),
                                        ContainerKind::AddToChart => "AddToChart".to_string(),
                                    };
                                    let sel = i == active_container;
                                    if crate::shell::theme::seg_btn(ui, &label, sel, None, false)
                                        .clicked()
                                    {
                                        switch_to = Some(i);
                                    }
                                    ui.add_space(crate::shell::theme::BTN_GAP);
                                }
                            });
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
                            crate::dock::ScaleAction::Percent(pct) => {
                                p.chart.view.set_scale_percent(pct)
                            }
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
            let _ = close_chart;
            // Кнопка «Отчёты» в шапке теперь выбирает вкладку «Отчёт» дока (а не
            // открывает отдельное окно): отчёт живёт во вкладке, окном становится
            // только при откреплении. Если вкладка откреплена — App сфокусит окно.
            if reports_clicked {
                self.workspace.dock.tab = crate::dock::DockTab::Report;
                layout_changed = true;
            }

            // Полоса вкладок-контейнеров (если >1) съедает верх центральной зоны.
            let tabs_h = if self.containers.len() > 1 { 28.0 } else { 0.0 };
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
                .update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &tris, &screen);

            self.egui_tris = Some(tris);
            self.egui_ppp = full_output.pixels_per_point;
            self.egui_wants_repaint = self.egui_ctx.has_requested_repaint();
            self.last_chrome_sig = sig;
            self.last_egui_run = now;
            self.egui_cache_size = size_now;
            self.egui_dirty = false;
            to_free = full_output.textures_delta.free;
        }

        // Чарт(ы): панели активного контейнера в их полосы. Раскладку (физ. px)
        // запоминаем для hit-теста ввода. Первая панель чистит кадр, остальные —
        // поверх (Load), иначе их clear стёр бы соседей.
        let area = self.egui_area;
        self.chart_area = (area.x, area.y, area.w, area.h);
        let cur = self
            .cursor
            .filter(|(_, y)| *y >= area.y && *y <= area.y + area.h);
        let ac = self.active_container;
        let layout = self.containers[ac].layout(area);
        self.pane_rects = layout.clone();
        let render_open = !layout.is_empty();
        if !render_open {
            // Пустой контейнер — серый фон (closed_bg).
            let cb = self.theme.closed_bg;
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chart-empty"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: crate::chart::srgb_to_linear(cb[0]),
                            g: crate::chart::srgb_to_linear(cb[1]),
                            b: crate::chart::srgb_to_linear(cb[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        } else {
            let hovered = self.hovered_pane;
            for (n, (idx, rect)) in layout.iter().enumerate() {
                let clear = n == 0;
                let (core, market) = {
                    let p = &self.containers[ac].panes[*idx];
                    (p.core, p.market.clone())
                };
                let data = session.market_view(core, &market);
                let pcur = if hovered == Some(*idx) { cur } else { None };
                let pane = &mut self.containers[ac].panes[*idx];
                pane.chart.set_cursor(pcur);
                pane.chart.render(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &mut encoder,
                    &view,
                    *rect,
                    resolution,
                    ppp,
                    now_ms,
                    data,
                    true,
                    clear,
                    &self.theme,
                );
            }
        }

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
        // Гоняется КАЖДЫМ кадром по снимку вида (не кэшируется): шкала времени
        // привязана к данным → едет за паном/скроллом, readout'ы — за курсором.
        if render_open && area.w > 1.0 && area.h > 1.0 {
            // Снимки осей + rect (точки egui) для каждой видимой панели; курсор —
            // только у панели под мышью.
            let hovered = self.hovered_pane;
            let mut overlays: Vec<(crate::chart::axes::AxisSnapshot, egui::Rect, Option<egui::Pos2>)> =
                Vec::new();
            for (idx, rect) in &layout {
                let v = &self.containers[ac].panes[*idx].chart.view;
                let snap = crate::chart::axes::AxisSnapshot {
                    px_per_ms: v.px_per_ms,
                    right_margin_frac: v.right_margin_frac,
                    render_center: v.render_center,
                    render_range: v.render_range,
                    epoch_ms: v.epoch_ms,
                    right_time_ms: v.right_time_ms,
                    tz_offset_sec: local_offset_sec(),
                };
                let prect = egui::Rect::from_min_size(
                    egui::pos2(rect.x / ppp, rect.y / ppp),
                    egui::vec2(rect.w / ppp, rect.h / ppp),
                );
                let pcur = if hovered == Some(*idx) {
                    cur.map(|(x, y)| egui::pos2(x / ppp, y / ppp))
                } else {
                    None
                };
                overlays.push((snap, prect, pcur));
            }

            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(resolution[0] / ppp, resolution[1] / ppp),
                )),
                ..Default::default()
            };
            let vid = raw.viewport_id;
            raw.viewports.entry(vid).or_default().native_pixels_per_point = Some(ppp);

            let out = self.overlay_ctx.run(raw, |ctx| {
                let p = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("axes-overlay"),
                ));
                for (snap, prect, pcur) in &overlays {
                    crate::chart::axes::draw(&p, *prect, ppp, snap, *pcur);
                }
            });
            let otris = self.overlay_ctx.tessellate(out.shapes, out.pixels_per_point);
            for (id, delta) in &out.textures_delta.set {
                self.overlay_renderer
                    .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
            let oscreen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
                pixels_per_point: ppp,
            };
            self.overlay_renderer.update_buffers(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &otris,
                &oscreen,
            );
            {
                let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("axes-overlay-pass"),
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
                self.overlay_renderer.render(&mut rpass, &otris, &oscreen);
            }
            for id in &out.textures_delta.free {
                self.overlay_renderer.free_texture(id);
            }
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        self.last_present_at = now;
        self.present_marks.push_back(now);
        for tex_id in &to_free {
            self.egui_renderer.free_texture(tex_id);
        }

        self.last_orders_sig = orders_sig;
        self.last_detects_sig = self.detects_sig(store);
        self.last_visible_sig = self.visible_sig(session, now_ms);
        // Следующий кадр держим «грязным», если egui анимирует (popup/fade) ИЛИ
        // только что открыли/закрыли чарт ИЛИ удалили истёкшие панели.
        self.dirty = self.egui_wants_repaint || layout_changed || pruned;

        HostRender {
            gear_clicked,
            strategies_clicked,
            detach: detach_req,
            repin: repin_req,
        }
    }
}

/// Сигнатура содержимого хрома: меняется только при смене рынка/статуса/цены
/// (до копеек) / набора ордеров. Живые счётчики статус-бара (fps/present/CPU/RAM)
/// СЮДА НЕ входят — их освежает EGUI_THROTTLE, иначе хром «менялся» бы каждый кадр.
fn chrome_sig(
    market: &str,
    status: &ConnStatus,
    last_price: Option<f32>,
    orders_sig: u64,
    conn_sig: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    market.hash(&mut h);
    let (code, txt): (u8, &str) = match status {
        ConnStatus::Connecting => (0, ""),
        ConnStatus::Stage(s) => (1, s.as_str()),
        ConnStatus::Ready => (2, ""),
        ConnStatus::Failed(e) => (3, e.as_str()),
        ConnStatus::Disconnected => (4, ""),
    };
    code.hash(&mut h);
    txt.hash(&mut h);
    last_price
        .map(|p| (p * 100.0).round() as i64)
        .unwrap_or(i64::MIN)
        .hash(&mut h);
    orders_sig.hash(&mut h);
    conn_sig.hash(&mut h);
    h.finish()
}

/// Хэш сводки подключений (ready/total + список упавших) — чтобы статус-бар
/// перерисовывался при смене статуса ЛЮБОГО ядра, а не только активного.
fn conn_summary_sig(summary: &crate::session::ConnSummary) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    summary.ready.hash(&mut h);
    summary.total.hash(&mut h);
    for (name, st) in &summary.down {
        name.hash(&mut h);
        match st {
            ConnStatus::Connecting => 0u8.hash(&mut h),
            ConnStatus::Stage(s) => {
                1u8.hash(&mut h);
                s.hash(&mut h);
            }
            ConnStatus::Ready => 2u8.hash(&mut h),
            ConnStatus::Failed(e) => {
                3u8.hash(&mut h);
                e.hash(&mut h);
            }
            ConnStatus::Disconnected => 4u8.hash(&mut h),
        }
    }
    h.finish()
}
