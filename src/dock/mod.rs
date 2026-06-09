//! Dock: раскладка панелей. Верхний тулбар + правая панель ордера; центральная
//! область отдаётся под wgpu график+стакан (её rect возвращается наружу).

pub mod close_btn;
pub mod controls;
pub mod detects;
pub mod log_panel;
pub mod order;
pub mod orders_panel;
pub mod report_view;
pub mod tabs;
pub mod toolbar;

pub use controls::{OrderControls, ScaleAction};
pub use report_view::ReportView;
pub use tabs::DockTab;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::session::{CoreId, CoreStore};
use crate::shell::HEADER_H;
use crate::workspace::CoreInfo;
use detects::DetectRibbon;

/// Ширина правого дока детектов (точки egui). Нужна и close-кнопке (отступ).
pub const DETECTS_W: f32 = 96.0;

pub struct Dock {
    pub controls: OrderControls,
    /// Лента детектов над чартом (state: очередь кнопок + курсоры ядер).
    pub ribbon: DetectRibbon,
    /// Активная вкладка нижнего дока (Ордера/Активы/Лог/Отчёт).
    pub tab: DockTab,
    /// Состояние вкладки «Отчёт» (фильтры/таблица/своё SQLite-соединение).
    pub report: ReportView,
    /// Какие вкладки сейчас откреплены в отдельные окна (по [`DockTab::idx`]).
    /// Откреплённая вкладка в доке показывает плашку, а контент рисует окно.
    detached: [bool; 4],
}

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
    /// `generation` — счётчик writer'а отчётов (проброс во вкладку «Отчёт»).
    pub fn new(generation: Option<Arc<AtomicU64>>) -> Self {
        Self {
            controls: OrderControls::default(),
            ribbon: DetectRibbon::default(),
            tab: DockTab::default(),
            report: ReportView::new(generation),
            detached: [false; 4],
        }
    }

    /// Откреплена ли вкладка в окно (контент рисует окно, в доке — плашка).
    pub fn is_detached(&self, tab: DockTab) -> bool {
        self.detached[tab.idx()]
    }

    /// Пометить вкладку откреплённой/прикреплённой (вызывает App при создании/
    /// закрытии окна открепления).
    pub fn set_detached(&mut self, tab: DockTab, on: bool) {
        self.detached[tab.idx()] = on;
    }

    /// Состояние вкладки «Отчёт» (для рендера в окне открепления).
    pub fn report_mut(&mut self) -> &mut ReportView {
        &mut self.report
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        cores: &[CoreInfo],
        store: &CoreStore,
        orders: &[(String, crate::feed::OrderRow)],
        following: bool,
        chart_open: bool,
        now_ms: f64,
    ) -> DockOutput {
        // Лента: втянуть новые детекты ядер группы и выбросить просроченные.
        self.ribbon.ingest(cores, store);
        self.ribbon.prune(now_ms);

        let tb = toolbar::show(ctx, &mut self.controls, following);

        // Нижний док с вкладками (Ордера/Активы/Лог/Отчёт) — уровень группы, не
        // контейнера чарта. Активная вкладка живёт в self.tab.
        let tabs_out = tabs::show(ctx, &mut self.tab, &mut self.report, orders, &self.detached);

        // Панель ордера — часть контейнера чарта: при закрытом чарте скрыта
        // (контейнер пустой/серый, центральная область не растягивается контентом).
        // Содержимое пока пустое — только подпись (см. order::show).
        if chart_open {
            order::show(ctx);
        }

        // Правый док детектов — вертикальная колонка кнопок (новые сверху). Виден
        // всегда: детекты приходят группе независимо от открытого чарта.
        let detects_resp = egui::SidePanel::right("detects")
            .exact_width(DETECTS_W)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                self.ribbon.show(ui, now_ms)
            });
        let open_detect = detects_resp.inner;
        let detects_rect = detects_resp.response.rect;

        // Кнопка закрытия — в правом-верхнем углу области графика (левее дока
        // детектов), только при открытом чарте.
        let close_chart =
            chart_open && close_btn::show(ctx, HEADER_H + toolbar::TOOLBAR_H, DETECTS_W);

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
