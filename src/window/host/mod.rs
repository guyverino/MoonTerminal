//! WindowHost — одно ОС-окно одной группы: свой surface + egui + chart + dock.
//! App держит по WindowHost на группу. CoreStore общий (читается, не владеется).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window};

use crate::chart::container::{Container, ContainerKind, Mode, Pane, PaneSource};
use crate::chart::paint::MIN_FRAME_DT;
use crate::chart::view::Rect;
use crate::config::ChartTheme;
use crate::session::CoreId;
use crate::feed::OrderRow;
use crate::gpu::GpuContext;
use crate::session::{CoreStore, SessionManager};
use crate::shell::Shell;
use crate::workspace::Workspace;

mod input;
mod render;
mod signatures;

pub struct HostRender {
    pub gear_clicked: bool,
    pub strategies_clicked: bool,
    /// Вкладку дока потянули — открепить в окно (App создаёт окно для этого host'а).
    pub detach: Option<crate::dock::DockTab>,
    /// Нажата «вернуть в док» — App закроет окно открепления этой вкладки.
    pub repin: Option<crate::dock::DockTab>,
    /// Чарт-вкладку (контейнер №idx) потянули — открепить в отдельное чарт-окно.
    pub detach_chart: Option<usize>,
    /// Детекты для УЖЕ откреплённых чарт-окон: (целевой вид, ядро, рынок, ttl_ms).
    pub addto_detached: Vec<(ContainerKind, crate::session::CoreId, String, f64)>,
}

/// Принудительный прогон egui хотя бы раз в этот интервал — освежает живые
/// счётчики статус-бара (fps/present/CPU/RAM), которые исключены из хром-сигнатуры.
const EGUI_THROTTLE: Duration = Duration::from_millis(400);

/// Высота верхней полосы чарт-вкладок (Main/1/2/3) над областью графиков, точки.
const CHART_TABS_H: f32 = 28.0;

/// Чарт-вкладка (underline-стиль, как нижний док) с бейджем-счётчиком открытых
/// графиков. Активную подчёркивает [`render`] поверх бейзлайна.
fn chart_tab(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    selected: bool,
    draggable: bool,
) -> egui::Response {
    use crate::shell::theme;
    let f = if selected {
        theme::font_bold()
    } else {
        theme::font()
    };
    let pad = 12.0;
    let badge = count > 0;
    let badge_w = if badge { 20.0 } else { 0.0 };
    let w = theme::text_w(ui, label, &f) + pad * 2.0 + badge_w;
    let sense = if draggable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), sense);
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let round = egui::Rounding {
            nw: 5.0,
            ne: 5.0,
            sw: 0.0,
            se: 0.0,
        };
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, round, theme::LIFT);
        } else if hovered {
            p.rect_filled(rect, round, theme::LIFT.gamma_multiply(0.55));
        }
        let fg = if selected || hovered {
            theme::TEXT
        } else {
            theme::TEXT_2
        };
        p.text(
            egui::pos2(rect.min.x + pad, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            f,
            fg,
        );
        if badge {
            let c = egui::pos2(rect.max.x - 12.0, rect.center().y);
            p.circle_filled(c, 7.5, theme::ACCENT.gamma_multiply(0.9));
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                count.to_string(),
                egui::FontId::proportional(9.0),
                egui::Color32::from_rgb(0x14, 0x14, 0x16),
            );
        }
    }
    resp
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
    /// Per-core курсор уже учтённых детектов для AddToChart-ингеста.
    add_seq: HashMap<CoreId, u64>,
    /// Epoch для создания новых панелей (Chart::new) на лету.
    epoch_ms: f64,
    pub workspace: Workspace,
    /// Тема оформления чарта (приходит из App; смена → dirty-кадр).
    theme: ChartTheme,
    last_frame: Instant,
    /// Время последнего present — для капа частоты кадров (MIN_FRAME_DT).
    last_present_at: Instant,
    fps: f32,
    /// Метки реальных present() за последнюю секунду → честный present/s
    /// (диагностика нагрузки: idle после edge-driven фикса должен давать ~0).
    present_marks: VecDeque<Instant>,
    present_hz: f32,

    /// Ввод/интерактив чарта (общий с ChartWindow), всё в физ. пикселях.
    input: crate::chart::input::ChartInput,
    /// Прямоугольник зоны графика (физ. px) — hit-тест ввода. egui НЕ годится
    /// для гейта: CentralPanel поверх чарта считается «областью под курсором».
    chart_area: (f32, f32, f32, f32),
    /// Прямоугольник правого дока детектов (физ. px) — движение курсора над ним
    /// форсит перетесселяцию egui (живой spotlight на кнопках).
    detects_area: (f32, f32, f32, f32),

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
            add_seq: HashMap::new(),
            epoch_ms,
            workspace,
            theme: ChartTheme::default(),
            last_frame: Instant::now(),
            last_present_at: Instant::now() - MIN_FRAME_DT,
            fps: 0.0,
            present_marks: VecDeque::new(),
            present_hz: 0.0,
            input: crate::chart::input::ChartInput::default(),
            chart_area: (0.0, 0.0, 0.0, 0.0),
            detects_area: (0.0, 0.0, 0.0, 0.0),
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

    /// Втянуть свежие AddToChart-детекты ядер группы в чарт-вкладки по их НОМЕРУ
    /// (AddToChart=N → вкладка №N, создаётся лениво). TTL панели = KeepInChart.
    /// Такие детекты в ленту-кнопки не идут (см. DetectRibbon::ingest).
    /// `detached_keys` — виды чартов, уже откреплённых в окна: их детекты НЕ создают
    /// вкладку в host, а возвращаются для проброса в окно. `split_by_core` — настройка
    /// «отдельная вкладка на ядро» (ключ контейнера = (номер, ядро) или (номер, None)).
    /// Возвращает (изменился ли host, детекты для откреплённых окон).
    fn ingest_addtochart(
        &mut self,
        store: &CoreStore,
        now_ms: f64,
        detached_keys: &HashSet<ContainerKind>,
        split_by_core: bool,
    ) -> (bool, Vec<(ContainerKind, CoreId, String, f64)>) {
        // (целевой вид контейнера, ядро-источник, рынок, ttl_ms)
        let mut adds: Vec<(ContainerKind, CoreId, String, f64)> = Vec::new();
        for ci in &self.workspace.cores {
            let Some(d) = store.core(ci.id) else { continue };
            let last = self.add_seq.get(&ci.id).copied().unwrap_or(0);
            let mut newest = last;
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                newest = newest.max(det.seq);
                if det.add_to_chart > 0 {
                    let ttl = (det.keep_in_chart_secs.max(1) as f64) * 1000.0;
                    let kind = ContainerKind::Chart {
                        num: det.add_to_chart,
                        core: if split_by_core { Some(ci.id) } else { None },
                    };
                    adds.push((kind, ci.id, det.market.clone(), ttl));
                }
            }
            if newest != last {
                self.add_seq.insert(ci.id, newest);
            }
        }
        if adds.is_empty() {
            return (false, Vec::new());
        }
        let (fmt, epoch) = (self.gpu.format, self.epoch_ms);
        let mut forwarded: Vec<(ContainerKind, CoreId, String, f64)> = Vec::new();
        let mut host_changed = false;
        // Свежие детекты с конца (новые) → добавляем в обратном порядке (старые выше).
        for (kind, core, market, ttl) in adds.into_iter().rev() {
            // Чарт уже откреплён в окно → детект туда (App пробросит), не в host.
            if detached_keys.contains(&kind) {
                forwarded.push((kind, core, market, ttl));
                continue;
            }
            let idx = match self.containers.iter().position(|c| c.kind == kind) {
                Some(i) => i,
                None => {
                    self.containers.push(Container::new(kind));
                    self.containers.len() - 1
                }
            };
            self.containers[idx].push_auto(core, &market, now_ms, ttl, &self.gpu.device, fmt, epoch);
            host_changed = true;
        }
        if host_changed {
            // Порядок вкладок: Main, затем по (номер, ядро). active_container
            // восстанавливаем по виду (kind), т.к. индексы могли сдвинуться.
            let active_kind = self.containers[self.active_container].kind;
            self.containers.sort_by_key(|c| match c.kind {
                ContainerKind::Main => (0u8, 0u32, 0u64),
                ContainerKind::Chart { num, core } => (1, num, core.unwrap_or(0)),
            });
            self.active_container = self
                .containers
                .iter()
                .position(|c| c.kind == active_kind)
                .unwrap_or(0);
        }
        (host_changed, forwarded)
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
    /// времени каждой видимой панели. Меняется → нужен кадр. (В фулскрине
    /// невидимые панели всё равно не меняют картинку — приемлемо проходить по всем.)
    fn visible_sig(&self, session: &SessionManager, now_ms: f64) -> u64 {
        crate::chart::paint::panes_visible_sig(&self.active().panes, session, now_ms)
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
        if self.input.cursor.take().is_some() {
            self.window.set_cursor(CursorIcon::Default);
            self.dirty = true;
        }
    }

    /// Состояние Shift (для shift+колесо = пан по X).
    pub fn set_modifiers(&mut self, shift: bool) {
        self.input.shift_down = shift;
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

    /// Убрать все нумерованные чарт-вкладки (оставить Main). Зовётся при смене
    /// настройки charts_split_by_core — старые вкладки больше не получат детекты.
    pub fn clear_chart_tabs(&mut self) {
        self.containers
            .retain(|c| matches!(c.kind, ContainerKind::Main));
        self.active_container = 0;
        self.mark_egui_dirty();
    }

    /// Открыть монету на главной вкладке (Main) фулскрин и сделать Main активной.
    /// Зовётся App при двойном клике по чарту в откреплённом окне.
    pub fn open_on_main(&mut self, core: CoreId, market: &str, now_ms: f64) {
        let main_idx = self
            .containers
            .iter()
            .position(|c| c.kind == ContainerKind::Main)
            .unwrap_or(0);
        let (fmt, epoch) = (self.gpu.format, self.epoch_ms);
        self.containers[main_idx].open_manual(core, market, &self.gpu.device, fmt, epoch);
        self.active_container = main_idx;
        let c = &mut self.containers[main_idx];
        if let Mode::Fullscreen(i) = c.mode {
            if let Some(p) = c.panes.get_mut(i) {
                p.chart.view.resume_live(now_ms);
                p.chart.view.reset_y();
            }
        }
        self.mark_egui_dirty();
    }

    /// Открепить контейнер №`idx` (только нумерованную чарт-вкладку): забрать его
    /// спецификацию (для пересоздания в окне) и удалить из набора. Main не
    /// открепляется. Возвращает (вид, режим, спецификация панелей).
    pub fn take_container(
        &mut self,
        idx: usize,
    ) -> Option<(ContainerKind, Mode, Vec<(CoreId, String, PaneSource)>)> {
        let c = self.containers.get(idx)?;
        if !matches!(c.kind, ContainerKind::Chart { .. }) {
            return None; // Main не открепляем
        }
        let spec = c.spec();
        let (kind, mode) = (c.kind, c.mode);
        self.containers.remove(idx);
        // Поправить активный контейнер после удаления.
        if self.active_container >= self.containers.len() {
            self.active_container = 0;
        } else if self.active_container > idx {
            self.active_container -= 1;
        } else if self.active_container == idx {
            self.active_container = 0; // ушла активная → на Main
        }
        Some((kind, mode, spec))
    }
}

