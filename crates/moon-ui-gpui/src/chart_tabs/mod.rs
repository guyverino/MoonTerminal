//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.
//!
//! Подсистема выносных ОС-окон откреп-вкладок (detach/restore/repin/персист + хост
//! `DetachedChartHost`) — в [`windows`].

mod add_stack;
mod layout_popup;
mod layout_popup_window;
mod main_stack;
mod sig;
mod stack;
mod strip;
mod windows;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use rust_i18n::t;

pub(crate) use add_stack::AddChartStack;
pub(crate) use main_stack::MainChartStack;
use sig::{chart_tabs_sig, core_belongs_to_group};

use crate::chart_persist::StackLayoutMode;

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Panel, PanelEvent, PanelState, Root};

use crate::Backend;
use crate::chart_persist;
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
    /// Окно-поповер настроек раскладки активной вкладки (кнопка ⚙). Отдельное безрамочное ОС-окно
    /// (см. [`layout_popup_window`]) — иначе оси own-pass просвечивают сквозь in-scene попап.
    /// Some = открыто; закрытие по потере фокуса само сбросит в None (`on_layout_popup_closed`).
    layout_popup: Option<WindowHandle<Root>>,
    /// Оконный (логич. px) rect кнопки ⚙ — снимается canvas-пробой в paint, читается при открытии
    /// попапа, чтобы привязать правый край окна к правому краю кнопки. `Cell` — писать из paint
    /// без borrow самого view.
    settings_btn_rect: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Время (unix ms) последнего авто-закрытия попапа по потере фокуса. Клик по ⚙ при открытом
    /// окне сперва уводит фокус (окно закрывается, handle чистится), и тогда тот же клик открыл бы
    /// его заново → флаг. Если с момента закрытия прошло <~250мс, повторное открытие гасим.
    layout_popup_closed_ms: f64,
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
        let (main_scale, main_layout, main_orderbook, restore_pending): (
            Option<f32>,
            (Option<StackLayoutMode>, Option<u16>, Option<u16>),
            Option<bool>,
            Vec<_>,
        ) = {
            let specs = &backend.read(cx).chart_specs;
            let main_spec = specs.iter().find(|s| s.group == group && s.num == 0);
            let main_scale = main_spec.and_then(|s| s.scale);
            let main_layout = main_spec.map_or((None, None, None), |s| {
                (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
            });
            let main_orderbook = main_spec.and_then(|s| s.orderbook_enabled);
            let pending = specs
                .iter()
                .filter(|s| s.group == group && s.num >= 1 && s.detached.is_some())
                .map(|s| (s.num, s.bucket(), s.detached.unwrap(), s.scale))
                .collect();
            (main_scale, main_layout, main_orderbook, pending)
        };
        if main_scale.is_some() {
            main.update(cx, |p, pcx| p.set_scale(main_scale, pcx));
        }
        if main_layout.0.is_some() || main_layout.1.is_some() || main_layout.2.is_some() {
            main.update(cx, |p, pcx| {
                p.set_layout(main_layout.0, main_layout.1, main_layout.2, pcx)
            });
        }
        if main_orderbook.is_some() {
            main.update(cx, |p, pcx| p.set_orderbook_enabled(main_orderbook, pcx));
        }
        cx.observe(&backend, |this, backend, cx| {
            // Запросы «применить ко всем» из выносных окон — до early-return по sig (они sig не меняют).
            this.drain_apply_all(cx);
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
            layout_popup: None,
            settings_btn_rect: Rc::new(Cell::new(None)),
            layout_popup_closed_ms: 0.0,
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

    /// Открыть/закрыть попап настроек раскладки. При открытии заполняет оба поля высоты
    /// (Fit/Scroll) текущими значениями активной вкладки (нужен `window` для `set_value`).
    /// Открыть/закрыть окно-поповер настроек раскладки активной вкладки. Открытие — отдельным
    /// безрамочным ОС-окном у кнопки ⚙ (см. [`layout_popup_window`]); повторный клик закрывает.
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.layout_popup.take() {
            let _ = handle.update(cx, |_, w, _| w.remove_window());
            cx.notify();
            return;
        }
        // Клик по ⚙ только что закрыл окно по blur → не открывать заново (см. поле).
        if moon_chart::paint::now_unix_ms() - self.layout_popup_closed_ms < 250.0 {
            return;
        }
        let mode = self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit);
        let hf = self.active_layout_height_fit(cx);
        let hs = self.active_layout_height_scroll(cx);
        let win_size = layout_popup_window::content_size(cx);
        let btn = self
            .settings_btn_rect
            .get()
            .unwrap_or(Bounds {
                origin: point(window.viewport_size().width - px(40.0), px(4.0)),
                size: size(px(22.0), px(22.0)),
            });
        // Правый край попапа = правый край кнопки ⚙, вплотную под ней (~2px).
        let (origin, display_id, place_phys) = crate::windowing::popup_placement(
            window,
            cx,
            win_size,
            btn,
            crate::windowing::PopupGrowX::Left,
            2.0,
        );
        let clear = layout_popup_window::clear_color(cx);
        // Из попапа Main — «ко всем окнам» (incl. Main); из чарт-вкладки — «ко всем чартам».
        let include_main = matches!(self.active, Tab::Main);
        let apply_all_label = if include_main {
            t!("chart.layout.apply_all_windows").to_string()
        } else {
            t!("chart.layout.apply_all_charts").to_string()
        };
        let owner = cx.entity();
        let apply: layout_popup_window::ApplyFn = Rc::new(move |m, hf, hs, app| {
            owner.update(app, |o, oc| o.apply_layout(Some(m), hf, hs, oc));
        });
        let owner_all = cx.entity();
        let apply_all: layout_popup_window::ApplyFn = Rc::new(move |m, hf, hs, app| {
            owner_all.update(app, |o, oc| {
                o.apply_layout_to_all(include_main, Some(m), hf, hs, oc)
            });
        });
        let owner2 = cx.entity();
        let closed: layout_popup_window::ClosedFn = Rc::new(move |app| {
            owner2.update(app, |o, oc| o.on_layout_popup_closed(oc));
        });
        let orderbook_enabled = self.active_orderbook_enabled(cx);
        let owner_ob = cx.entity();
        let on_toggle_orderbook: layout_popup_window::OrderbookFn = Rc::new(move |enabled, app| {
            owner_ob.update(app, |o, oc| o.apply_orderbook(enabled, oc));
        });
        self.layout_popup = layout_popup_window::open(
            origin,
            win_size,
            display_id,
            place_phys,
            clear,
            mode,
            hf,
            hs,
            orderbook_enabled,
            apply,
            apply_all,
            apply_all_label.into(),
            on_toggle_orderbook,
            closed,
            cx,
        );
        // Сразу после открытия (синхронно, до первого present) доставляем окно на верное место по
        // абсолютным screen-px — иначе виден прыжок (open кладёт его багнутым SetWindowPlacement).
        if let (Some(h), Some(phys)) = (self.layout_popup, place_phys) {
            let _ = h.update(cx, |_, w, _| crate::windowing::move_window_to_physical(w, phys));
        }
        cx.notify();
    }

    /// Окно-поповер закрылось (по потере фокуса) — забыть handle, перерисовать кнопку ⚙.
    fn on_layout_popup_closed(&mut self, cx: &mut Context<Self>) {
        self.layout_popup_closed_ms = moon_chart::paint::now_unix_ms();
        if self.layout_popup.take().is_some() {
            cx.notify();
        }
    }

    /// Ключ персиста активной вкладки: Main → (0, Shared); AddToChart → (num, bucket).
    fn active_stack_key(&self) -> (u32, ChartBucket) {
        match &self.active {
            Tab::Main => (0, ChartBucket::Shared),
            Tab::Add(n, b) => (*n, b.clone()),
        }
    }

    /// Per-tab режим раскладки активной вкладки (None = дефолт Fit).
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

    /// Per-tab высота Fit активной вкладки.
    fn active_layout_height_fit(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height_fit(),
            Tab::Add(n, b) => self
                .add
                .iter()
                .find(|(num, bk, _)| num == n && bk == b)
                .and_then(|(_, _, p)| p.read(cx).layout_height_fit()),
        }
    }

    /// Per-tab высота Scroll активной вкладки.
    fn active_layout_height_scroll(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height_scroll(),
            Tab::Add(n, b) => self
                .add
                .iter()
                .find(|(num, bk, _)| num == n && bk == b)
                .and_then(|(_, _, p)| p.read(cx).layout_height_scroll()),
        }
    }

    /// Стакан включён на активной вкладке (None → дефолт вкл).
    fn active_orderbook_enabled(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).orderbook_enabled(),
            Tab::Add(n, b) => self
                .add
                .iter()
                .find(|(num, bk, _)| num == n && bk == b)
                .and_then(|(_, _, p)| p.read(cx).orderbook_enabled()),
        };
        v.unwrap_or(true)
    }

    /// Вкл/выкл стакан на АКТИВНОЙ вкладке + persist.
    fn apply_orderbook(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_orderbook_enabled(Some(enabled), c)),
            Tab::Add(n, b) => {
                if let Some((_, _, p)) = self.add.iter().find(|(num, bk, _)| *num == n && *bk == b) {
                    p.update(cx, |s, c| s.set_orderbook_enabled(Some(enabled), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.orderbook_enabled = Some(enabled);
        });
        // Stage 2: пересобрать набор рынков, которым нужен стакан (мог измениться спрос).
        self.backend
            .update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    /// Применить раскладку (режим + раздельные высоты Fit/Scroll) к АКТИВНОЙ вкладке и
    /// сохранить в charts.json.
    fn apply_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_layout(mode, height_fit, height_scroll, c)),
            Tab::Add(n, b) => {
                if let Some((_, _, p)) = self.add.iter().find(|(num, bk, _)| *num == n && *bk == b) {
                    p.update(cx, |s, c| s.set_layout(mode, height_fit, height_scroll, c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.layout_mode = mode;
            s.layout_height_fit = height_fit;
            s.layout_height_scroll = height_scroll;
        });
        cx.notify();
    }

    /// Применить раскладку ко ВСЕМ стекам группы. `include_main`: трогать ли Main (true — из попапа
    /// Main → ко всем окнам; false — из чартов → Main не трогаем). Персист каждой вкладки.
    fn apply_layout_to_all(
        &mut self,
        include_main: bool,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if include_main {
            self.main
                .update(cx, |s, c| s.set_layout(mode, height_fit, height_scroll, c));
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
            });
        }
        // «Чарты» = add-вкладки в стрипе + откреплённые в окна (их стеки держим в self.detached).
        let targets: Vec<(u32, ChartBucket, Entity<AddChartStack>)> = self
            .add
            .iter()
            .chain(self.detached.iter())
            .map(|(n, b, p)| (*n, b.clone(), p.clone()))
            .collect();
        for (num, bucket, panel) in targets {
            panel.update(cx, |s, c| s.set_layout(mode, height_fit, height_scroll, c));
            self.upsert_spec(cx, num, &bucket, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
            });
        }
        cx.notify();
    }

    /// Дренаж запросов «применить ко всем» из выносных окон чартов ЭТОЙ группы (у них нет доступа
    /// к стекам группы, поэтому шлют через Backend).
    fn drain_apply_all(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let reqs: Vec<crate::ChartApplyAll> = self.backend.update(cx, |b, _| {
            let (mine, rest): (Vec<_>, Vec<_>) = b
                .chart_apply_all
                .drain(..)
                .partition(|r| r.group == group);
            b.chart_apply_all = rest;
            mine
        });
        for r in reqs {
            self.apply_layout_to_all(r.include_main, r.mode, r.height_fit, r.height_scroll, cx);
        }
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
                let (saved_scale, saved_layout, saved_orderbook) = {
                    let specs = &self.backend.read(cx).chart_specs;
                    let spec = specs
                        .iter()
                        .find(|s| s.group == self.group && s.num == n && s.bucket() == bucket);
                    (
                        spec.and_then(|s| s.scale),
                        spec.map_or((None, None, None), |s| {
                            (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
                        }),
                        spec.and_then(|s| s.orderbook_enabled),
                    )
                };
                if saved_scale.is_some() {
                    panel.update(cx, |p, pcx| p.set_scale(saved_scale, pcx));
                }
                if saved_layout.0.is_some() || saved_layout.1.is_some() || saved_layout.2.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_layout(saved_layout.0, saved_layout.1, saved_layout.2, pcx)
                    });
                }
                if saved_orderbook.is_some() {
                    panel.update(cx, |p, pcx| p.set_orderbook_enabled(saved_orderbook, pcx));
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
