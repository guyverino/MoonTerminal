//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.
//!
//! Подсистема выносных ОС-окон откреп-вкладок (detach/restore/repin/персист + хост
//! `DetachedChartHost`) — в [`windows`].

mod layout_popup;
mod stack;
mod windows;

use std::collections::HashMap;
use std::rc::Rc;

use stack::{
    ChartStackEntry, render_chart_stack, resolve_layout, resolve_mode, retain_nonempty_panels,
    set_panels_scale,
};

use crate::chart_persist::StackLayoutMode;

use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputEvent,
    MoonInputState, MoonRect, MoonTabItem, MoonTabStrip, MoonVirtualListScrollHandle, Panel,
    PanelEvent, PanelState, v_flex,
};

use crate::Backend;
use crate::chart_persist;
use crate::panels::ChartPanel;
use moon_core::config::{ChartBucket, ChartTheme};
use moon_core::session::CoreId;

/// Высота полоски чарт-вкладок (px). Табы в MoonTabStrip — h=28 + подчёркивание; 30 даёт
/// ровный ряд. Резервируется в layout сверху, и в неё же кладутся bounds стрипа.
const CHART_TAB_STRIP_H: f32 = 30.0;

/// Идентичность вкладки чарта. Main — фуллскрин; Add(номер, bucket) — AddToChart-вкладка,
/// где `bucket` — куда сведены графики ядра внутри группы (своё ядро / общая / именованная
/// связка; см. `ChartBucket`). Порт egui `ContainerKind` (Main / Chart{num, bucket}).
#[derive(Clone, PartialEq, Eq)]
enum Tab {
    Main,
    Add(u32, ChartBucket),
}

pub struct ChartTabs {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    /// Main-чарты: несколько рынков как stack отдельных `ChartPanel`, активный — fullscreen.
    main: Entity<MainChartStack>,
    /// AddToChart-вкладки (номер, bucket, стек графиков), отсортированы по (номер, bucket).
    add: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Откреплённые в своё ОС-окно вкладки — держим Entity, чтобы при закрытии окна
    /// вернуть панель в стрип (repin) и чтобы новые детекты этого номера шли в неё.
    detached: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Активная вкладка.
    active: Tab,
    /// Сколько монет на вкладке (num, bucket) пользователь уже «видел» (был на ней активен).
    /// Бейдж = pane_count - seen (новые с момента ухода). На активной вкладке seen догоняет
    /// pane_count → бейджа нет. Уходишь → seen заморожен → новые детекты растят бейдж.
    seen: HashMap<(u32, ChartBucket), usize>,
    /// Per-core курсор учтённых AddToChart-детектов.
    add_seq: HashMap<CoreId, u64>,
    /// Сигнатура входов, которые реально меняют tab-strip: AddToChart-детекты,
    /// split-настройка и явный запрос открыть монету на Main.
    last_sig: u64,
    /// Последняя виденная `price_scale_rev` тулбара — применяем масштаб к АКТИВНОЙ панели
    /// только когда rev вырос (юзер выбрал), иначе синхроним показ масштаба активной вкладки.
    last_scale_rev: u64,
    /// Откреп-вкладки на восстановление при загрузке (из charts.json): создаём их пустыми и
    /// открываем окна на ПЕРВОМ render (не в конструкторе окна группы — нельзя вложенно).
    restore_pending: Vec<(u32, ChartBucket, chart_persist::WinGeom, Option<f32>)>,
    /// Handle окна группы. Backend-observe callbacks не получают `&mut Window`, но open/activate
    /// и restore detached окон должны жить вне `render()`.
    window_handle: AnyWindowHandle,
    focus: FocusHandle,
    /// Открыт ли попап настроек раскладки (кнопка ⚙ в полоске вкладок). Применяется к
    /// АКТИВНОЙ вкладке.
    layout_popup_open: bool,
    /// Был ли курсор уже внутри попапа (для авто-скрытия: закрываем по уходу ТОЛЬКО после
    /// первого входа — иначе попап закрылся бы сразу, т.к. при открытии курсор ещё на кнопке).
    layout_popup_hovered: bool,
    /// Поле ввода высоты слота в попапе раскладки (Blur/Enter → применить к активной вкладке).
    layout_height_input: Entity<MoonInputState>,
}


/// Main-вкладка: один рынок = один отдельный `ChartPanel`/`gpu_canvas`.
/// Обычный клик по рынку в таблицах открывает/фокусирует его fullscreen. ПКМ по ОБЛАСТИ
/// ГРАФИКА (не по стакану) текущего графика переключает fullscreen ↔ весь stack, не возвращая
/// несколько рынков внутрь одного `ChartEngine`.
pub(crate) struct MainChartStack {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    active: Option<usize>,
    show_stack: bool,
    scale: Option<f32>,
    /// Per-tab режим раскладки (None = глобальный дефолт конфига).
    layout_mode: Option<StackLayoutMode>,
    /// Per-tab высота слота px (None = глобальный дефолт).
    layout_height: Option<u16>,
    scroll: MoonVirtualListScrollHandle,
}

impl MainChartStack {
    fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            charts: Vec::new(),
            active: None,
            show_stack: false,
            scale: None,
            layout_mode: None,
            layout_height: None,
            scroll: MoonVirtualListScrollHandle::new(),
        };
        if let Some((core, market)) = focus_open {
            this.open_or_focus(core, market, cx);
        }
        this
    }

    fn create_panel(
        &self,
        core: CoreId,
        market: &str,
        cx: &mut Context<Self>,
    ) -> Entity<ChartPanel> {
        let backend = self.backend.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let market = market.to_string();
        let panel =
            cx.new(|cx| ChartPanel::new_main(backend, Some((core, market)), epoch, theme, cx));
        cx.observe(&panel, |this, _, cx| {
            if this.prune_empty(cx) {
                this.sync_visibility(cx);
                this.sync_backend_active(cx);
                cx.notify();
            }
        })
        .detach();
        if self.scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(self.scale, pcx));
        }
        panel
    }

    fn open_or_focus(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .charts
            .iter()
            .position(|entry| entry.core == core && entry.market == market)
        {
            self.active = Some(ix);
            self.show_stack = false;
            self.sync_visibility(cx);
            self.sync_backend_active(cx);
            cx.notify();
            return;
        }

        let panel = self.create_panel(core, &market, cx);
        self.charts.push(ChartStackEntry {
            core,
            market,
            panel,
        });
        self.active = Some(self.charts.len() - 1);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_active(cx);
        cx.notify();
    }

    fn prune_empty(&mut self, cx: &App) -> bool {
        let active_key = self
            .active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        let changed = retain_nonempty_panels(&mut self.charts, cx);
        if self.charts.is_empty() {
            self.active = None;
            self.show_stack = false;
        } else {
            self.active = active_key
                .and_then(|(core, market)| {
                    self.charts
                        .iter()
                        .position(|entry| entry.core == core && entry.market == market)
                })
                .or_else(|| Some(self.active.unwrap_or(0).min(self.charts.len() - 1)));
        }
        changed
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale == pct {
            return;
        }
        self.scale = pct;
        set_panels_scale(&self.charts, pct, cx);
        cx.notify();
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height(&self) -> Option<u16> {
        self.layout_height
    }

    /// Применить per-tab раскладку (режим + высоту) к этому стеку.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode && self.layout_height == height {
            return;
        }
        self.layout_mode = mode;
        self.layout_height = height;
        cx.notify();
    }

    pub(crate) fn active_target(&self, cx: &App) -> Option<(CoreId, String)> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .and_then(|entry| entry.panel.read(cx).active_target())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_data_handle(&self, cx: &App) -> Option<crate::chartdx::ChartDataHandle> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| entry.panel.read(cx).debug_data_handle())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active else {
            log::warn!("debug fill main chart: no active main chart");
            return false;
        };
        let Some(entry) = self.charts.get(ix) else {
            log::warn!("debug fill main chart: active main chart index is stale");
            return false;
        };
        entry
            .panel
            .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx))
    }

    fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible {
            self.sync_visibility(cx);
        } else {
            for entry in &self.charts {
                entry.panel.update(cx, |panel, _| {
                    panel.set_main_stack_scroll(false);
                    panel.set_scene_visible(false);
                });
            }
        }
    }

    fn sync_visibility(&mut self, cx: &mut Context<Self>) {
        for (ix, entry) in self.charts.iter().enumerate() {
            // Fullscreen: ровно активный график видим. Stack: конкретные видимые tiles
            // сами выставят visible=true в `ChartPanel::render`; offscreen элементы
            // виртуального списка остаются false и не гоняют prepare.
            let visible = !self.show_stack && Some(ix) == self.active;
            let stack_scroll = self.show_stack;
            entry.panel.update(cx, |panel, _| {
                panel.set_main_stack_scroll(stack_scroll);
                panel.set_scene_visible(visible);
            });
        }
    }

    fn sync_backend_active(&self, cx: &mut Context<Self>) {
        let target = self.active_target(cx);
        self.backend
            .update(cx, |b, _| b.set_main_chart_target(&self.group, target));
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(handle) = self.debug_data_handle(cx) {
                self.backend.update(cx, |b, _| {
                    b.register_debug_main_chart(self.group.clone(), handle);
                });
            }
        }
    }

    fn toggle_from_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.charts.len() {
            return;
        }
        self.active = Some(ix);
        self.show_stack = !self.show_stack;
        self.sync_visibility(cx);
        self.sync_backend_active(cx);
        cx.notify();
    }

    fn render_tile(
        &self,
        ix: usize,
        panel: Entity<ChartPanel>,
        height: Option<f32>,
        flex: bool,
        border: Rgba,
        entity: Entity<Self>,
    ) -> Stateful<Div> {
        let panel_for_event = panel.clone();
        let mut tile = div()
            .id(("main-chart-stack-tile", ix))
            .w_full()
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .on_mouse_up(
                MouseButton::Right,
                move |event: &MouseUpEvent, _window, app| {
                    // Возврат из фулскрина — ПКМ по ОБЛАСТИ ГРАФИКА (не по стакану) и только
                    // коротким кликом (не зум-перетаскиванием цены).
                    let panel = panel_for_event.read(app);
                    if panel.window_pos_in_chart_plot(event.position) && !panel.rmb_was_moved() {
                        entity.update(app, |this, cx| this.toggle_from_chart(ix, cx));
                        app.stop_propagation();
                    }
                },
            );
        if let Some(height) = height {
            // Фикс. высота: min_h=0 даёт COMPRESS-сжатие при переполнении окна (в SCROLL —
            // безвредно, высота слота фиксирована виртуальным списком).
            tile = tile.h(px(height)).min_h(px(0.0));
        }
        if flex {
            tile = tile.flex_1().min_h(px(0.0));
        }
        tile.child(div().size_full().relative().overflow_hidden().child(panel))
    }
}

impl Render for MainChartStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            return div()
                .relative()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(220.0))
                .into_any_element();
        }

        let active = self.active.unwrap_or(0).min(self.charts.len() - 1);
        if !self.show_stack {
            let panel = self.charts[active].panel.clone();
            let entity = cx.entity();
            return self
                .render_tile(active, panel, None, false, rgb(palette.border), entity)
                .size_full()
                .border_0()
                .into_any_element();
        }

        // Stack: per-tab раскладка (FIT/SCROLL/COMPRESS + высота), иначе глобальный дефолт.
        let (scroll, compress, cfg_h) =
            resolve_layout(self.layout_mode, self.layout_height, self.backend.read(cx));
        let count = self.charts.len();
        let border = rgb(palette.border);
        let base_id = format!("main-chart-stack-{}", self.group);
        let entity = cx.entity();
        render_chart_stack(
            &base_id,
            self,
            entity,
            count,
            scroll,
            compress,
            cfg_h,
            &self.scroll,
            border,
            |s, ix| s.charts.get(ix).map(|e| e.panel.clone()),
            |s, ix, panel, height, flex, border, ent| {
                s.render_tile(ix, panel, height, flex, border, ent)
                    .into_any_element()
            },
        )
    }
}

/// AddToChart-вкладка: визуально это один список графиков, но архитектурно каждый график —
/// отдельный `ChartPanel`/`gpu_canvas`/dirty entity. Не возвращаемся к ебанине
/// `ChartPanel -> Container.panes`, где mousemove одного графика перерисовывал overlay всех.
pub(crate) struct AddChartStack {
    backend: Entity<Backend>,
    num: u32,
    bucket: ChartBucket,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    scale: Option<f32>,
    /// Per-tab режим раскладки (None = глобальный дефолт конфига).
    layout_mode: Option<StackLayoutMode>,
    /// Per-tab высота слота px (None = глобальный дефолт).
    layout_height: Option<u16>,
    /// Скролл-хэндл вертикального MoonVirtualList (scroll-режим стека).
    scroll: MoonVirtualListScrollHandle,
}

impl AddChartStack {
    fn new(
        backend: Entity<Backend>,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
    ) -> Self {
        Self {
            backend,
            num,
            bucket,
            epoch,
            theme,
            charts: Vec::new(),
            scale: None,
            layout_mode: None,
            layout_height: None,
            scroll: MoonVirtualListScrollHandle::new(),
        }
    }

    fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64, cx: &mut Context<Self>) {
        if let Some(entry) = self
            .charts
            .iter()
            .find(|entry| entry.core == core && entry.market == market)
        {
            entry
                .panel
                .update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
            return;
        }

        let backend = self.backend.clone();
        let num = self.num;
        let bucket = self.bucket.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let scale = self.scale;
        let panel = cx.new(|cx| ChartPanel::new_addto(backend, num, bucket, epoch, theme, cx));
        cx.observe(&panel, |this, _, cx| {
            if this.prune_empty(cx) {
                cx.notify();
            }
        })
        .detach();
        if scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(scale, pcx));
        }
        panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
        self.charts.push(ChartStackEntry {
            core,
            market: market.to_string(),
            panel,
        });
        cx.notify();
    }

    fn prune_empty(&mut self, cx: &App) -> bool {
        retain_nonempty_panels(&mut self.charts, cx)
    }

    pub(crate) fn pane_count(&self, cx: &App) -> usize {
        self.charts
            .iter()
            .filter(|entry| entry.panel.read(cx).pane_count() > 0)
            .count()
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale == pct {
            return;
        }
        self.scale = pct;
        set_panels_scale(&self.charts, pct, cx);
        cx.notify();
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height(&self) -> Option<u16> {
        self.layout_height
    }

    /// Применить per-tab раскладку (режим + высоту) к этому стеку.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode && self.layout_height == height {
            return;
        }
        self.layout_mode = mode;
        self.layout_height = height;
        cx.notify();
    }

    fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, _| panel.set_scene_visible(visible));
        }
    }

    pub(crate) fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, pcx| panel.close_all_panes(pcx));
        }
        self.charts.clear();
        cx.notify();
    }
}

impl Render for AddChartStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            // Непрозрачный фон: в выносном окне Root=NoFill и own-pass нет → без фона
            // сквозь логотип просвечивает белая подложка окна.
            return div()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(220.0))
                .into_any_element();
        }

        // Stack: per-tab раскладка (FIT/SCROLL/COMPRESS + высота), иначе глобальный дефолт.
        // ВАЖНО: чарт-слоты ПРОЗРАЧНЫЕ. own-pass (combo/стакан) — слой GpuCanvasLayer::UnderScene
        // (под сценой); любой непрозрачный `.bg()` над слотом его перекрывает. Разделитель — рамка.
        let (scroll, compress, cfg_h) =
            resolve_layout(self.layout_mode, self.layout_height, self.backend.read(cx));
        let count = self.charts.len();
        let border = rgb(palette.border);
        let base_id = format!("add-chart-stack-{}", self.num);
        let entity = cx.entity();
        render_chart_stack(
            &base_id,
            self,
            entity,
            count,
            scroll,
            compress,
            cfg_h,
            &self.scroll,
            border,
            |s, ix| s.charts.get(ix).map(|e| e.panel.clone()),
            |s, ix, panel, height, flex, border, _ent| {
                let id = match s.charts.get(ix) {
                    Some(e) => format!("add-chart-stack-tile-{}-{}-{}", s.num, e.core, e.market),
                    None => format!("add-chart-stack-tile-{}-{ix}", s.num),
                };
                let mut tile = div()
                    .id(SharedString::from(id))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .border_1()
                    .border_color(border);
                if let Some(h) = height {
                    tile = tile.h(px(h)).min_h(px(0.0));
                }
                if flex {
                    tile = tile.flex_1().min_h(px(0.0));
                }
                tile.child(div().size_full().relative().overflow_hidden().child(panel))
                    .into_any_element()
            },
        )
    }
}

impl ChartTabs {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let main = cx.new(|cx| {
            MainChartStack::new(
                backend.clone(),
                group.clone(),
                focus_open,
                epoch,
                theme.clone(),
                cx,
            )
        });
        let initial_sig = chart_tabs_sig(backend.read(cx), &group);
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(main_handle) = main.read(cx).debug_data_handle(cx) {
                backend.update(cx, |b, _| {
                    b.register_debug_main_chart(group.clone(), main_handle);
                });
            }
        }
        // Из charts.json: масштаб Main (num=0) и список откреп-вкладок этой группы на
        // восстановление (создадим пустыми на первом render → ждут детект).
        #[allow(clippy::type_complexity)]
        let (main_scale, main_layout, restore_pending): (
            Option<f32>,
            (Option<StackLayoutMode>, Option<u16>),
            Vec<_>,
        ) = {
            let specs = &backend.read(cx).chart_specs;
            let main_spec = specs.iter().find(|s| s.group == group && s.num == 0);
            let main_scale = main_spec.and_then(|s| s.scale);
            let main_layout = main_spec.map_or((None, None), |s| (s.layout_mode, s.layout_height));
            let pending = specs
                .iter()
                .filter(|s| s.group == group && s.num >= 1 && s.detached.is_some())
                .map(|s| (s.num, s.bucket(), s.detached.unwrap(), s.scale))
                .collect();
            (main_scale, main_layout, pending)
        };
        if main_scale.is_some() {
            main.update(cx, |p, pcx| p.set_scale(main_scale, pcx));
        }
        if main_layout.0.is_some() || main_layout.1.is_some() {
            main.update(cx, |p, pcx| p.set_layout(main_layout.0, main_layout.1, pcx));
        }
        cx.observe(&backend, |this, backend, cx| {
            let sig = chart_tabs_sig(backend.read(cx), &this.group);
            if sig == this.last_sig {
                return;
            }
            this.last_sig = sig;
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            this.drain_debug_fill_main_chart(cx);
            this.handle_open_request(cx);
            this.ingest(cx);
            this.drain_chart_repin(cx);
            this.sync_active_scale(cx);
            this.sync_main_chart_target(cx);
            this.sync_seen_for_active(cx);
            this.persist_scales(cx);
            this.sync_inactive_chart_visibility(cx);
            this.last_sig = chart_tabs_sig(backend.read(cx), &this.group);
            cx.notify();
        })
        .detach();
        // Поле высоты попапа раскладки: Blur (клик вне) / Enter → применить к активной вкладке.
        let layout_height_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_height_input,
            |this, inp, ev: &MoonInputEvent, cx| {
                if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    return;
                }
                let raw = inp.read(cx).value().to_string();
                if let Ok(h) = raw.trim().parse::<u16>() {
                    let mode = this.active_layout_mode(cx);
                    this.apply_layout(mode, Some(h.clamp(120, 2000)), cx);
                }
            },
        )
        .detach();
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            main,
            add: Vec::new(),
            detached: Vec::new(),
            active: Tab::Main,
            seen: HashMap::new(),
            add_seq: HashMap::new(),
            last_sig: initial_sig,
            last_scale_rev: 0,
            restore_pending,
            window_handle: window.window_handle(),
            focus: cx.focus_handle(),
            layout_popup_open: false,
            layout_popup_hovered: false,
            layout_height_input,
        };
        this.restore_detached(cx);
        this.sync_active_scale(cx);
        this.sync_main_chart_target(cx);
        this.persist_scales(cx);
        this
    }

    fn handle_open_request(&mut self, cx: &mut Context<Self>) {
        let pending = {
            let b = self.backend.read(cx);
            b.open_request
                .as_ref()
                .cloned()
                .filter(|(core, _)| core_belongs_to_group(b, self.group.as_str(), *core))
        };
        let Some((pending_core, pending_market)) = pending else {
            return;
        };
        let req = self.backend.update(cx, |b, _| {
            if b.open_request
                .as_ref()
                .is_some_and(|(core, market)| *core == pending_core && market == &pending_market)
            {
                let activate = b.open_request_activate;
                b.open_request_activate = false;
                b.open_request.take().map(|(c, m)| (c, m, activate))
            } else {
                None
            }
        });
        if let Some((core, market, activate)) = req {
            self.main
                .update(cx, |p, pcx| p.open_or_focus(core, market, pcx));
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            // П.1: поднимаем/фокусируем окно Main ТОЛЬКО для дабл-клика по чарту
            // (open_request_activate). Клики в Ордерах/Детектах открывают монету, но окно
            // не активируют — иначе любой клик дёргал бы окно на передний план.
            if activate {
                let handle = self.window_handle;
                cx.defer(move |app| {
                    let _ = handle.update(app, |_, window, _| window.activate_window());
                });
            }
            self.sync_inactive_chart_visibility(cx);
            self.sync_seen_for_active(cx);
            self.sync_active_scale(cx);
            self.sync_main_chart_target(cx);
            self.persist_scales(cx);
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn drain_debug_fill_main_chart(&mut self, cx: &mut Context<Self>) {
        let requested = self.backend.update(cx, |b, _| {
            if b.debug_fill_main_chart_group.as_deref() == Some(self.group.as_str()) {
                b.debug_fill_main_chart_group = None;
                Some(b.debug_fill_main_chart_rev)
            } else {
                None
            }
        });
        if requested.is_some() {
            let rev = requested.unwrap_or_default();
            let filled = self
                .main
                .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx));
            if filled {
                log::info!(
                    "debug fill main chart: delivered group={} rev={} result=ok",
                    self.group,
                    rev
                );
            } else {
                log::warn!(
                    "debug fill main chart: delivered group={} rev={} result=failed",
                    self.group,
                    rev
                );
            }
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            cx.notify();
        }
    }

    /// Открыть/закрыть попап настроек раскладки. При открытии заполняет поле высоты текущим
    /// значением активной вкладки (нужен `window` для `set_value`).
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout_popup_open = !self.layout_popup_open;
        self.layout_popup_hovered = false;
        if self.layout_popup_open {
            let (_, _, h) = resolve_layout(
                self.active_layout_mode(cx),
                self.active_layout_height(cx),
                self.backend.read(cx),
            );
            let val = format!("{}", h as u16);
            self.layout_height_input
                .update(cx, |st, c| st.set_value(val, window, c));
        }
        cx.notify();
    }

    /// Ключ персиста активной вкладки: Main → (0, Shared); AddToChart → (num, bucket).
    fn active_stack_key(&self) -> (u32, ChartBucket) {
        match &self.active {
            Tab::Main => (0, ChartBucket::Shared),
            Tab::Add(n, b) => (*n, b.clone()),
        }
    }

    /// Per-tab режим раскладки активной вкладки (None = глобальный дефолт).
    fn active_layout_mode(&self, cx: &App) -> Option<StackLayoutMode> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_mode(),
            Tab::Add(n, b) => self
                .add
                .iter()
                .find(|(num, bk, _)| num == n && bk == b)
                .and_then(|(_, _, p)| p.read(cx).layout_mode()),
        }
    }

    /// Per-tab высота слота активной вкладки (None = глобальный дефолт).
    fn active_layout_height(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height(),
            Tab::Add(n, b) => self
                .add
                .iter()
                .find(|(num, bk, _)| num == n && bk == b)
                .and_then(|(_, _, p)| p.read(cx).layout_height()),
        }
    }

    /// Применить раскладку (режим + высоту) к АКТИВНОЙ вкладке и сохранить в charts.json.
    fn apply_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |s, c| s.set_layout(mode, height, c)),
            Tab::Add(n, b) => {
                if let Some((_, _, p)) = self.add.iter().find(|(num, bk, _)| *num == n && *bk == b) {
                    p.update(cx, |s, c| s.set_layout(mode, height, c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.layout_mode = mode;
            s.layout_height = height;
        });
        cx.notify();
    }

    /// Ингест AddToChart-детектов (add_to_chart>0) → создать/наполнить вкладку.
    /// Ключ вкладки — `ChartBucket` ядра (своё ядро / общая / именованная связка),
    /// резолвится из конфига ядра + глоб. `charts_split_by_core`.
    /// БЕЗ авто-перехода: active не трогаем (порт «не уводить на чарт при детекте»).
    fn ingest(&mut self, cx: &mut Context<Self>) {
        let (split, fresh, cursors): (
            bool,
            Vec<(u32, CoreId, ChartBucket, String, f64)>,
            Vec<(CoreId, u64)>,
        ) = {
            let b = self.backend.read(cx);
            let split = b.config.charts_split_by_core;
            let mut fresh = Vec::new();
            let mut cursors = Vec::new();
            for s in b
                .session
                .sessions()
                .iter()
                .filter(|s| s.group == self.group)
            {
                let id = s.id;
                let Some(d) = b.session.store().core(id) else {
                    continue;
                };
                // Bucket ядра — из его конфига (связка) + глоб. split. Нет конфига → своя вкладка.
                let bucket = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == id)
                    .map(|sv| sv.chart_bucket(split))
                    .unwrap_or(ChartBucket::Core(id));
                let last = self.add_seq.get(&id).copied().unwrap_or(0);
                let mut mx = last;
                for det in &d.detects {
                    if det.seq <= last {
                        continue;
                    }
                    mx = mx.max(det.seq);
                    if det.add_to_chart > 0 {
                        let ttl = (det.keep_in_chart_secs.max(1) as f64) * 1000.0;
                        fresh.push((
                            det.add_to_chart,
                            id,
                            bucket.clone(),
                            det.market.clone(),
                            ttl,
                        ));
                    }
                }
                if mx != last {
                    cursors.push((id, mx));
                }
            }
            (split, fresh, cursors)
        };
        for (id, mx) in cursors {
            self.add_seq.insert(id, mx);
        }
        if fresh.is_empty() {
            return;
        }
        // detect-diag: AddToChart-детекты дошли до UI этой группы. fresh — сколько монет
        // на добавление в этом проходе. (env MOON_DETECT_DIAG, off by default.)
        moon_core::detect_diag::line(&format!(
            "[ingest] group={} split={split} fresh={} existing_tabs={}",
            self.group,
            fresh.len(),
            self.add.len()
        ));
        let (epoch, theme, backend) = (self.epoch, self.theme.clone(), self.backend.clone());
        for (n, core, bucket, market, ttl) in fresh {
            let in_detached = self
                .detached
                .iter()
                .any(|(num, c, _)| *num == n && *c == bucket);
            if let Some((_, _, tab)) = self
                .add
                .iter()
                .find(|(num, c, _)| *num == n && *c == bucket)
                .or_else(|| {
                    self.detached
                        .iter()
                        .find(|(num, c, _)| *num == n && *c == bucket)
                })
            {
                if in_detached {
                    moon_core::detect_diag::line(&format!(
                        "[ingest] +coin n={n} bucket={bucket:?} market={market} → DETACHED-окно"
                    ));
                }
                tab.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
            } else {
                let panel = cx.new(|_| {
                    AddChartStack::new(backend.clone(), n, bucket.clone(), epoch, theme.clone())
                });
                // Восстановить сохранённый масштаб и раскладку этой вкладки (charts.json).
                let (saved_scale, saved_layout) = {
                    let specs = &self.backend.read(cx).chart_specs;
                    let spec = specs
                        .iter()
                        .find(|s| s.group == self.group && s.num == n && s.bucket() == bucket);
                    (
                        spec.and_then(|s| s.scale),
                        spec.map_or((None, None), |s| (s.layout_mode, s.layout_height)),
                    )
                };
                if saved_scale.is_some() {
                    panel.update(cx, |p, pcx| p.set_scale(saved_scale, pcx));
                }
                if saved_layout.0.is_some() || saved_layout.1.is_some() {
                    panel.update(cx, |p, pcx| p.set_layout(saved_layout.0, saved_layout.1, pcx));
                }
                panel.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
                self.add.push((n, bucket.clone(), panel));
                // Порядок вкладок: по (номер, bucket) — как egui sort_by_key.
                self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
                moon_core::detect_diag::line(&format!(
                    "[ingest] NEW tab n={n} bucket={bucket:?} (total_tabs={})",
                    self.add.len()
                ));
                // active НЕ меняем — не уводим пользователя на новую вкладку.
            }
        }
        self.sync_seen_for_active(cx);
        self.persist_scales(cx);
    }

    fn add_stack(&self, n: u32, bucket: &ChartBucket) -> Option<Entity<AddChartStack>> {
        self.add
            .iter()
            .find(|(num, c, _)| *num == n && c == bucket)
            .map(|(_, _, p)| p.clone())
    }

    /// Активная панель (Main или AddToChart stack) для показа.
    fn active_element(&self) -> AnyElement {
        match &self.active {
            Tab::Main => self.main.clone().into_any_element(),
            Tab::Add(n, bucket) => self
                .add_stack(*n, bucket)
                .map(|p| p.into_any_element())
                .unwrap_or_else(|| self.main.clone().into_any_element()),
        }
    }

    fn active_scale(&self, cx: &App) -> Option<f32> {
        match &self.active {
            Tab::Main => self.main.read(cx).scale(),
            Tab::Add(n, bucket) => self
                .add_stack(*n, bucket)
                .map(|p| p.read(cx).scale())
                .unwrap_or_else(|| self.main.read(cx).scale()),
        }
    }

    fn main_chart_target(&self, cx: &App) -> Option<(CoreId, String)> {
        self.main.read(cx).active_target(cx)
    }

    fn sync_main_chart_target(&self, cx: &mut Context<Self>) {
        let target = self.main_chart_target(cx);
        self.backend
            .update(cx, |b, _| b.set_main_chart_target(&self.group, target));
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(handle) = self.main.read(cx).debug_data_handle(cx) {
                self.backend.update(cx, |b, _| {
                    b.register_debug_main_chart(self.group.clone(), handle)
                });
            }
        }
    }

    fn set_active_scale(&self, pct: Option<f32>, cx: &mut Context<Self>) {
        match &self.active {
            Tab::Main => self.main.update(cx, |p, pcx| p.set_scale(pct, pcx)),
            Tab::Add(n, bucket) => {
                if let Some(stack) = self.add_stack(*n, bucket) {
                    stack.update(cx, |p, pcx| p.set_scale(pct, pcx));
                }
            }
        }
    }

    /// Метка вкладки (П.4): «номер-группа», «номер-группа-ядро» (своё ядро) или
    /// «номер-группа-связка» (именованная связка).
    fn add_label(&self, n: u32, bucket: &ChartBucket, cx: &App) -> String {
        chart_pane_label(&self.backend, &self.group, n, bucket, cx)
    }

    /// Неактивные вкладки отсутствуют в текущей GPUI scene, значит их chart data observe не должен
    /// гонять CPU prepare. Активная/откреплённая панель сама выставит visible=true в своём render.
    fn sync_inactive_chart_visibility(&self, cx: &mut Context<Self>) {
        let active = self.active.clone();
        if matches!(active, Tab::Main) {
            self.main
                .update(cx, |panel, pcx| panel.set_scene_visible(true, pcx));
        } else {
            self.main
                .update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
        }
        for (n, c, panel) in &self.add {
            if Tab::Add(*n, c.clone()) != active {
                panel.update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
            }
        }
    }

    fn sync_seen_for_active(&mut self, cx: &App) {
        if let Tab::Add(n, c) = self.active.clone() {
            if let Some((_, _, panel)) = self.add.iter().find(|(num, cc, _)| *num == n && *cc == c)
            {
                let cnt = panel.read(cx).pane_count(cx);
                self.seen.insert((n, c), cnt);
            }
        }
    }

    fn sync_active_scale(&mut self, cx: &mut Context<Self>) {
        let (rev, want) = {
            let b = self.backend.read(cx);
            (b.price_scale_rev, b.price_scale)
        };
        if rev != self.last_scale_rev {
            self.last_scale_rev = rev;
            self.set_active_scale(want, cx);
        } else {
            let cur = self.active_scale(cx);
            self.backend.update(cx, |b, _| {
                if b.price_scale != cur {
                    b.price_scale = cur;
                }
            });
        }
    }
}

fn chart_tabs_sig(b: &Backend, group: &str) -> u64 {
    let mut sig = if b
        .open_request
        .as_ref()
        .is_some_and(|(core, _)| core_belongs_to_group(b, group, *core))
    {
        b.open_request_rev
    } else {
        0
    };
    sig = sig
        .wrapping_mul(31)
        .wrapping_add(u64::from(b.config.charts_split_by_core));
    // Раскладка стека (скролл/сжатие/высота) — перерисовать активный стек при смене настройки.
    sig = sig
        .wrapping_mul(31)
        .wrapping_add(u64::from(b.config.charts_stack_scroll));
    sig = sig
        .wrapping_mul(31)
        .wrapping_add(u64::from(b.config.charts_stack_compress));
    sig = sig
        .wrapping_mul(31)
        .wrapping_add(u64::from(b.config.chart_stack_height));
    if b.price_scale_group.as_deref() == Some(group) {
        sig = sig.wrapping_mul(31).wrapping_add(b.price_scale_rev);
    }
    for (g, n, bucket) in &b.chart_repin_request {
        if g == group {
            sig = sig
                .wrapping_mul(31)
                .wrapping_add(*n as u64)
                .wrapping_mul(31)
                .wrapping_add(text_sig(&format!("{bucket:?}")));
        }
    }
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    if b.debug_fill_main_chart_group.as_deref() == Some(group) {
        sig = sig
            .wrapping_mul(31)
            .wrapping_add(b.debug_fill_main_chart_rev);
    }
    let store = b.session.store();
    for s in b.session.sessions().iter().filter(|s| s.group == group) {
        if let Some(d) = store.core(s.id) {
            sig = sig.wrapping_mul(31).wrapping_add(d.detects_rev);
        }
    }
    sig
}

fn text_sig(text: &str) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        sig ^= byte as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig
}

fn core_belongs_to_group(b: &Backend, group: &str, core: CoreId) -> bool {
    b.session
        .sessions()
        .iter()
        .any(|s| s.id == core && s.group == group)
}

impl EventEmitter<PanelEvent> for ChartTabs {}
impl Focusable for ChartTabs {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartTabs {
    fn panel_name(&self) -> &'static str {
        "ChartTabs"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Чарты")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        // AddToChart-вкладки не сохраняем: они пересоздаются из детектов при работе.
        crate::dock_persist::panel_state_with_group("ChartTabs", &self.group)
    }
    fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy {
        MoonBackgroundPolicy::NoFill
    }
}

impl Render for ChartTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Снимок вкладок — чтобы callbacks не держали borrow self.add. (Tab, label, count для
        // ширины, unread для бейджа, detachable.)
        let mut tabs: Vec<(Tab, String, usize, usize, bool)> =
            vec![(Tab::Main, "Main".to_string(), 0, 0, false)];
        tabs.extend(self.add.iter().map(|(n, bucket, panel)| {
            let count = panel.read(cx).pane_count(cx);
            let seen = self.seen.get(&(*n, bucket.clone())).copied().unwrap_or(0);
            (
                Tab::Add(*n, bucket.clone()),
                self.add_label(*n, bucket, cx),
                count,
                count.saturating_sub(seen),
                true,
            )
        }));
        let tab_keys = Rc::new(
            tabs.iter()
                .map(|(tab, _, _, _, _)| tab.clone())
                .collect::<Vec<_>>(),
        );
        let items = tabs
            .iter()
            .map(|(tab, label, _count, unread, detachable)| {
                let width = (label.chars().count() as f32 * 7.0
                    + if *unread > 0 { 38.0 } else { 28.0 }
                    + if *detachable { 20.0 } else { 0.0 })
                .clamp(72.0, 168.0);
                let mut item = MoonTabItem::new(label.clone())
                    .width(width)
                    .selected(self.active == *tab)
                    .closable(*detachable);
                if *unread > 0 {
                    item = item.badge(unread.to_string());
                }
                item
            })
            .collect::<Vec<_>>();
        let view = cx.entity();
        // MoonTabStrip рисует ВСЕ табы абсолютно и режет по `overflow_hidden` ПО СВОИМ
        // bounds. Без явных bounds его root схлопывается в 0×0 → полоска невидима, а чарт
        // (flex_1 ниже) забирает всю высоту (ровно баг «график есть, вкладок нет»). Даём
        // ширину окна (контейнер ниже обрежет до ширины панели) и фикс. высоту полосы.
        let strip_w = f32::from(window.viewport_size().width).max(1.0);
        let strip = MoonTabStrip::new("chart-tabs-strip")
            .padding_left(8.0)
            .gap(4.0)
            .bounds(MoonRect::new(0.0, 0.0, strip_w, CHART_TAB_STRIP_H))
            .items(items)
            .on_click({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if !matches!(tab_id, Tab::Main) && event.click_count() >= 2 {
                            this.detach(tab_id, cx);
                        } else if matches!(tab_id, Tab::Main)
                            || this
                                .add
                                .iter()
                                .any(|(n, c, _)| Tab::Add(*n, c.clone()) == tab_id)
                        {
                            if this.active != tab_id {
                                this.active = tab_id;
                                this.sync_seen_for_active(cx);
                                this.sync_active_scale(cx);
                                this.sync_inactive_chart_visibility(cx);
                                this.persist_scales(cx);
                                cx.notify();
                            }
                        }
                    });
                }
            })
            .on_close({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, _event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    if matches!(tab_id, Tab::Main) {
                        return;
                    }
                    view.update(app, |this, cx| {
                        this.add
                            .retain(|(n, c, _)| Tab::Add(*n, c.clone()) != tab_id);
                        if this.active == tab_id {
                            this.active = Tab::Main;
                        }
                        this.sync_seen_for_active(cx);
                        this.sync_active_scale(cx);
                        this.sync_inactive_chart_visibility(cx);
                        this.persist_scales(cx);
                        cx.notify();
                    });
                }
            });

        // Кнопка «собрать окна» — справа в полосе вкладок, ТОЛЬКО если у группы есть откреп-окна.
        // Восстанавливает/показывает/возвращает на экран окна чартов, если они свёрнуты/спрятаны/
        // уехали за пределы экранов (они независимы и не ходят за Main).
        let detached_count = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == self.group)
            .count();
        let gather_btn = (detached_count > 0).then(|| {
            let entity = cx.entity();
            div().absolute().right(px(34.0)).top(px(4.0)).child(
                MoonButton::new("chart-gather-windows")
                    .label("▦")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.gather_windows(cx));
                    })
                    .render(),
            )
        });

        // Кнопка настроек раскладки активной вкладки (⚙) — справа в полосе вкладок.
        let settings_btn = {
            let entity = cx.entity();
            div().absolute().right(px(6.0)).top(px(4.0)).child(
                MoonButton::new("chart-layout-settings")
                    .label("⚙")
                    .size(MoonButtonSize::Micro)
                    .variant(if self.layout_popup_open {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(self.layout_popup_open)
                    .on_click(move |_, window, app| {
                        entity.update(app, |this, cx| this.toggle_layout_popup(window, cx));
                    })
                    .render(),
            )
        };

        // Попап раскладки — поверх содержимого (вне overflow_hidden стрипа).
        let popup = self.layout_popup_open.then(|| {
            let entity = cx.entity();
            let current = resolve_mode(self.active_layout_mode(cx), self.backend.read(cx));
            let p = moon_ui::MoonPalette::active(cx);
            div()
                .id("chart-layout-popup-wrap")
                .occlude()
                .absolute()
                .right(px(6.0))
                .top(px(CHART_TAB_STRIP_H + 2.0))
                // Авто-скрытие: закрываем по уходу курсора, но только ПОСЛЕ первого входа
                // (при открытии курсор ещё на кнопке, не на попапе).
                .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                    if *hovered {
                        this.layout_popup_hovered = true;
                    } else if this.layout_popup_hovered {
                        this.layout_popup_open = false;
                        cx.notify();
                    }
                }))
                .child(layout_popup::render_layout_popup(
                    "chart-layout",
                    current,
                    &self.layout_height_input,
                    p,
                    cx,
                    move |mode, app| {
                        entity.update(app, |this, cx| {
                            let h = this.active_layout_height(cx);
                            this.apply_layout(Some(mode), h, cx);
                        });
                    },
                ))
        });

        v_flex()
            .size_full()
            .relative()
            .child(
                div()
                    .h(px(CHART_TAB_STRIP_H))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip)
                    .children(gather_btn)
                    .child(settings_btn),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.active_element()),
            )
            .children(popup)
    }
}

/// Осмысленная подпись AddToChart-графика (П.4 — порт egui): «номер-группа», а далее по
/// `bucket`: своё ядро → «номер-группа-ядро», именованная связка → «номер-группа-связка»,
/// общая → только «номер-группа». Пустая группа → только номер (старый фолбэк). Используется
/// и в стрипе вкладок, и в заголовке/титуле выносного окна.
fn chart_pane_label(
    backend: &Entity<Backend>,
    group: &str,
    n: u32,
    bucket: &ChartBucket,
    cx: &App,
) -> String {
    let mut label = if group.is_empty() {
        n.to_string()
    } else {
        format!("{n}-{group}")
    };
    let suffix = match bucket {
        ChartBucket::Shared => String::new(),
        ChartBucket::Core(cid) => backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .find(|s| s.id == *cid)
            .map(|s| s.name.clone())
            .unwrap_or_default(),
        ChartBucket::Bundle(name) => name.clone(),
    };
    if !suffix.is_empty() {
        label.push('-');
        label.push_str(&suffix);
    }
    label
}
