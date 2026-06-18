//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonPalette, MoonRect, MoonTabItem, MoonTabStrip, Panel, PanelEvent,
    PanelState, Root, h_flex, v_flex,
};

use crate::Backend;
use crate::chart_persist;
use crate::design;
use crate::panels::ChartPanel;
use moon_core::config::ChartTheme;
use moon_core::session::CoreId;

/// Высота полоски чарт-вкладок (px). Табы в MoonTabStrip — h=28 + подчёркивание; 30 даёт
/// ровный ряд. Резервируется в layout сверху, и в неё же кладутся bounds стрипа.
const CHART_TAB_STRIP_H: f32 = 30.0;

/// Идентичность вкладки чарта. Main — фуллскрин; Add(номер, ядро) — AddToChart-вкладка
/// (ядро задано при `charts_split_by_core`, иначе None — общая на номер). Порт egui
/// `ContainerKind` (Main / Chart{num, core}).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Main,
    Add(u32, Option<CoreId>),
}

pub struct ChartTabs {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    /// Main-чарт (вкладка Main).
    main: Entity<ChartPanel>,
    /// AddToChart-вкладки (номер, ядро, панель), отсортированы по (номер, ядро).
    add: Vec<(u32, Option<CoreId>, Entity<ChartPanel>)>,
    /// Откреплённые в своё ОС-окно вкладки — держим Entity, чтобы при закрытии окна
    /// вернуть панель в стрип (repin) и чтобы новые детекты этого номера шли в неё.
    detached: Vec<(u32, Option<CoreId>, Entity<ChartPanel>)>,
    /// Активная вкладка.
    active: Tab,
    /// Сколько монет на вкладке (num, core) пользователь уже «видел» (был на ней активен).
    /// Бейдж = pane_count - seen (новые с момента ухода). На активной вкладке seen догоняет
    /// pane_count → бейджа нет. Уходишь → seen заморожен → новые детекты растят бейдж.
    seen: HashMap<(u32, Option<CoreId>), usize>,
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
    restore_pending: Vec<(u32, Option<CoreId>, chart_persist::WinGeom, Option<f32>)>,
    focus: FocusHandle,
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
                .map(|s| (s.num, s.core, s.detached.unwrap(), s.scale))
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

    /// Дабл-клик по чарту AddToChart-вкладки → открыть монету на Main + переключиться.
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
                b.open_request.take()
            } else {
                None
            }
        });
        if let Some((core, market)) = req {
            self.main
                .update(cx, |p, pcx| p.open_market(core, market, pcx));
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
        }
    }

    /// Ингест AddToChart-детектов (add_to_chart>0) → создать/наполнить вкладку.
    /// Ключ вкладки — (номер, ядро) при `charts_split_by_core`, иначе (номер, None).
    /// БЕЗ авто-перехода: active не трогаем (порт «не уводить на чарт при детекте»).
    fn ingest(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (split, fresh, cursors): (bool, Vec<(u32, CoreId, String, f64)>, Vec<(CoreId, u64)>) = {
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
                let last = self.add_seq.get(&id).copied().unwrap_or(0);
                let mut mx = last;
                for det in &d.detects {
                    if det.seq <= last {
                        continue;
                    }
                    mx = mx.max(det.seq);
                    if det.add_to_chart > 0 {
                        let ttl = (det.keep_in_chart_secs.max(1) as f64) * 1000.0;
                        fresh.push((det.add_to_chart, id, det.market.clone(), ttl));
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
        for (n, core, market, ttl) in fresh {
            let key_core = if split { Some(core) } else { None };
            backend.update(cx, |b, _| {
                if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                    b.desired.push((core, market.clone()));
                }
            });
            let in_detached = self
                .detached
                .iter()
                .any(|(num, c, _)| *num == n && *c == key_core);
            if let Some((_, _, tab)) = self
                .add
                .iter()
                .find(|(num, c, _)| *num == n && *c == key_core)
                .or_else(|| {
                    self.detached
                        .iter()
                        .find(|(num, c, _)| *num == n && *c == key_core)
                })
            {
                if in_detached {
                    moon_core::detect_diag::line(&format!(
                        "[ingest] +coin n={n} core={key_core:?} market={market} → DETACHED-окно"
                    ));
                }
                tab.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
            } else {
                let panel = cx.new(|cx| {
                    ChartPanel::new_addto(backend.clone(), n, key_core, epoch, theme.clone(), cx)
                });
                // Восстановить сохранённый масштаб этой вкладки (charts.json), если был.
                let saved_scale = self
                    .backend
                    .read(cx)
                    .chart_specs
                    .iter()
                    .find(|s| s.group == self.group && s.num == n && s.core == key_core)
                    .and_then(|s| s.scale);
                if saved_scale.is_some() {
                    panel.update(cx, |p, pcx| p.set_scale(saved_scale, pcx));
                }
                panel.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
                self.add.push((n, key_core, panel));
                // Порядок вкладок: по (номер, ядро) — как egui sort_by_key.
                self.add.sort_by_key(|(num, c, _)| (*num, c.unwrap_or(0)));
                moon_core::detect_diag::line(&format!(
                    "[ingest] NEW tab n={n} core={key_core:?} (total_tabs={})",
                    self.add.len()
                ));
                // active НЕ меняем — не уводим пользователя на новую вкладку.
            }
        }
    }

    /// Отцепить AddToChart-вкладку в отдельное ОС-окно (убрать из стрипа).
    fn detach(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Tab::Add(n, core) = tab else { return };
        let Some(pos) = self
            .add
            .iter()
            .position(|(num, c, _)| *num == n && *c == core)
        else {
            return;
        };
        let (_, _, panel) = self.add.remove(pos);
        if self.active == tab {
            self.active = Tab::Main;
        }
        // Геометрия: сохранённая (если уже откреплялась) или дефолт-каскад.
        let geom = self
            .spec_geom(cx, n, core)
            .unwrap_or(chart_persist::WinGeom {
                x: 200,
                y: 160,
                w: 900,
                h: 620,
            });
        // Пометить вкладку откреплённой в charts.json (восстановится окном на след. запуске).
        self.upsert_spec(cx, n, core, |s| s.detached = Some(geom));
        moon_core::detect_diag::line(&format!(
            "[detach] n={n} core={core:?} → detached=Some({},{},{},{})",
            geom.x, geom.y, geom.w, geom.h
        ));
        self.open_chart_window(n, core, panel, geom, false, cx);
        cx.notify();
    }

    /// Открыть ОС-окно откреп-вкладки (общий код detach и восстановления при загрузке). Панель
    /// держим в `detached` (ingest наполняет её по num/core); `gpu_canvas` переезжает вместе
    /// с GPUI scene окна.
    /// Хост (`DetachedChartHost`) сам пишет геометрию и просит репин по закрытию. Окно трекаем
    /// по группе (закрытие окна группы закроет его — main.rs on_window_closed).
    fn open_chart_window(
        &mut self,
        n: u32,
        core: Option<CoreId>,
        panel: Entity<ChartPanel>,
        geom: chart_persist::WinGeom,
        restored: bool,
        cx: &mut Context<Self>,
    ) {
        self.detached.push((n, core, panel.clone()));
        panel.update(cx, |p, _| p.set_scene_visible(false));
        // КРИТИЧНО для мультимонитора: без display_id окно создаётся на PRIMARY, и если
        // сохранённые bounds вне primary — gpui откатывается на default_bounds() (центр + дефолт-
        // размер). Поэтому ищем монитор, СОДЕРЖАЩИЙ сохранённую точку, и передаём его display_id —
        // тогда bounds валидны для него и окно встаёт точно (см. retrieve_window_placement).
        let origin = point(px(geom.x as f32), px(geom.y as f32));
        let display_id = cx
            .displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id());
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(geom.w as f32), px(geom.h as f32)),
            })),
            display_id,
            titlebar: Some(TitlebarOptions {
                title: Some(format!("MoonTerminal — Чарт {n}").into()),
                ..Default::default()
            }),
            window_decorations: design::platform_window_decorations(),
            ..Default::default()
        };
        let backend = self.backend.clone();
        let group = self.group.clone();
        // Для восстановленного окна — сохранённый логический размер, чтобы скорректировать
        // DPICHANGED-сжатие на первом render (см. DetachedChartHost.restore_size).
        let restore_size = restored.then(|| size(px(geom.w as f32), px(geom.h as f32)));
        let opened = cx.open_window(opts, move |window, cx| {
            let host = cx.new(|cx| {
                DetachedChartHost::new(
                    panel,
                    backend,
                    group,
                    n,
                    core,
                    restored,
                    restore_size,
                    window,
                    cx,
                )
            });
            cx.new(|cx| Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
        });
        if let Ok(handle) = opened {
            let group = self.group.clone();
            self.backend.update(cx, |b, _| {
                b.detached_chart_windows.push((group, handle));
            });
        }
    }

    /// Геометрия сохранённого откреп-окна вкладки (если есть в charts.json).
    fn spec_geom(
        &self,
        cx: &App,
        num: u32,
        core: Option<CoreId>,
    ) -> Option<chart_persist::WinGeom> {
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .find(|s| s.group == self.group && s.num == num && s.core == core)
            .and_then(|s| s.detached)
    }

    /// Найти/создать спеку вкладки (group/num/core), применить мутатор, пометить dirty.
    fn upsert_spec(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        core: Option<CoreId>,
        f: impl FnOnce(&mut chart_persist::ChartTabSpec),
    ) {
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.core == core)
            {
                f(s);
            } else {
                let mut s = chart_persist::ChartTabSpec {
                    group,
                    num,
                    core,
                    scale: None,
                    detached: None,
                };
                f(&mut s);
                b.chart_specs.push(s);
            }
            b.chart_specs_dirty = true;
        });
    }

    /// Дренаж репина откреп-вкладок: хост закрыли (пользователь) → панель detached→add, спека
    /// → НЕ откреплена. Зовётся из render. (На выходе приложения запрос не обработается → спека
    /// остаётся откреплённой → окно восстановится на след. запуске — как у detached.rs.)
    fn drain_chart_repin(&mut self, cx: &mut Context<Self>) {
        // На выходе из приложения НЕ репиним: закрытие откреп-окон при quit не должно сбрасывать
        // detached (иначе окна не восстановятся). Финальный сейв уже сделан в on_app_quit.
        if self.backend.read(cx).quitting {
            return;
        }
        let group = self.group.clone();
        let reqs: Vec<(u32, Option<CoreId>)> = self.backend.update(cx, |b, _| {
            let mut out = Vec::new();
            b.chart_repin_request.retain(|(g, n, c)| {
                if *g == group {
                    out.push((*n, *c));
                    false
                } else {
                    true
                }
            });
            out
        });
        for (n, core) in reqs {
            if let Some(p) = self
                .detached
                .iter()
                .position(|(num, c, _)| *num == n && *c == core)
            {
                let (num, c, pnl) = self.detached.remove(p);
                self.add.push((num, c, pnl));
                self.add.sort_by_key(|(num, c, _)| (*num, c.unwrap_or(0)));
            }
            self.upsert_spec(cx, n, core, |s| s.detached = None);
            moon_core::detect_diag::line(&format!(
                "[repin] n={n} core={core:?} → detached=None (окно закрыли/репин)"
            ));
            cx.notify();
        }
    }

    /// Сохранить масштаб каждой вкладки в charts.json (upsert при изменении). Main = num 0.
    fn persist_scales(&self, cx: &mut Context<Self>) {
        let mut items: Vec<(u32, Option<CoreId>, Option<f32>)> =
            vec![(0, None, self.main.read(cx).scale())];
        for (n, c, p) in &self.add {
            items.push((*n, *c, p.read(cx).scale()));
        }
        for (n, c, p) in &self.detached {
            items.push((*n, *c, p.read(cx).scale()));
        }
        for (num, core, scale) in items {
            let (cur, exists) = {
                let specs = &self.backend.read(cx).chart_specs;
                let found = specs
                    .iter()
                    .find(|s| s.group == self.group && s.num == num && s.core == core);
                (found.and_then(|s| s.scale), found.is_some())
            };
            if cur != scale && (scale.is_some() || exists) {
                self.upsert_spec(cx, num, core, move |s| s.scale = scale);
            }
        }
    }

    /// Восстановить отложенные откреп-окна (charts.json). Открывать ОС-окна В render НЕЛЬЗЯ
    /// (рушит element-арену gpui: «ArenaRef after Arena was cleared»). Откладываем через
    /// `cx.defer` — закрытие выполнится ПОСЛЕ цикла рендера, когда открытие окон безопасно.
    fn restore_detached(&mut self, cx: &mut Context<Self>) {
        if self.restore_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.restore_pending);
        let this = cx.entity();
        cx.defer(move |app| {
            this.update(app, |this, cx| {
                let (epoch, theme) = (this.epoch, this.theme.clone());
                for (n, core, geom, scale) in pending {
                    let backend = this.backend.clone();
                    let panel = cx
                        .new(|c| ChartPanel::new_addto(backend, n, core, epoch, theme.clone(), c));
                    if scale.is_some() {
                        panel.update(cx, |p, pcx| p.set_scale(scale, pcx));
                    }
                    this.open_chart_window(n, core, panel, geom, true, cx);
                }
                cx.notify();
            });
        });
    }

    /// Активная панель (Main или AddToChart) для показа.
    fn active_panel(&self) -> Entity<ChartPanel> {
        match self.active {
            Tab::Main => self.main.clone(),
            Tab::Add(n, core) => self
                .add
                .iter()
                .find(|(num, c, _)| *num == n && *c == core)
                .map(|(_, _, p)| p.clone())
                .unwrap_or_else(|| self.main.clone()),
        }
    }

    /// Метка вкладки: «номер-ядро» (при split, ядро известно), иначе «номер».
    fn add_label(&self, n: u32, core: Option<CoreId>, cx: &App) -> String {
        match core {
            Some(cid) => {
                let name = self
                    .backend
                    .read(cx)
                    .session
                    .sessions()
                    .iter()
                    .find(|s| s.id == cid)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                format!("{n}-{name}")
            }
            None => n.to_string(),
        }
    }

    /// Неактивные вкладки отсутствуют в текущей GPUI scene, значит их chart data observe не должен
    /// гонять CPU prepare. Активная/откреплённая панель сама выставит visible=true в своём render.
    fn sync_inactive_chart_visibility(&self, cx: &mut Context<Self>) {
        let active = self.active;
        if !matches!(active, Tab::Main) {
            self.main
                .update(cx, |panel, _| panel.set_scene_visible(false));
        }
        for (n, c, panel) in &self.add {
            if Tab::Add(*n, *c) != active {
                panel.update(cx, |panel, _| panel.set_scene_visible(false));
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
        self.handle_open_request(cx);
        self.ingest(window, cx);
        self.sync_inactive_chart_visibility(cx);
        // Откреп-вкладки: вернуть закрытые в стрип (репин) + восстановить сохранённые окна
        // (charts.json) на первом render — пустыми, ждут детект.
        self.drain_chart_repin(cx);
        self.restore_detached(cx);
        // Бейджи = непрочитанные С МОМЕНТА УХОДА: на АКТИВНОЙ вкладке seen догоняет pane_count
        // (бейджа нет — ты смотришь). Ушёл → seen заморожен → новые монеты растят бейдж только
        // этой вкладки (а не всех открытых). Прибраться от закрытых вкладок: чистим seen.
        if let Tab::Add(n, c) = self.active {
            if let Some((_, _, panel)) = self.add.iter().find(|(num, cc, _)| *num == n && *cc == c)
            {
                let cnt = panel.read(cx).pane_count();
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
            let active = self.active_panel();
            if rev != self.last_scale_rev {
                self.last_scale_rev = rev;
                active.update(cx, |p, pcx| p.set_scale(want, pcx));
            } else {
                let cur = active.read(cx).scale();
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
        tabs.extend(self.add.iter().map(|(n, core, panel)| {
            let count = panel.read(cx).pane_count();
            let seen = self.seen.get(&(*n, *core)).copied().unwrap_or(0);
            (
                Tab::Add(*n, *core),
                self.add_label(*n, *core, cx),
                count,
                count.saturating_sub(seen),
                true,
            )
        }));
        let tab_keys = Rc::new(
            tabs.iter()
                .map(|(tab, _, _, _, _)| *tab)
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
                    let Some(tab_id) = tab_keys.get(ix).copied() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if !matches!(tab_id, Tab::Main) && event.click_count() >= 2 {
                            this.detach(tab_id, cx);
                        } else if matches!(tab_id, Tab::Main)
                            || this.add.iter().any(|(n, c, _)| Tab::Add(*n, *c) == tab_id)
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
                    let Some(tab_id) = tab_keys.get(ix).copied() else {
                        return;
                    };
                    if matches!(tab_id, Tab::Main) {
                        return;
                    }
                    view.update(app, |this, cx| {
                        this.add.retain(|(n, c, _)| Tab::Add(*n, *c) != tab_id);
                        if this.active == tab_id {
                            this.active = Tab::Main;
                        }
                        cx.notify();
                    });
                }
            });

        v_flex()
            .size_full()
            .child(
                div()
                    .h(px(CHART_TAB_STRIP_H))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip),
            )
            .child(div().flex_1().w_full().child(self.active_panel()))
    }
}

/// Хост-вид окна откреплённой чарт-вкладки: шапка (масштаб + «закрыть все графики») + панель.
/// Сам пишет геометрию окна в charts.json (`observe_window_bounds`) и просит репин по закрытию
/// (`on_release` → `chart_repin_request`, дренит ChartTabs).
struct DetachedChartHost {
    panel: Entity<ChartPanel>,
    backend: Entity<Backend>,
    group: String,
    num: u32,
    core: Option<CoreId>,
    /// Можно ли сохранять геометрию из `observe_window_bounds`. У ВОССТАНОВЛЕННОГО окна сперва
    /// false: авто-размещение gpui на не-primary DPI читается со сдвигом ×scale, и пересохранять
    /// его НЕЛЬЗЯ (иначе позиция уезжает с каждым запуском). Армируется через ~1.5с — дальше
    /// пишем только реальные перемещения пользователя. У свежего детача — сразу true.
    persist_armed: bool,
    /// Логический размер для коррекции на ПЕРВОМ render восстановленного окна: gpui создаёт окно
    /// на primary, и `WM_DPICHANGED` при переезде на монитор с другим DPI пере-масштабирует
    /// РАЗМЕР (позиция уже верная) → форсим сохранённый логический размер один раз. None у детача.
    restore_size: Option<Size<Pixels>>,
}

impl DetachedChartHost {
    fn new(
        panel: Entity<ChartPanel>,
        backend: Entity<Backend>,
        group: String,
        num: u32,
        core: Option<CoreId>,
        restored: bool,
        restore_size: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Геометрия окна (causal bounds event) → charts.json («то же место» при загрузке).
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Восстановленное окно: НИКОГДА не пересохраняем геометрию автоматически. gpui на
        // не-primary DPI читает позицию со сдвигом ×scale (баг размещения, см. заметку для
        // MoonUI GPUI), и если её сохранить — на след. запуске окно уезжает ещё → улетает за
        // экран → дефолт (компаундинг). Поэтому сохранённую позицию НЕ трогаем: рестор кладёт
        // окно на исходное место и держит стабильно. (Свежий детач — persist_armed=true.)
        // Закрытие окна → репин в стрип (дренит ChartTabs). На выходе приложения запрос не
        // обработается → спека остаётся откреплённой → окно восстановится на след. запуске.
        let (g, n, c) = (group.clone(), num, core);
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, _| {
                b.chart_repin_request.push((g.clone(), n, c));
            });
        })
        .detach();
        Self {
            panel,
            backend,
            group,
            num,
            core,
            persist_armed: !restored,
            restore_size,
        }
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        // У восстановленного окна сохранение пока заглушено (см. persist_armed): не даём авто-
        // размещению gpui (со сдвигом ×scale на не-primary DPI) перезаписать сохранённую позицию.
        if !self.persist_armed {
            return;
        }
        let wb = window.window_bounds();
        let WindowBounds::Windowed(b) = wb else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed ({:?}) → геометрия не сохранена",
                self.num,
                std::mem::discriminant(&wb)
            ));
            return;
        };
        let geom = chart_persist::WinGeom {
            x: f32::from(b.origin.x) as i32,
            y: f32::from(b.origin.y) as i32,
            w: f32::from(b.size.width) as u32,
            h: f32::from(b.size.height) as u32,
        };
        let (group, num, core) = (self.group.clone(), self.num, self.core);
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.core == core)
            {
                let cur = s.detached.map(|g| (g.x, g.y, g.w, g.h));
                if cur != Some((geom.x, geom.y, geom.w, geom.h)) {
                    s.detached = Some(geom);
                    bk.chart_specs_dirty = true;
                }
                true
            } else {
                false
            }
        });
        moon_core::detect_diag::line(&format!(
            "[geom] n={num} core={core:?} → x={} y={} w={} h={} (spec_found={found})",
            geom.x, geom.y, geom.w, geom.h
        ));
    }
}

impl Render for DetachedChartHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Коррекция размера восстановленного окна (один раз): окно уже на целевом мониторе с
        // верным scale → форсим сохранённый логический размер, перебивая DPICHANGED-сжатие.
        if let Some(sz) = self.restore_size.take() {
            window.resize(sz);
        }
        let p = MoonPalette::active(cx);
        // Масштаб — СВОЙ у этой панели (по-вкладочно), правится прямо в неё.
        let scale = self.panel.read(cx).scale();
        let panel = self.panel.clone();
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .px(design::ui_px(cx, 8.0))
                    .bg(rgba(0x121416E6))
                    .child(crate::controls::scale_dropdown_for_panel(
                        scale,
                        self.panel.clone(),
                        p,
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("detached-close-all")
                            .px(design::ui_px(cx, 8.0))
                            .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(design::ui_px(cx, 3.0))
                            .text_size(design::text_px(cx, 11.0))
                            .text_color(rgba(0xC8CCD0FF))
                            .bg(rgba(0x00000059))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xE04848CC)).text_color(rgb(0xFFFFFF)))
                            .child("Закрыть все графики")
                            .on_mouse_down(MouseButton::Left, move |_e, _w, app| {
                                panel.update(app, |p, cx| p.close_all_panes(cx));
                            }),
                    ),
            )
            .child(div().flex_1().w_full().child(self.panel.clone()))
    }
}
