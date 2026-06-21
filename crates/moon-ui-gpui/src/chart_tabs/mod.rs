//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.
//!
//! Подсистема выносных ОС-окон откреп-вкладок (detach/restore/repin/персист + хост
//! `DetachedChartHost`) — в [`windows`].

mod windows;

use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonRect,
    MoonScrollbarVisibility, MoonTabItem, MoonTabStrip, MoonVirtualList,
    MoonVirtualListScrollHandle, Panel, PanelEvent, PanelState, v_flex,
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
    /// Main-чарт (вкладка Main).
    main: Entity<ChartPanel>,
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
    focus: FocusHandle,
}

struct AddChartEntry {
    core: CoreId,
    market: String,
    panel: Entity<ChartPanel>,
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
    charts: Vec<AddChartEntry>,
    scale: Option<f32>,
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
        if scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(scale, pcx));
        }
        panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
        self.charts.push(AddChartEntry {
            core,
            market: market.to_string(),
            panel,
        });
        cx.notify();
    }

    fn prune_empty(&mut self, cx: &App) {
        self.charts
            .retain(|entry| entry.panel.read(cx).pane_count() > 0);
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
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, pcx| panel.set_scale(pct, pcx));
        }
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
        self.prune_empty(cx);
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

        // Режим раскладки стека (Настройки):
        //  • scroll=false               → FIT: графики делят высоту окна (масштаб по вертикали);
        //  • scroll=true, compress=false → SCROLL: каждый график фикс. высоты, MoonVirtualList
        //    со скроллбаром MoonUI;
        //  • scroll=true, compress=true  → COMPRESS: фикс. высоты, без скролла — при
        //    переполнении окна графики сжимаются (как в FIT).
        // ВАЖНО: чарт-слоты ПРОЗРАЧНЫЕ. own-pass (combo/стакан) — слой GpuCanvasLayer::UnderScene
        // (под сценой); любой непрозрачный `.bg()` над слотом его перекрывает. Разделитель — рамка.
        let (scroll, compress, stack_h) = {
            let b = self.backend.read(cx);
            (
                b.config.charts_stack_scroll,
                b.config.charts_stack_compress,
                b.config.chart_stack_height.clamp(120, 2000) as f32,
            )
        };
        let num = self.num;

        if scroll && !compress {
            // SCROLL: MoonUI-скроллбар + виртуальный список фикс. высоты stack_h. Рендер плитки —
            // через weak-entity (как лог-панель), т.к. render-фабрика MoonVirtualList отдаёт App.
            let weak = cx.entity().downgrade();
            let count = self.charts.len();
            let border = palette.border;
            let list = MoonVirtualList::new(
                format!("add-chart-stack-vlist-{num:?}"),
                count,
                stack_h,
                move |ix, _window, app| {
                    let Some(entity) = weak.upgrade() else {
                        return div().into_any_element();
                    };
                    let Some(panel) = entity.read(app).charts.get(ix).map(|e| e.panel.clone())
                    else {
                        return div().into_any_element();
                    };
                    div()
                        .id(("add-chart-stack-tile", ix))
                        .w_full()
                        .h(px(stack_h))
                        .relative()
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(border))
                        .child(div().size_full().relative().overflow_hidden().child(panel))
                        .into_any_element()
                },
            )
            .track_scroll(&self.scroll)
            .surface(false)
            .border(false)
            .radius(0.0)
            .scrollbar_visibility(MoonScrollbarVisibility::Scrolling);
            return div()
                .id(format!("add-chart-stack-scroll-{num:?}"))
                .size_full()
                .child(list)
                .into_any_element();
        }

        // FIT / COMPRESS: без скролла, v_flex на всю высоту окна.
        let compress_mode = compress;
        let tiles = self.charts.iter().map(|entry| {
            let tile = div()
                .id(format!(
                    "add-chart-stack-{}-{}-{}",
                    num, entry.core, entry.market
                ))
                .w_full()
                .relative()
                .overflow_hidden()
                .border_1()
                .border_color(rgb(palette.border));
            let tile = if compress_mode {
                // COMPRESS: целевая высота, но даём сжиматься при переполнении (min_h=0).
                tile.h(px(stack_h)).min_h(px(0.0))
            } else {
                // FIT: делим высоту окна поровну.
                tile.flex_1().min_h(px(0.0))
            };
            tile.child(
                div()
                    .size_full()
                    .relative()
                    .overflow_hidden()
                    .child(entry.panel.clone()),
            )
        });
        div()
            .id(format!("add-chart-stack-fit-{num:?}"))
            .size_full()
            .overflow_hidden()
            .child(v_flex().size_full().children(tiles))
            .into_any_element()
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
            ChartPanel::new(
                backend.clone(),
                focus_open,
                epoch,
                theme.clone(),
                window,
                cx,
            )
        });
        let initial_sig = chart_tabs_sig(backend.read(cx), &group);
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            let main_handle = main.read(cx).debug_data_handle();
            backend.update(cx, |b, _| {
                b.register_debug_main_chart(group.clone(), main_handle);
            });
        }
        // Из charts.json: масштаб Main (num=0) и список откреп-вкладок этой группы на
        // восстановление (создадим пустыми на первом render → ждут детект).
        let (main_scale, restore_pending): (Option<f32>, Vec<_>) = {
            let specs = &backend.read(cx).chart_specs;
            let main_scale = specs
                .iter()
                .find(|s| s.group == group && s.num == 0)
                .and_then(|s| s.scale);
            let pending = specs
                .iter()
                .filter(|s| s.group == group && s.num >= 1 && s.detached.is_some())
                .map(|s| (s.num, s.bucket(), s.detached.unwrap(), s.scale))
                .collect();
            (main_scale, pending)
        };
        if main_scale.is_some() {
            main.update(cx, |p, pcx| p.set_scale(main_scale, pcx));
        }
        cx.observe(&backend, |this, backend, cx| {
            let sig = chart_tabs_sig(backend.read(cx), &this.group);
            if sig != this.last_sig {
                this.last_sig = sig;
                cx.notify();
            }
        })
        .detach();
        Self {
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
            focus: cx.focus_handle(),
        }
    }

    fn handle_open_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                .update(cx, |p, pcx| p.open_market(core, market, pcx));
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            // П.1: поднимаем/фокусируем окно Main ТОЛЬКО для дабл-клика по чарту
            // (open_request_activate). Клики в Ордерах/Детектах открывают монету, но окно
            // не активируют — иначе любой клик дёргал бы окно на передний план.
            if activate {
                window.activate_window();
            }
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

    /// Ингест AddToChart-детектов (add_to_chart>0) → создать/наполнить вкладку.
    /// Ключ вкладки — `ChartBucket` ядра (своё ядро / общая / именованная связка),
    /// резолвится из конфига ядра + глоб. `charts_split_by_core`.
    /// БЕЗ авто-перехода: active не трогаем (порт «не уводить на чарт при детекте»).
    fn ingest(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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
            backend.update(cx, |b, _| {
                if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                    b.desired.push((core, market.clone()));
                }
            });
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
                // Восстановить сохранённый масштаб этой вкладки (charts.json), если был.
                let saved_scale = self
                    .backend
                    .read(cx)
                    .chart_specs
                    .iter()
                    .find(|s| s.group == self.group && s.num == n && s.bucket() == bucket)
                    .and_then(|s| s.scale);
                if saved_scale.is_some() {
                    panel.update(cx, |p, pcx| p.set_scale(saved_scale, pcx));
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
        if !matches!(active, Tab::Main) {
            self.main
                .update(cx, |panel, _| panel.set_scene_visible(false));
        }
        for (n, c, panel) in &self.add {
            if Tab::Add(*n, c.clone()) != active {
                panel.update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
            }
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
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        self.drain_debug_fill_main_chart(cx);
        self.handle_open_request(window, cx);
        self.ingest(window, cx);
        self.sync_inactive_chart_visibility(cx);
        // Откреп-вкладки: вернуть закрытые в стрип (репин) + восстановить сохранённые окна
        // (charts.json) на первом render — пустыми, ждут детект.
        self.drain_chart_repin(cx);
        self.restore_detached(cx);
        // Бейджи = непрочитанные С МОМЕНТА УХОДА: на АКТИВНОЙ вкладке seen догоняет pane_count
        // (бейджа нет — ты смотришь). Ушёл → seen заморожен → новые монеты растят бейдж только
        // этой вкладки (а не всех открытых). Прибраться от закрытых вкладок: чистим seen.
        if let Tab::Add(n, c) = self.active.clone() {
            if let Some((_, _, panel)) = self.add.iter().find(|(num, cc, _)| *num == n && *cc == c)
            {
                let cnt = panel.read(cx).pane_count(cx);
                self.seen.insert((n, c), cnt);
            }
        }
        // Масштаб ПО-ВКЛАДОЧНЫЙ: тулбар окна правит масштаб АКТИВНОЙ вкладки. rev вырос (юзер
        // выбрал) → применяем к активной панели; иначе синхроним backend.price_scale = масштаб
        // активной панели (чтобы тулбар показывал масштаб именно её).
        {
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
        // Сохранить масштаб каждой вкладки в charts.json (upsert при изменении).
        self.persist_scales(cx);

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
            div().absolute().right(px(6.0)).top(px(4.0)).child(
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

        v_flex()
            .size_full()
            .child(
                div()
                    .h(px(CHART_TAB_STRIP_H))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip)
                    .children(gather_btn),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.active_element()),
            )
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
