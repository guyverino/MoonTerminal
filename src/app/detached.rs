//! Откреплённые окна App: вкладки дока (Orders/Assets/Log/Report → отдельное
//! ОС-окно с egui-сурфейсом) и чарт-окна (контейнеры графиков, вынесенные
//! drag'ом вкладки). Создание/возврат/отрисовка и ключи запомненной геометрии.

use std::sync::Arc;

use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::dock::DockTab;
use crate::window::{ChartWindow, EguiSurface};

use super::App;

/// Окно открепления вкладки дока: отдельное ОС-окно с egui-сурфейсом, которое
/// рисует контент одной вкладки своего окна-владельца (`owner`). Состояние вкладки
/// (например фильтры отчёта) остаётся в доке владельца — окно лишь рисует его в
/// свой `ui`. Закрытие окна → вкладка возвращается в док (App снимает флаг).
pub(super) struct DetachedPanel {
    /// Окно группы, из которого открепили (для Orders — чьи ордера показывать; для
    /// глобальных Report/Log/Assets — просто «откуда вызвали»).
    pub owner: WindowId,
    pub tab: DockTab,
    /// Глобальная вкладка (Report/Log/Assets) — один экземпляр на все окна групп, и
    /// флаг открепления глобальный. Orders — пер-окно (`global = false`).
    pub global: bool,
    pub window: Arc<Window>,
    pub egui: EguiSurface,
    /// Ревизия данных вкладки на прошлом кадре — для авто-перерисовки (живой
    /// отчёт/лог/ордера без необходимости двигать мышь).
    pub last_rev: u64,
    /// Состояние вида ордеров (фильтр/сортировка) этого окна (для Orders-окна).
    pub orders_view: crate::dock::OrdersViewState,
}

/// Глобальные вкладки (один экземпляр на все окна групп) vs пер-окно (Orders).
pub(super) fn is_global_tab(tab: DockTab) -> bool {
    !matches!(tab, DockTab::Orders)
}

impl App {
    /// Ключ запомненной геометрии окна открепления: глобальные — по вкладке;
    /// Orders — по вкладке+группе владельца.
    pub(super) fn detached_geom_key(&self, global: bool, tab: DockTab, owner: WindowId) -> String {
        if global {
            format!("g:{}", tab.idx())
        } else {
            let g = self
                .windows
                .get(&owner)
                .map(|h| h.workspace.group.as_str())
                .unwrap_or("");
            format!("o:{}:{}", tab.idx(), g)
        }
    }

    /// Открепить вкладку `tab` окна-владельца `owner` в отдельное окно.
    /// Глобальные вкладки (Report/Log/Assets) — один экземпляр на все окна групп
    /// (дедуп по `tab`); Orders — пер-окно (дедуп по `owner`+`tab`). Уже открепена
    /// → фокусируем существующее окно.
    pub(super) fn open_detached(
        &mut self,
        event_loop: &ActiveEventLoop,
        owner: WindowId,
        tab: DockTab,
        geom: Option<(i32, i32, u32, u32)>,
    ) {
        let global = is_global_tab(tab);
        let existing = self
            .detached
            .values()
            .find(|p| p.tab == tab && (global || p.owner == owner));
        if let Some(p) = existing {
            p.window.focus_window();
            return;
        }
        // Геометрия: явная (восстановление с диска) или запомненная для этой
        // вкладки/группы (повторное открепление встаёт на прежнее место).
        let geom = geom.or_else(|| {
            let key = self.detached_geom_key(global, tab, owner);
            self.layout.detached_geom.get(&key).map(|g| (g.x, g.y, g.w, g.h))
        });
        let mut attrs = Window::default_attributes()
            .with_title(format!("{} — MoonTerminal", tab.title()))
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1100.0, 520.0));
        if let Some((x, y, w, h)) = geom {
            attrs = attrs
                .with_position(winit::dpi::PhysicalPosition::new(x, y))
                .with_inner_size(winit::dpi::PhysicalSize::new(w.max(200), h.max(150)));
        }
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
        // Пометить откреплённой. Глобальная — флаг общий (все окна групп покажут
        // плашку); Orders — флаг в доке окна-владельца.
        if global {
            self.global_detached[tab.idx()] = true;
            for host in self.windows.values_mut() {
                host.mark_egui_dirty();
            }
        } else if let Some(host) = self.windows.get_mut(&owner) {
            host.workspace.dock.set_orders_detached(true);
            host.mark_egui_dirty();
        }
        self.detached.insert(
            det_id,
            DetachedPanel {
                owner,
                tab,
                global,
                window,
                egui,
                last_rev: u64::MAX,
                orders_view: crate::dock::OrdersViewState::default(),
            },
        );
    }

    /// Открепить чарт-вкладку (контейнер №idx окна `owner`) в отдельное чарт-окно:
    /// забрать спецификацию панелей у host'а и пересоздать графики на девайсе окна.
    pub(super) fn open_detached_chart(
        &mut self,
        event_loop: &ActiveEventLoop,
        owner: WindowId,
        idx: usize,
    ) {
        let theme = self.config.theme.clone();
        let orders_style = self.config.orders.clone();
        let epoch = self.epoch_ms;
        let taken = self
            .windows
            .get_mut(&owner)
            .and_then(|h| h.take_container(idx));
        let Some((kind, mode, spec)) = taken else {
            return;
        };
        if spec.is_empty() {
            return;
        }
        // Подпись окна: «номер-группа[-ядро]» (1-HL или 1-HL-Ядро), чтобы одинаковые
        // номера в разных группах/ядрах не путались.
        let owner_host = self.windows.get(&owner);
        let group = owner_host
            .map(|h| h.workspace.group.clone())
            .unwrap_or_default();
        let display = match kind {
            crate::chart::container::ContainerKind::Chart { num, core: None } => {
                format!("{num}-{group}")
            }
            crate::chart::container::ContainerKind::Chart {
                num,
                core: Some(cid),
            } => {
                let cn = owner_host
                    .and_then(|h| h.workspace.cores.iter().find(|c| c.id == cid))
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                format!("{num}-{group}-{cn}")
            }
            crate::chart::container::ContainerKind::Main => group.clone(),
        };
        // HWND окна-владельца → чарт-окно станет дочерним (без кнопки в таскбаре,
        // сворачивается/разворачивается вместе с родителем).
        let owner_hwnd = owner_host.and_then(|h| crate::win_taskbar::hwnd_of(&h.window));
        match ChartWindow::new(
            event_loop, owner, owner_hwnd, &display, kind, mode, spec, theme, orders_style, epoch,
        ) {
            Ok(w) => {
                self.detached_charts.insert(w.window.id(), w);
            }
            Err(e) => log::error!("чарт-окно: {e:#}"),
        }
        if let Some(h) = self.windows.get_mut(&owner) {
            h.mark_egui_dirty();
        }
    }

    /// Закрыть окно открепления по его id и вернуть вкладку в док(и).
    pub(super) fn close_detached(&mut self, det_id: WindowId) {
        if let Some(p) = self.detached.remove(&det_id) {
            // Запомнить позицию ДО закрытия — чтобы повторное открепление встало
            // на то же место (окно ещё живо в `p`, читаем его геометрию).
            if let Ok(pos) = p.window.outer_position() {
                let size = p.window.inner_size();
                let key = self.detached_geom_key(p.global, p.tab, p.owner);
                self.layout.detached_geom.insert(
                    key,
                    crate::config::GeomRect { x: pos.x, y: pos.y, w: size.width, h: size.height },
                );
            }
            // Закрыли окно лога → состояние откреплённого лога к дефолту (агрегат · Live).
            if p.tab == DockTab::Log {
                self.detached_log.reset();
            }
            if p.global {
                self.global_detached[p.tab.idx()] = false;
                for host in self.windows.values_mut() {
                    host.mark_egui_dirty();
                }
            } else if let Some(host) = self.windows.get_mut(&p.owner) {
                host.workspace.dock.set_orders_detached(false);
                host.mark_egui_dirty();
            }
        }
    }

    /// Вернуть вкладку в док (по кнопке «вернуть» на плашке) — закрывает окно.
    pub(super) fn repin(&mut self, owner: WindowId, tab: DockTab) {
        let global = is_global_tab(tab);
        let id = self
            .detached
            .iter()
            .find(|(_, p)| p.tab == tab && (global || p.owner == owner))
            .map(|(id, _)| *id);
        if let Some(id) = id {
            self.close_detached(id);
        }
    }

    /// Рисует окна открепления. Глобальные вкладки берут ОБЩЕЕ состояние (App'овый
    /// `report` / глобальный лог), Orders — из окна-владельца. Единый `content_ui`,
    /// без дубля состояния.
    pub(super) fn render_detached(&mut self) {
        // Откреплённое окно лога видит ВСЕ ядра (scope=None; агрегат = «Все ядра»).
        // Строим до мутабельных заёмов полей.
        let log_sources = self.build_log_sources(None);
        let ids: Vec<WindowId> = self.detached.keys().copied().collect();
        for det_id in ids {
            let Some(panel) = self.detached.get_mut(&det_id) else {
                continue;
            };
            let owner = panel.owner;
            let tab = panel.tab;

            // Живость: перерисовать, если данные вкладки изменились с прошлого кадра.
            let rev = match tab {
                DockTab::Report => self.report.generation(),
                DockTab::Log => self
                    .detached_log
                    .live_revision(self.session.store(), &log_sources),
                DockTab::Orders => self
                    .windows
                    .get(&owner)
                    .map(|h| h.orders_rev(self.session.store()))
                    .unwrap_or(0),
                DockTab::Assets => 0,
            };
            if rev != panel.last_rev {
                panel.egui.mark_dirty();
                panel.last_rev = rev;
            }
            if !panel.egui.needs_render() {
                continue;
            }

            // Orders — ордера окна-владельца + его Main-маркет (для фильтра); глобальные
            // — пустой срез. Report везде рисует ОБЩИЙ self.report.
            let (orders, main_market, order_cores) = if tab == DockTab::Orders {
                match self.windows.get(&owner) {
                    Some(h) => (
                        h.collect_orders(self.session.store()),
                        h.main_fullscreen(),
                        h.workspace
                            .cores
                            .iter()
                            .map(|c| (c.id, c.name.clone()))
                            .collect::<Vec<_>>(),
                    ),
                    None => continue, // владелец Orders-окна закрыт
                }
            } else {
                (Vec::new(), None, Vec::new())
            };
            let report = &mut self.report;
            let log = &mut self.detached_log;
            let store = self.session.store();
            let ov = &mut panel.orders_view;
            let log_sources = &log_sources;
            let mut clicked: Option<(crate::session::CoreId, String)> = None;
            panel.egui.render(&panel.window, "detached-pass", |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut data = crate::dock::tabs::TabData {
                        report,
                        orders: &orders,
                        order_cores: &order_cores,
                        orders_view: ov,
                        main_market: main_market.clone(),
                        store,
                        log,
                        log_sources,
                    };
                    clicked = crate::dock::tabs::content_ui(ui, tab, &mut data);
                });
            });
            // Клик по токену в откреплённых «Ордерах» → открыть на Main окна-владельца.
            if let Some((core, market)) = clicked {
                let now = super::now_ms();
                if let Some(h) = self.windows.get_mut(&owner) {
                    h.open_on_main(core, &market, now);
                    h.mark_egui_dirty();
                }
            }
        }
    }
}
