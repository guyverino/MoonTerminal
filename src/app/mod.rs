//! App: менеджер ОС-окон. По окну на группу + окна-утилиты (Настройки/Стратегии)
//! + окна открепления вкладок дока (Ордера/Активы/Лог/Отчёт «вытянуты» в окно).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowId};

use crate::config::AppConfig;
use crate::db::{self, ReportsHandle};
use crate::dock::DockTab;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::session::SessionManager;
use crate::settings::SettingsState;
use crate::window::{
    handle_aux_event, EguiSurface, SettingsWindow, StrategiesWindow, WindowHost,
};
use crate::workspace::Workspace;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Окно открепления вкладки дока: отдельное ОС-окно с egui-сурфейсом, которое
/// рисует контент одной вкладки своего окна-владельца (`owner`). Состояние вкладки
/// (например фильтры отчёта) остаётся в доке владельца — окно лишь рисует его в
/// свой `ui`. Закрытие окна → вкладка возвращается в док (App снимает флаг).
struct DetachedPanel {
    owner: WindowId,
    tab: DockTab,
    window: Arc<Window>,
    egui: EguiSurface,
    /// Ревизия данных вкладки на прошлом кадре — для авто-перерисовки (живой
    /// отчёт/лог/ордера без необходимости двигать мышь).
    last_rev: u64,
}

pub struct App {
    config: AppConfig,
    settings: SettingsState,
    settings_window: Option<SettingsWindow>,
    open_settings_requested: bool,
    strategies_window: Option<StrategiesWindow>,
    open_strategies_requested: bool,
    /// Окна открепления вкладок (ключ — id окна открепления, не владельца).
    detached: HashMap<WindowId, DetachedPanel>,
    /// Очередь запросов на открепление/возврат вкладок (создание окна требует
    /// ActiveEventLoop, доступного только в about_to_wait/window_event).
    detach_reqs: Vec<(WindowId, DockTab)>,
    repin_reqs: Vec<(WindowId, DockTab)>,
    session: SessionManager,
    /// Хэндл БД отчётов: канал записи + счётчик-генерация (None = БД недоступна).
    reports: Option<ReportsHandle>,
    epoch_ms: f64,
    windows: HashMap<WindowId, WindowHost>,
    needs_rebuild: bool,
    metrics: Metrics,
    /// Последний снимок статусов ядер, показанный в окне настроек. Сравниваем,
    /// чтобы перерисовывать настройки только при реальной смене статуса.
    settings_statuses: crate::settings::CoreStatuses,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let epoch_ms = now_ms();
        let reports = db::spawn_writer();
        let mut session = SessionManager::start(&config, epoch_ms, reports.as_ref().map(|h| &h.tx));
        session.set_market_mode(config.market_mode);
        Self {
            config,
            settings: SettingsState::new(),
            settings_window: None,
            open_settings_requested: false,
            strategies_window: None,
            open_strategies_requested: false,
            detached: HashMap::new(),
            detach_reqs: Vec::new(),
            repin_reqs: Vec::new(),
            session,
            reports,
            epoch_ms,
            windows: HashMap::new(),
            needs_rebuild: false,
            metrics: Metrics::new(),
            settings_statuses: HashMap::new(),
        }
    }

    /// (Пере)создаёт окна групп. Нет групп — одно пустое окно (для Настроек).
    fn build_windows(&mut self, event_loop: &ActiveEventLoop) {
        self.windows.clear();
        // Окна открепления — дети окон групп; при пересоздании окон закрываем их.
        self.detached.clear();
        // Счётчик-генерация writer'а отчётов — во вкладку «Отчёт» дока каждого окна.
        let gen = self.reports.as_ref().map(|h| h.generation.clone());
        let mut workspaces = Workspace::build_all(&self.config, gen.clone());
        if workspaces.is_empty() {
            // Первый запуск (ни одного сервера с ключом) — групповых окон не делаем
            // вовсе: resumed() откроет только окно Настроек. Если же серверы есть, но
            // все headless/неактивны — пустое окно-заглушка для доступа к Настройкам.
            if !self.config.has_keyed_server() {
                return;
            }
            workspaces.push(Workspace::placeholder(gen));
        }
        for ws in workspaces {
            match WindowHost::new(event_loop, ws, self.epoch_ms) {
                Ok(host) => {
                    self.windows.insert(host.window.id(), host);
                }
                Err(e) => log::error!("создание окна: {e:#}"),
            }
        }
    }

    fn render_window(&mut self, id: WindowId, metrics: MetricsSnapshot) {
        let now = now_ms();
        let mut gear = false;
        let mut strategies = false;
        let mut detach = None;
        let mut repin = None;
        {
            let session = &self.session;
            if let Some(host) = self.windows.get_mut(&id) {
                if !host.needs_render(session, now) {
                    return;
                }
                let out = host.render(session, now, metrics);
                gear = out.gear_clicked;
                strategies = out.strategies_clicked;
                detach = out.detach;
                repin = out.repin;
            }
        }
        if gear {
            self.open_settings_requested = true;
        }
        if strategies {
            self.open_strategies_requested = true;
        }
        if let Some(tab) = detach {
            self.detach_reqs.push((id, tab));
        }
        if let Some(tab) = repin {
            self.repin_reqs.push((id, tab));
        }
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop) {
        self.open_settings_requested = false;
        if let Some(sw) = &self.settings_window {
            sw.window.focus_window();
            return;
        }
        self.settings.begin(&self.config);
        match SettingsWindow::new(event_loop) {
            Ok(w) => {
                self.settings_window = Some(w);
            }
            Err(e) => log::error!("окно настроек: {e:#}"),
        }
    }

    fn open_strategies(&mut self, event_loop: &ActiveEventLoop) {
        self.open_strategies_requested = false;
        if let Some(sw) = &self.strategies_window {
            sw.window.focus_window();
            return;
        }
        match StrategiesWindow::new(event_loop) {
            Ok(w) => self.strategies_window = Some(w),
            Err(e) => log::error!("окно стратегий: {e:#}"),
        }
    }

    fn render_strategies(&mut self) {
        // Новые снимки стратегий/схемы → перерисовка (poll), затем кадр. Действия
        // (синхронизация галок + старт/стоп отмеченных) шлём через единый диспетчер.
        let mut actions: Vec<crate::strategies::StratAction> = Vec::new();
        if let Some(sw) = self.strategies_window.as_mut() {
            sw.poll(&self.session);
            if sw.needs_render() {
                actions = sw.render(&self.session).actions;
            }
        }
        for a in actions {
            self.session.apply_strategies(a.core, a.checks, a.start_stop);
        }
    }

    /// Открепить вкладку `tab` окна-владельца `owner` в отдельное окно. Уже
    /// откреплена → просто фокусируем существующее окно.
    fn open_detached(&mut self, event_loop: &ActiveEventLoop, owner: WindowId, tab: DockTab) {
        if let Some(p) = self.detached.values().find(|p| p.owner == owner && p.tab == tab) {
            p.window.focus_window();
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("{} — MoonTerminal", tab.title()))
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1100.0, 520.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("окно открепления: {e:#}");
                return;
            }
        };
        window.set_window_icon(crate::icons::brand_winit_icon());
        let egui = match EguiSurface::new(&window) {
            Ok(e) => e,
            Err(e) => {
                log::error!("egui окна открепления: {e:#}");
                return;
            }
        };
        let det_id = window.id();
        if let Some(host) = self.windows.get_mut(&owner) {
            host.workspace.dock.set_detached(tab, true);
            host.mark_egui_dirty(); // в доке вместо контента появится плашка
        }
        self.detached.insert(
            det_id,
            DetachedPanel { owner, tab, window, egui, last_rev: u64::MAX },
        );
    }

    /// Закрыть окно открепления по его id и вернуть вкладку в док владельца.
    fn close_detached(&mut self, det_id: WindowId) {
        if let Some(p) = self.detached.remove(&det_id) {
            if let Some(host) = self.windows.get_mut(&p.owner) {
                host.workspace.dock.set_detached(p.tab, false);
                host.mark_egui_dirty();
            }
        }
    }

    /// Вернуть вкладку в док (по кнопке «вернуть» на плашке) — закрывает окно.
    fn repin(&mut self, owner: WindowId, tab: DockTab) {
        let id = self
            .detached
            .iter()
            .find(|(_, p)| p.owner == owner && p.tab == tab)
            .map(|(id, _)| *id);
        if let Some(id) = id {
            self.close_detached(id);
        }
    }

    /// Рисует окна открепления. Контент берётся из дока владельца (тот же
    /// `content_ui`, что и inline) — единый источник состояния, без дубля.
    fn render_detached(&mut self) {
        let ids: Vec<WindowId> = self.detached.keys().copied().collect();
        for det_id in ids {
            let Some(panel) = self.detached.get_mut(&det_id) else {
                continue;
            };
            let owner = panel.owner;
            let tab = panel.tab;
            let Some(host) = self.windows.get_mut(&owner) else {
                continue; // владелец закрыт — окно закроется по своему CloseRequested
            };

            // Живость: перерисовать, если данные вкладки изменились с прошлого кадра.
            let store = self.session.store();
            let rev = match tab {
                DockTab::Report => host.workspace.dock.report.generation(),
                DockTab::Log => crate::applog::revision(),
                DockTab::Orders => host.orders_rev(store),
                DockTab::Assets => 0,
            };
            if rev != panel.last_rev {
                panel.egui.mark_dirty();
                panel.last_rev = rev;
            }
            if !panel.egui.needs_render() {
                continue;
            }

            let orders = if tab == DockTab::Orders {
                host.collect_orders(store)
            } else {
                Vec::new()
            };
            let report = host.workspace.dock.report_mut();
            panel.egui.render(&panel.window, "detached-pass", |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::dock::tabs::content_ui(ui, tab, report, &orders);
                });
            });
        }
    }

    fn render_settings(&mut self) {
        if self.settings_window.is_none() {
            return;
        }
        // Снимок статусов из живой сессии. Если он изменился с прошлого кадра —
        // форсируем перерисовку настроек, чтобы кружки статуса обновлялись вживую.
        let statuses = self.session.status_map();
        if statuses != self.settings_statuses {
            self.settings_statuses = statuses.clone();
            if let Some(sw) = self.settings_window.as_mut() {
                sw.mark_dirty();
            }
        }
        // Рисуем окно настроек только если оно открыто и просит кадр.
        let render = self
            .settings_window
            .as_ref()
            .is_some_and(|sw| sw.needs_render());
        if !render {
            return;
        }
        // Снимок до сохранения — чтобы понять, ЧТО изменилось (тема/язык/структура).
        let before = self.config.clone();
        let mut saved = false;
        if let Some(sw) = self.settings_window.as_mut() {
            saved = sw.render(&mut self.settings, &mut self.config, &statuses).saved;
        }
        // Действия вкладок (ручной реконнект ядра по кнопке) — применяем независимо
        // от сохранения: реконнект работает по живому (сохранённому) конфигу.
        for id in self.settings.take_actions().reconnect {
            self.session
                .reconnect(id, &self.config, self.reports.as_ref().map(|h| &h.tx));
        }
        if !saved {
            return;
        }

        // Реконнект к ядрам + пересоздание окон — ТОЛЬКО при смене серверов/групп
        // (число/состав окон зависит от них). Тема применяется живо (ничего не
        // нужно). Смена языка — без реконнекта: только перегон egui-хрома.
        let struct_changed = before.structural_sig() != self.config.structural_sig();
        let lang_changed = before.language != self.config.language;
        let mode_changed = before.market_mode != self.config.market_mode;
        if struct_changed {
            rust_i18n::set_locale(self.config.language.code());
            self.session = SessionManager::start(
                &self.config,
                self.epoch_ms,
                self.reports.as_ref().map(|h| &h.tx),
            );
            self.session.set_market_mode(self.config.market_mode);
            self.needs_rebuild = true;
        } else if mode_changed {
            // Режим рынка меняем живо: ядра остаются подключёнными, координатор лишь
            // пере-выберет провайдеров и перешлёт роли на следующем тике.
            self.session.set_market_mode(self.config.market_mode);
        } else if lang_changed {
            // Язык не трогает подключения/окна — только тексты egui. Глобальная
            // локаль + форс перетесселяции хрома во всех окнах.
            rust_i18n::set_locale(self.config.language.code());
            for host in self.windows.values_mut() {
                host.mark_egui_dirty();
            }
        }
        // Иначе (изменилась только тема) — ничего: уже применено живо.
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.windows.is_empty() {
            self.build_windows(event_loop);
            // Окон групп нет (первый запуск без серверов) — открываем только Настройки.
            if self.windows.is_empty() {
                self.open_settings(event_loop);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // Окна-утилиты (Настройки/Стратегии) обрабатывают ввод/ресайз одинаково
        // (handle_aux_event); различается только реакция на закрытие.
        if let Some(w) = self.settings_window.as_mut().filter(|w| w.window.id() == id) {
            if handle_aux_event(w, &event) {
                self.settings_window = None;
                // Настройки были единственным окном (первый запуск без серверов) —
                // закрыли, открывать больше нечего → выходим.
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }
            return;
        }
        if let Some(w) = self.strategies_window.as_mut().filter(|w| w.window.id() == id) {
            if handle_aux_event(w, &event) {
                self.strategies_window = None;
            }
            return;
        }

        // Окно открепления вкладки: ввод/ресайз в его egui; закрытие → возврат в док.
        if self.detached.contains_key(&id) {
            let mut close = false;
            if let Some(panel) = self.detached.get_mut(&id) {
                panel.egui.on_event(&panel.window, &event);
                if let WindowEvent::Resized(size) = &event {
                    panel.egui.resize(*size);
                }
                close = matches!(event, WindowEvent::CloseRequested);
            }
            if close {
                self.close_detached(id);
            }
            return;
        }

        // Окно группы.
        {
            let Some(host) = self.windows.get_mut(&id) else {
                return;
            };
            // egui всё равно получает событие; гейт ввода чарта — собственный
            // hit-тест в host (CentralPanel поверх чарта ломает egui-«consumed»).
            let consumed = host.on_egui_event(&event);
            // Перерисовку и перегон egui форсируем точечно. CursorMoved над
            // графиком/драг решает pointer_moved (только кадр, хром reuse). Клик,
            // колесо, ресайз, клавиши и таскание egui-виджета (consumed) →
            // mark_egui_dirty (кадр + перегон хрома). Голый mark на КАЖДОЕ
            // событие убран — он гнал «шторм» кадров и тесселяций от мыши.
            match &event {
                WindowEvent::Resized(size) => {
                    host.resize(*size);
                    host.mark_egui_dirty();
                }
                WindowEvent::ModifiersChanged(m) => {
                    host.set_modifiers(m.state().shift_key());
                }
                WindowEvent::CursorMoved { position, .. } => {
                    host.pointer_moved(position.x as f32, position.y as f32);
                    if consumed {
                        host.mark_egui_dirty(); // тащим egui-виджет → хром меняется
                    }
                }
                WindowEvent::CursorLeft { .. } => host.clear_cursor(),
                WindowEvent::MouseInput { state, button, .. } => {
                    let pressed = *state == winit::event::ElementState::Pressed;
                    host.mouse_button(*button, pressed);
                    host.mark_egui_dirty();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    host.wheel(delta);
                    host.mark_egui_dirty();
                }
                _ => host.mark_egui_dirty(),
            }
        }

        if let WindowEvent::CloseRequested = event {
            self.windows.remove(&id);
            // Закрылось окно группы — закрываем и его окна открепления (дети).
            self.detached.retain(|_, p| p.owner != id);
            if self.windows.is_empty() {
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.needs_rebuild {
            self.needs_rebuild = false;
            self.build_windows(event_loop);
        }

        self.session.drain();

        // Открытые чарты по окнам: ядро → рынок (динамическая подписка). Окно без
        // открытого чарта в карту не попадает → ничего не подписываем.
        let open: HashMap<crate::session::CoreId, String> = self
            .windows
            .values()
            .filter_map(|h| {
                h.workspace
                    .open
                    .as_ref()
                    .map(|o| (o.core, o.market.clone()))
            })
            .collect();
        self.session.set_open(&open);

        // Тема для чарта: пока открыто окно настроек — берём редактируемый draft
        // (живое превью), иначе сохранённую из конфига. Смена → host пометит dirty.
        let theme = if self.settings_window.is_some() {
            self.settings.draft_theme().clone()
        } else {
            self.config.theme.clone()
        };
        for host in self.windows.values_mut() {
            host.set_theme(&theme);
        }

        // Один снимок метрик на тик (sysinfo сам троттлит до ~1 Гц), общий для
        // всех окон: CPU/RAM — общепроцессные, present/fps — пер-окно.
        let metrics = self.metrics.sample(Instant::now());
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.render_window(id, metrics);
        }

        // Открепление/возврат вкладок (накоплены в render_window — здесь есть
        // event_loop для создания окон).
        for (owner, tab) in std::mem::take(&mut self.detach_reqs) {
            self.open_detached(event_loop, owner, tab);
        }
        for (owner, tab) in std::mem::take(&mut self.repin_reqs) {
            self.repin(owner, tab);
        }
        self.render_detached();

        if self.open_settings_requested {
            self.open_settings(event_loop);
        }
        self.render_settings();

        if self.open_strategies_requested {
            self.open_strategies(event_loop);
        }
        self.render_strategies();

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(8),
        ));
    }
}
