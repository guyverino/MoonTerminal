//! AddToChart-вкладка: визуально один список графиков, архитектурно — отдельный `ChartPanel`
//! на каждый график. Вынесено из `chart_tabs` как самостоятельная вью-модель; общий рендер
//! стека — в [`super::stack`]. Используется и полоской вкладок, и выносными окнами ([`super::windows`]).

use gpui::*;
use moon_ui::MoonVirtualListScrollHandle;

use super::stack::{
    ChartStackEntry, HIGHLIGHT, render_chart_stack, resolve_layout, set_panels_orderbook_enabled,
    set_panels_scale,
};
use crate::Backend;
use crate::chart_persist::StackLayoutMode;
use crate::panels::ChartPanel;
use moon_core::config::{ChartBucket, ChartTheme};
use moon_core::session::CoreId;

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
    /// Per-tab режим раскладки (Fit/Scroll; None = дефолт Fit).
    layout_mode: Option<StackLayoutMode>,
    /// Высота слота для Fit: 0 = растяжение, ≥20 = compress. None = дефолт.
    layout_height_fit: Option<u16>,
    /// Высота слота для Scroll. None = дефолт.
    layout_height_scroll: Option<u16>,
    /// Показывать ли стакан на графиках вкладки (per-окно). None = дефолт (вкл).
    orderbook_enabled: Option<bool>,
    /// Скролл-хэндл вертикального MoonVirtualList (scroll-режим стека).
    scroll: MoonVirtualListScrollHandle,
}

impl AddChartStack {
    pub(super) fn new(
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
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            scroll: MoonVirtualListScrollHandle::new(),
        }
    }

    pub(super) fn add_coin(
        &mut self,
        core: CoreId,
        market: &str,
        ttl_ms: f64,
        cx: &mut Context<Self>,
    ) {
        // Уже есть такой график → продлить TTL.
        if let Some(entry) = self
            .charts
            .iter()
            .find(|e| e.core == core && e.market == market)
        {
            entry
                .panel
                .update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
            cx.notify();
            return;
        }

        // Новый график — в конец (запиненные сами всплывут наверх при сортировке в render).
        let backend = self.backend.clone();
        let num = self.num;
        let bucket = self.bucket.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let scale = self.scale;
        let panel = cx.new(|cx| ChartPanel::new_addto(backend, num, bucket, epoch, theme, cx));
        // Любое изменение панели (вкл. переключение пина ●/○) → перерисовать стек: prune пустых +
        // пере-сортировка запиненных наверх происходит в render.
        cx.observe(&panel, |this, _, cx| {
            this.prune_empty(cx);
            cx.notify();
        })
        .detach();
        if scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(scale, pcx));
        }
        if let Some(en) = self.orderbook_enabled {
            panel.update(cx, |panel, pcx| panel.set_orderbook_enabled(en, pcx));
        }
        panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
        self.charts
            .push(ChartStackEntry::new(core, market.to_string(), panel));
        cx.notify();
    }

    /// Снятие выбывших графиков (TTL истёк → пустая панель). Запиненные панели TTL не трогает
    /// (`prune_ttl` пропускает pinned), так что наверху они держатся. Удаляем пустые сразу —
    /// стабильность позиции обеспечивает пин (см. сортировку в render), а не задержка.
    fn prune_empty(&mut self, cx: &App) -> bool {
        let before = self.charts.len();
        self.charts.retain(|e| e.panel.read(cx).pane_count() > 0);
        self.charts.len() != before
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

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Вкл/выкл стакан для всех графиков стека (per-окно).
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        if self.orderbook_enabled == enabled {
            return;
        }
        self.orderbook_enabled = enabled;
        set_panels_orderbook_enabled(&self.charts, enabled.unwrap_or(true), cx);
        cx.notify();
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height_fit(&self) -> Option<u16> {
        self.layout_height_fit
    }

    pub(crate) fn layout_height_scroll(&self) -> Option<u16> {
        self.layout_height_scroll
    }

    /// Применить per-tab раскладку (режим + раздельные высоты Fit/Scroll) к этому стеку.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode
            && self.layout_height_fit == height_fit
            && self.layout_height_scroll == height_scroll
        {
            return;
        }
        self.layout_mode = mode;
        self.layout_height_fit = height_fit;
        self.layout_height_scroll = height_scroll;
        cx.notify();
    }

    pub(super) fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
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
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        // Запиненные графики — наверх кластером (стабильная сортировка: порядок внутри групп
        // сохраняется). Пин защищает от TTL (prune_ttl), так что они держатся вверху.
        self.charts
            .sort_by_key(|e| !e.panel.read(cx).is_pinned());
        let count = self.charts.len();
        let border = rgb(palette.border);
        let accent = rgb(palette.blue);
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
            move |s, ix, panel, height, flex, border, _ent| {
                let (id, fresh) = match s.charts.get(ix) {
                    Some(e) => (
                        format!("add-chart-stack-tile-{}-{}-{}", s.num, e.core, e.market),
                        e.arrived_at.elapsed() < HIGHLIGHT,
                    ),
                    None => (format!("add-chart-stack-tile-{}-{ix}", s.num), false),
                };
                let mut tile = div()
                    .id(SharedString::from(id.clone()))
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
                // Подсветка только что появившегося графика: яркая акцентная рамка поверх, пульс
                // (3 мигания за HIGHLIGHT). Сдвинута внутрь на 1px, чтобы overflow_hidden её не
                // срезал; opacity не падает в 0 на пике, чтобы было хорошо видно. gpui гонит кадры.
                let highlight = fresh.then(|| {
                    div()
                        .absolute()
                        .top(px(1.0))
                        .left(px(1.0))
                        .right(px(1.0))
                        .bottom(px(1.0))
                        .border_2()
                        .border_color(accent)
                        .rounded(px(2.0))
                        .with_animation(
                            SharedString::from(format!("{id}-arrive")),
                            Animation::new(HIGHLIGHT),
                            |el, delta| {
                                // 3 чётких мигания: 0 → 1 → 0, повторённые.
                                let pulse = (delta * std::f32::consts::PI * 3.0).sin().abs();
                                el.opacity(pulse)
                            },
                        )
                });
                tile.child(div().size_full().relative().overflow_hidden().child(panel))
                    .children(highlight)
                    .into_any_element()
            },
        )
    }
}
