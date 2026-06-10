//! App: менеджер ОС-окон. По окну на группу + окна-утилиты (Настройки/Стратегии)
//! + окна открепления вкладок дока (Ордера/Активы/Лог/Отчёт «вытянуты» в окно).

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use crate::config::AppConfig;
use crate::db::{self, ReportsHandle};
use crate::dock::DockTab;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::session::SessionManager;
use crate::settings::SettingsState;
use crate::window::{
    handle_aux_event, ChartWindow, SettingsWindow, StrategiesWindow, WindowHost,
};
use crate::workspace::Workspace;

mod detached;
mod layout;

use detached::DetachedPanel;

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
    strategies_window: Option<StrategiesWindow>,
    open_strategies_requested: bool,
    /// Окна открепления вкладок дока (ключ — id окна открепления, не владельца).
    detached: HashMap<WindowId, DetachedPanel>,
    /// Откреплённые чарт-окна (контейнеры графиков, вынесенные drag'ом вкладки).
    detached_charts: HashMap<WindowId, ChartWindow>,
    /// Очередь запросов на откреп чарт-вкладки: (окно-владелец, индекс контейнера).
    detach_chart_reqs: Vec<(WindowId, usize)>,
    /// Группы, окно которых попросили показать (кнопка «глаз» в Настройках).
    show_group_reqs: Vec<String>,
    /// Очередь запросов на открепление/возврат вкладок (создание окна требует
    /// ActiveEventLoop, доступного только в about_to_wait/window_event).
    detach_reqs: Vec<(WindowId, DockTab)>,
    repin_reqs: Vec<(WindowId, DockTab)>,
    /// ОБЩИЙ для всех окон групп `ReportView` (один экземпляр, одно SQLite-чтение).
    report: crate::dock::ReportView,
    /// Состояние лог-панели ОТКРЕПЛЁННОГО окна лога (одно общее окно на все группы;
    /// область видимости — все ядра). Состояние докового лога — своё у каждого окна
    /// (в его `Dock`). Сбрасывается к дефолту при закрытии окна лога.
    detached_log: crate::dock::LogPanelState,
    /// Глобальные флаги открепления Report/Log/Assets (по [`DockTab::idx`]; Orders
    /// игнорируется — его открепление пер-окно живёт в доке окна).
    global_detached: [bool; 4],
    /// Ревизии общих живых вкладок на прошлом тике — для форса кадров окон, где
    /// активна Лог/Отчёт и вкладка не откреплена.
    last_log_rev: u64,
    last_report_gen: u64,
    /// Раскладка окон (позиции/свёрнутость/вкладки + откреплённые) — layout.toml.
    layout: crate::config::WindowLayout,
    layout_dirty: bool,
    last_layout_save: Instant,
    /// Когда последний раз чистили старые файлы лога (раз в сутки).
    last_purge: Instant,
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
        let report = crate::dock::ReportView::new(reports.as_ref().map(|h| h.generation.clone()));
        Self {
            config,
            settings: SettingsState::new(),
            settings_window: None,
            open_settings_requested: false,
            strategies_window: None,
            open_strategies_requested: false,
            detached: HashMap::new(),
            detached_charts: HashMap::new(),
            detach_chart_reqs: Vec::new(),
            show_group_reqs: Vec::new(),
            detach_reqs: Vec::new(),
            repin_reqs: Vec::new(),
            report,
            detached_log: crate::dock::LogPanelState::default(),
            global_detached: [false; 4],
            last_log_rev: 0,
            last_report_gen: 0,
            layout: crate::config::WindowLayout::load(),
            layout_dirty: false,
            last_layout_save: Instant::now(),
            last_purge: Instant::now(),
            session,
            reports,
            epoch_ms,
            windows: HashMap::new(),
            needs_rebuild: false,
            metrics: Metrics::new(),
            settings_statuses: HashMap::new(),
        }
    }

    /// Список источников лога для селектора. Порядок: агрегат (по умолчанию) →
    /// «Локальный» → ядра. `scope`=Some(группа) — только ядра этой группы (для дока
    /// окна группы; агрегат = «Лог группы»); None — все ядра (для откреплённого окна;
    /// агрегат = «Все ядра»). `id` сервера = рантайм-CoreId (ключ store).
    fn build_log_sources(&self, scope: Option<&str>) -> Vec<crate::dock::LogSourceItem> {
        use crate::dock::{LogSource, LogSourceItem};
        let agg_label = match scope {
            Some(_) => t!("log.source.group"),
            None => t!("log.source.all"),
        };
        let mut v = vec![
            LogSourceItem {
                source: LogSource::Aggregate,
                display: agg_label.to_string(),
                file_label: String::new(),
            },
            LogSourceItem {
                source: LogSource::Local,
                display: t!("log.source.local").to_string(),
                file_label: "app".to_string(),
            },
        ];
        for s in &self.config.servers {
            if scope.is_some_and(|g| g != s.group) {
                continue; // в доке окна — только ядра его группы
            }
            v.push(LogSourceItem {
                source: LogSource::Core(s.id),
                display: s.name.clone(),
                file_label: crate::applog::sanitize_label(&s.name),
            });
        }
        v
    }

    /// (Пере)создаёт окна групп. Нет групп — одно пустое окно (для Настроек).
    fn build_windows(&mut self, event_loop: &ActiveEventLoop) {
        self.windows.clear();
        // Окна открепления — дети окон групп; при пересоздании окон закрываем их и
        // сбрасываем глобальные флаги открепления.
        self.detached.clear();
        self.detached_charts.clear();
        self.global_detached = [false; 4];
        let mut workspaces = Workspace::build_all(&self.config);
        if workspaces.is_empty() {
            // Первый запуск (ни одного сервера с ключом) — групповых окон не делаем
            // вовсе: resumed() откроет только окно Настроек. Если же серверы есть, но
            // все headless/неактивны — пустое окно-заглушка для доступа к Настройкам.
            if !self.config.has_keyed_server() {
                return;
            }
            workspaces.push(Workspace::placeholder());
        }
        for ws in workspaces {
            let gl = self.layout.groups.get(&ws.group).copied();
            match WindowHost::new(event_loop, ws, self.epoch_ms, gl) {
                Ok(host) => {
                    self.windows.insert(host.window.id(), host);
                }
                Err(e) => log::error!("создание окна: {e:#}"),
            }
        }

        // Восстановить откреплённые окна из раскладки (если их группа-владелец есть).
        for d in self.layout.detached.clone() {
            let owner = self
                .windows
                .iter()
                .find(|(_, h)| h.workspace.group == d.owner_group)
                .map(|(id, _)| *id);
            if let Some(owner) = owner {
                let tab = DockTab::from_idx(d.tab as usize);
                self.open_detached(event_loop, owner, tab, Some((d.x, d.y, d.w, d.h)));
            }
        }
    }

    /// Показать окно группы по кнопке «глаз»: если открыто — сфокусировать; если
    /// закрыто — создать заново (с сохранённой раскладкой).
    fn show_group(&mut self, event_loop: &ActiveEventLoop, name: &str) {
        if let Some(h) = self.windows.values().find(|h| h.workspace.group == name) {
            h.window.focus_window();
            return;
        }
        let gl = self.layout.groups.get(name).copied();
        let ws = Workspace::build_all(&self.config)
            .into_iter()
            .find(|w| w.group == name);
        if let Some(ws) = ws {
            match WindowHost::new(event_loop, ws, self.epoch_ms, gl) {
                Ok(host) => {
                    self.windows.insert(host.window.id(), host);
                }
                Err(e) => log::error!("показать группу «{name}»: {e:#}"),
            }
        }
    }

    fn render_window(&mut self, id: WindowId, metrics: MetricsSnapshot) {
        let now = now_ms();
        let mut gear = false;
        let mut strategies = false;
        let mut detach = None;
        let mut repin = None;
        // Виды чартов, откреплённых из ЭТОГО окна, — их новые детекты пойдут в их
        // окна, а не во вкладку host'а.
        let detached_keys: std::collections::HashSet<crate::chart::container::ContainerKind> = self
            .detached_charts
            .values()
            .filter(|w| w.owner() == id)
            .map(|w| w.chart_kind())
            .collect();
        let split_by_core = self.config.charts_split_by_core;
        // Источники лога этого окна — только ядра его группы (агрегат = «Лог группы»).
        let group = self.windows.get(&id).map(|h| h.workspace.group.clone());
        let log_sources = self.build_log_sources(group.as_deref());
        let mut addto: Vec<(
            crate::chart::container::ContainerKind,
            crate::session::CoreId,
            String,
            f64,
        )> = Vec::new();
        {
            let session = &self.session;
            let report = &mut self.report;
            let global_detached = self.global_detached;
            if let Some(host) = self.windows.get_mut(&id) {
                if !host.needs_render(session, now) {
                    return;
                }
                let out = host.render(
                    session,
                    now,
                    metrics,
                    report,
                    &log_sources,
                    global_detached,
                    &detached_keys,
                    split_by_core,
                );
                gear = out.gear_clicked;
                strategies = out.strategies_clicked;
                detach = out.detach;
                repin = out.repin;
                if let Some(ci) = out.detach_chart {
                    self.detach_chart_reqs.push((id, ci));
                }
                addto = out.addto_detached;
            }
        }
        // Пробросить детекты откреплённых чартов в их окна (по виду контейнера).
        for (kind, core, market, ttl) in addto {
            if let Some(w) = self
                .detached_charts
                .values_mut()
                .find(|w| w.owner() == id && w.chart_kind() == kind)
            {
                w.push_auto(core, &market, now, ttl);
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
        // Действия вкладок — применяем независимо от сохранения. Реконнект работает
        // по живому конфигу; «показать группу» откладываем (нужен event_loop).
        let acts = self.settings.take_actions();
        for id in acts.reconnect {
            self.session
                .reconnect(id, &self.config, self.reports.as_ref().map(|h| &h.tx));
        }
        self.show_group_reqs.extend(acts.show_group);
        if !saved {
            return;
        }

        // Настройки файлового лога применяем живо (без реконнекта). Если включили
        // запись или сократили срок — сразу подчищаем старые файлы.
        if before.log_to_file != self.config.log_to_file
            || before.log_retention_days != self.config.log_retention_days
        {
            crate::applog::set_file_logging(self.config.log_to_file, self.config.log_retention_days);
            crate::applog::purge_old();
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
        // Сменили тип группировки чартов по ядрам → старые чарт-вкладки/окна больше
        // не получат детекты (ключ контейнера изменился) → чистим, новые соберутся
        // заново в новом режиме. Независимо от прочих изменений (если не было
        // структурного ребилда, который и так всё пересоздаёт).
        if !struct_changed && before.charts_split_by_core != self.config.charts_split_by_core {
            for host in self.windows.values_mut() {
                host.clear_chart_tabs();
            }
            self.detached_charts.clear();
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
                // Выходим, если больше открывать нечего (нет окон групп и стратегий).
                if self.windows.is_empty() && self.strategies_window.is_none() {
                    event_loop.exit();
                }
            }
            return;
        }
        if let Some(w) = self.strategies_window.as_mut().filter(|w| w.window.id() == id) {
            if handle_aux_event(w, &event) {
                self.strategies_window = None;
                if self.windows.is_empty() && self.settings_window.is_none() {
                    event_loop.exit();
                }
            }
            return;
        }

        // Откреплённое чарт-окно: ввод/ресайз/закрытие обрабатывает оно само.
        if self.detached_charts.contains_key(&id) {
            let mut close = false;
            if let Some(w) = self.detached_charts.get_mut(&id) {
                close = w.on_event(&event);
            }
            if close {
                self.detached_charts.remove(&id);
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
            if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                self.layout_dirty = true;
            }
            if close {
                self.close_detached(id);
                self.update_detached_layout();
                self.layout.save();
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

        // Двигали/ресайзили/кликали окно группы → раскладка могла измениться
        // (позиция/размер, а клик мог свернуть док / сменить вкладку).
        if matches!(
            event,
            WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::MouseInput { .. }
        ) {
            self.layout_dirty = true;
        }

        if let WindowEvent::CloseRequested = event {
            // Снять геометрию закрываемого окна группы в раскладку ДО удаления.
            self.update_group_layout(id);
            self.windows.remove(&id);
            // Закрылось окно группы — закрываем и его детей: окна открепления вкладок
            // дока И откреплённые чарт-окна (иначе висят без владельца).
            self.detached.retain(|_, p| p.owner != id);
            self.detached_charts.retain(|_, w| w.owner() != id);
            self.update_detached_layout();
            self.layout.save();
            // Выходим, только когда не осталось НИ окон групп, НИ Настроек/Стратегий
            // — иначе из открытых Настроек можно вернуть закрытую группу «глазом».
            if self.windows.is_empty()
                && self.settings_window.is_none()
                && self.strategies_window.is_none()
            {
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

        // Открытые рынки всех панелей всех контейнеров всех окон → подписки
        // (список пар; ядро может иметь несколько открытых рынков — мульти-панель).
        // ВКЛЮЧАЯ откреплённые чарт-окна — иначе их рынок отпишется и тики пропадут.
        let open: Vec<(crate::session::CoreId, String)> = self
            .windows
            .values()
            .flat_map(|h| h.open_markets())
            .chain(self.detached_charts.values().flat_map(|w| w.open_markets()))
            .collect();
        self.session.set_open(&open);

        // Тема для чарта: пока открыто окно настроек — берём редактируемый draft
        // (живое превью), иначе сохранённую из конфига. Смена → host пометит dirty.
        let theme = if self.settings_window.is_some() {
            self.settings.draft_theme().clone()
        } else {
            self.config.theme.clone()
        };
        // Стиль линий ордеров (orders.toml) — без живого превью, из конфига.
        let orders_style = self.config.orders.clone();
        for host in self.windows.values_mut() {
            host.set_theme(&theme);
            host.set_orders_style(&orders_style);
        }

        // Живые ОБЩИЕ вкладки (Лог/Отчёт): при изменении данных форсим кадр окнам,
        // где такая вкладка активна и НЕ откреплена (её состояние/флаг — глобальны,
        // поэтому решает App, а не per-window needs_render).
        // Активность лога = локальный лог + лог всех ядер (доковый лог может смотреть
        // на ядро/группу, не только на локальный).
        let log_rev = crate::applog::revision().wrapping_add(self.session.store().log_activity());
        let report_gen = self.report.generation();
        let log_changed = log_rev != self.last_log_rev;
        let report_changed = report_gen != self.last_report_gen;
        self.last_log_rev = log_rev;
        self.last_report_gen = report_gen;
        if log_changed || report_changed {
            let gd = self.global_detached;
            for host in self.windows.values_mut() {
                let tab = host.workspace.dock.tab;
                let live = (tab == DockTab::Log && log_changed && !gd[DockTab::Log.idx()])
                    || (tab == DockTab::Report && report_changed && !gd[DockTab::Report.idx()]);
                if live {
                    host.mark_egui_dirty();
                }
            }
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
        let had_detach_ops = !self.detach_reqs.is_empty() || !self.repin_reqs.is_empty();
        for (owner, tab) in std::mem::take(&mut self.detach_reqs) {
            self.open_detached(event_loop, owner, tab, None);
        }
        for (owner, tab) in std::mem::take(&mut self.repin_reqs) {
            self.repin(owner, tab);
        }
        if had_detach_ops {
            self.layout_dirty = true; // состав откреплённых окон изменился
        }
        // Откреп чарт-вкладок в окна (накоплено в render_window — здесь есть event_loop).
        for (owner, idx) in std::mem::take(&mut self.detach_chart_reqs) {
            self.open_detached_chart(event_loop, owner, idx);
        }
        self.render_detached();

        // Рендер откреплённых чарт-окон (свой surface; данные — из общей сессии).
        let now_chart = now_ms();
        // Двойной клик в чарт-окне → открыть монету на Main окна-владельца.
        let mut to_main: Vec<(WindowId, crate::session::CoreId, String)> = Vec::new();
        // Сбор окон на закрытие (по крестику тулбара).
        let mut close_charts: Vec<WindowId> = Vec::new();
        for (cid, w) in self.detached_charts.iter_mut() {
            w.set_theme(&theme);
            w.set_orders_style(&orders_style);
            if let Some((core, market)) = w.take_pending_to_main() {
                to_main.push((w.owner(), core, market));
            }
            if w.take_close_requested() {
                close_charts.push(*cid);
            }
            if w.needs_render(&self.session, now_chart) {
                w.render(&self.session, now_chart);
            }
        }
        for cid in close_charts {
            self.detached_charts.remove(&cid);
        }
        for (owner, core, market) in to_main {
            if let Some(h) = self.windows.get_mut(&owner) {
                h.open_on_main(core, &market, now_chart);
                h.window.focus_window();
            }
        }

        // Раскладку пишем дебаунсом: накопили изменения (двигали/ресайзили окна,
        // свернули док, открепили) → раз в ~1.2 с снимаем состояние живых окон.
        if self.layout_dirty && self.last_layout_save.elapsed() > Duration::from_millis(1200) {
            self.save_layout();
        }

        // Чистка старых файлов лога раз в сутки (плюс одноразово при старте/сохранении).
        if self.last_purge.elapsed() > Duration::from_secs(24 * 3600) {
            crate::applog::purge_old();
            self.last_purge = Instant::now();
        }

        if self.open_settings_requested {
            self.open_settings(event_loop);
        }
        self.render_settings();
        // «Показать окно группы» (кнопка-глаз) — нужен event_loop для создания окна.
        for name in std::mem::take(&mut self.show_group_reqs) {
            self.show_group(event_loop, &name);
        }

        if self.open_strategies_requested {
            self.open_strategies(event_loop);
        }
        self.render_strategies();

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(8),
        ));
    }

    /// Выход из приложения — финальный флэш раскладки (на случай несохранённых
    /// изменений, не попавших под дебаунс).
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if self.layout_dirty {
            self.save_layout();
        }
    }
}
