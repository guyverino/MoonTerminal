//! App: менеджер ОС-окон. По окну на группу + отдельное окно Настроек.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use crate::config::AppConfig;
use crate::db::{self, ReportsHandle};
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::session::SessionManager;
use crate::settings::SettingsState;
use crate::window::{ReportsWindow, SettingsWindow, WindowHost};
use crate::workspace::Workspace;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

pub struct App {
    config: AppConfig,
    settings: SettingsState,
    settings_window: Option<SettingsWindow>,
    open_settings_requested: bool,
    reports_window: Option<ReportsWindow>,
    open_reports_requested: bool,
    session: SessionManager,
    /// Хэндл БД отчётов: канал записи + счётчик-генерация (None = БД недоступна).
    reports: Option<ReportsHandle>,
    epoch_ms: f64,
    windows: HashMap<WindowId, WindowHost>,
    needs_rebuild: bool,
    metrics: Metrics,
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
            reports_window: None,
            open_reports_requested: false,
            session,
            reports,
            epoch_ms,
            windows: HashMap::new(),
            needs_rebuild: false,
            metrics: Metrics::new(),
        }
    }

    /// (Пере)создаёт окна групп. Нет групп — одно пустое окно (для Настроек).
    fn build_windows(&mut self, event_loop: &ActiveEventLoop) {
        self.windows.clear();
        let mut workspaces = Workspace::build_all(&self.config);
        if workspaces.is_empty() {
            workspaces.push(Workspace::placeholder());
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
        let mut reports = false;
        {
            let session = &self.session;
            if let Some(host) = self.windows.get_mut(&id) {
                if !host.needs_render(session, now) {
                    return;
                }
                let out = host.render(session, now, metrics);
                gear = out.gear_clicked;
                reports = out.reports_clicked;
            }
        }
        if gear {
            self.open_settings_requested = true;
        }
        if reports {
            self.open_reports_requested = true;
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

    fn open_reports(&mut self, event_loop: &ActiveEventLoop) {
        self.open_reports_requested = false;
        if let Some(rw) = &self.reports_window {
            rw.window.focus_window();
            return;
        }
        let generation = self.reports.as_ref().map(|h| h.generation.clone());
        match ReportsWindow::new(event_loop, generation) {
            Ok(w) => self.reports_window = Some(w),
            Err(e) => log::error!("окно отчётов: {e:#}"),
        }
    }

    fn render_reports(&mut self) {
        if let Some(rw) = self.reports_window.as_mut() {
            // Новые/изменённые отчёты от writer → перезапрос даже если окно открыто.
            rw.poll();
            if rw.needs_render() {
                rw.render();
            }
        }
    }

    fn render_settings(&mut self) {
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
            saved = sw.render(&mut self.settings, &mut self.config).saved;
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
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // Окно настроек?
        let is_settings = self
            .settings_window
            .as_ref()
            .is_some_and(|sw| sw.window.id() == id);
        if is_settings {
            let mut close = false;
            if let Some(sw) = self.settings_window.as_mut() {
                sw.on_egui_event(&event);
                if let WindowEvent::Resized(size) = &event {
                    sw.resize(*size);
                }
                if matches!(event, WindowEvent::CloseRequested) {
                    close = true;
                }
            }
            if close {
                self.settings_window = None;
            }
            return;
        }

        // Окно отчётов?
        let is_reports = self
            .reports_window
            .as_ref()
            .is_some_and(|rw| rw.window.id() == id);
        if is_reports {
            let mut close = false;
            if let Some(rw) = self.reports_window.as_mut() {
                rw.on_egui_event(&event);
                if let WindowEvent::Resized(size) = &event {
                    rw.resize(*size);
                }
                if matches!(event, WindowEvent::CloseRequested) {
                    close = true;
                }
            }
            if close {
                self.reports_window = None;
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

        if self.open_settings_requested {
            self.open_settings(event_loop);
        }
        self.render_settings();

        if self.open_reports_requested {
            self.open_reports(event_loop);
        }
        self.render_reports();

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(8),
        ));
    }
}
