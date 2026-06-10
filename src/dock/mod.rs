//! Dock: раскладка панелей. Верхний тулбар + правая панель ордера; центральная
//! область отдаётся под wgpu график+стакан (её rect возвращается наружу).

pub mod controls;
pub mod detects;
pub mod log_panel;
pub mod order;
pub mod orders_panel;
pub mod report_view;
pub mod tabs;
pub mod toolbar;

pub use controls::{OrderControls, ScaleAction};
pub use log_panel::{LogPanelState, LogSource, LogSourceItem};
pub use orders_panel::{OrderEntry, OrderKind, OrdersViewState, PrimarySort};
pub use report_view::ReportView;
pub use tabs::DockTab;

use crate::session::{CoreId, CoreStore};
use crate::workspace::CoreInfo;
use detects::DetectRibbon;

/// Ширина правого дока детектов (точки egui). Нужна и close-кнопке (отступ).
pub const DETECTS_W: f32 = 96.0;

pub struct Dock {
    pub controls: OrderControls,
    /// Лента детектов над чартом (state: очередь кнопок + курсоры ядер).
    pub ribbon: DetectRibbon,
    /// Активная вкладка нижнего дока (Ордера/Активы/Лог/Отчёт). Своя у окна.
    pub tab: DockTab,
    /// Открепена ли вкладка «Ордера» этого окна (Orders — пер-окно; Report/Log/
    /// Assets открепляются ГЛОБАЛЬНО — их флаг живёт в App, передаётся в show).
    orders_detached: bool,
    /// Док свёрнут — видна только полоска вкладок, без контента (кнопка справа).
    collapsed: bool,
    /// Состояние лог-панели ЭТОГО окна (источник/файл/поиск/ошибки). Своё у каждого
    /// окна → в разных окнах группы во вкладках можно смотреть разный лог. Откреплённое
    /// окно лога использует ОТДЕЛЬНОЕ общее состояние (живёт в App).
    log: crate::dock::LogPanelState,
    /// Состояние вида таблицы ордеров (фильтр/сортировка) — своё у окна.
    orders_view: crate::dock::OrdersViewState,
    /// Высота развёрнутого дока (точки egui) — для персиста в layout.toml.
    /// Передаётся в tabs::show как default_height и обновляется фактической высотой.
    dock_height: f32,
}

/// Дефолтная высота развёрнутого дока (точки egui).
pub const DEFAULT_DOCK_H: f32 = 285.0;

pub struct DockOutput {
    /// Центральная область (точки egui) под график+стакан.
    pub central: egui::Rect,
    /// Новый масштаб цены (Y), если сменили в тулбаре.
    pub scale: Option<ScaleAction>,
    /// Новое состояние live-follow, если переключили.
    pub set_follow: Option<bool>,
    /// Клик по кнопке детекта → (ядро, рынок) для открытия чарта.
    pub open_detect: Option<(CoreId, String)>,
    /// Нажата кнопка закрытия графика.
    pub close_chart: bool,
    /// Rect правого дока детектов (точки egui) — host форсит кадр при движении
    /// курсора над ним (живой spotlight на кнопках).
    pub detects_rect: egui::Rect,
    /// Вкладку потянули — открепить в отдельное окно (создаёт App).
    pub detach: Option<DockTab>,
    /// Нажата «вернуть в док» на плашке откреплённой вкладки (App закроет окно).
    pub repin: Option<DockTab>,
}

impl Dock {
    pub fn new() -> Self {
        Self {
            controls: OrderControls::default(),
            ribbon: DetectRibbon::default(),
            tab: DockTab::default(),
            orders_detached: false,
            collapsed: false,
            log: crate::dock::LogPanelState::default(),
            orders_view: crate::dock::OrdersViewState::default(),
            dock_height: DEFAULT_DOCK_H,
        }
    }

    /// Высота развёрнутого дока (для сохранения раскладки).
    pub fn dock_height(&self) -> f32 {
        self.dock_height
    }

    /// Состояние вида ордеров в примитивах (для сохранения): (primary, newest, only_current, kind).
    pub fn orders_layout(&self) -> (u8, bool, bool, u8) {
        let v = &self.orders_view;
        (
            v.primary.to_u8(),
            v.newest_first,
            v.only_current_market,
            v.kind.to_u8(),
        )
    }

    /// Восстановить высоту дока и состояние ордеров из раскладки (источник — All).
    pub fn restore_extra(&mut self, dock_h: f32, primary: u8, newest: bool, only_current: bool, kind: u8) {
        if dock_h > 0.0 {
            self.dock_height = dock_h;
        }
        self.orders_view.primary = crate::dock::PrimarySort::from_u8(primary);
        self.orders_view.newest_first = newest;
        self.orders_view.only_current_market = only_current;
        self.orders_view.kind = crate::dock::OrderKind::from_u8(kind);
    }

    /// Пометить вкладку «Ордера» этого окна откреплённой/прикреплённой (App).
    pub fn set_orders_detached(&mut self, on: bool) {
        self.orders_detached = on;
    }

    /// Свёрнут ли док (для сохранения раскладки).
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    /// Восстановить из сохранённой раскладки (активная вкладка + свёрнутость).
    pub fn restore(&mut self, tab: DockTab, collapsed: bool) {
        self.tab = tab;
        self.collapsed = collapsed;
    }

    /// `report` — ОБЩИЙ `ReportView` (живёт в App, один на все окна групп).
    /// `global_detached` — глобальные флаги открепления Report/Log/Assets (Orders
    /// игнорируется: его открепление пер-окно, берётся из self.orders_detached).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        cores: &[CoreInfo],
        store: &CoreStore,
        orders: &[crate::dock::OrderEntry],
        main_market: Option<(CoreId, String)>,
        following: bool,
        chart_open: bool,
        now_ms: f64,
        report: &mut ReportView,
        log_sources: &[crate::dock::LogSourceItem],
        global_detached: [bool; 4],
    ) -> DockOutput {
        // Лента: втянуть новые детекты ядер группы и выбросить просроченные.
        self.ribbon.ingest(cores, store);
        self.ribbon.prune(now_ms);

        let tb = toolbar::show(ctx, &mut self.controls, following);

        // Нижний док с вкладками. Орд. открепление — пер-окно (self.orders_detached),
        // остальное — глобально (global_detached). Активная вкладка живёт в self.tab.
        let mut detached = global_detached;
        detached[DockTab::Orders.idx()] = self.orders_detached;
        // Ядра группы для поля-списка источника ордеров.
        let order_cores: Vec<(CoreId, String)> =
            cores.iter().map(|c| (c.id, c.name.clone())).collect();
        let mut data = tabs::TabData {
            report,
            orders,
            order_cores: &order_cores,
            orders_view: &mut self.orders_view,
            main_market,
            store,
            log: &mut self.log,
            log_sources,
        };
        let tabs_out = tabs::show(
            ctx,
            &mut self.tab,
            &mut data,
            &detached,
            &mut self.collapsed,
            self.dock_height,
        );
        // Запоминаем фактическую высоту развёрнутого дока (для персиста).
        if tabs_out.dock_height > 0.0 {
            self.dock_height = tabs_out.dock_height;
        }

        // Правый док детектов — вертикальная колонка кнопок (новые сверху). Виден
        // всегда: детекты приходят группе независимо от открытого чарта. Создаём
        // ПЕРВЫМ из правых панелей → он самый правый край окна.
        let detects_resp = egui::SidePanel::right("detects")
            .exact_width(DETECTS_W)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                self.ribbon.show(ui, now_ms)
            });
        // Открытие монеты на Main: клик по детекту (лента) ИЛИ по токену в «Ордерах».
        let open_detect = detects_resp.inner.or(tabs_out.open_market);
        let detects_rect = detects_resp.response.rect;

        // Панель ордера — правая колонка контейнера чарта (левее дока детектов).
        // При закрытом чарте скрыта. Содержимое пока пустое — только подпись.
        if chart_open {
            order::show(ctx);
        }

        // Закрытие — теперь пер-панельными крестиками (рисует host на каждой
        // панели контейнера), одиночная кнопка дока больше не нужна.
        let close_chart = false;

        let mut central = egui::Rect::NOTHING;
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                central = ui.max_rect();
            });

        DockOutput {
            central,
            scale: tb.scale,
            set_follow: tb.set_follow,
            open_detect,
            close_chart,
            detects_rect,
            detach: tabs_out.detach,
            repin: tabs_out.repin,
        }
    }
}
