//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use moon_palette::{
    MoonBackgroundPolicy, MoonTabItem, MoonTabStrip, Panel, PanelEvent, PanelState, Root, v_flex,
};

use crate::Backend;
use crate::panels::ChartPanel;
use moon_core::config::ChartTheme;
use moon_core::session::CoreId;

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
    /// Per-core курсор учтённых AddToChart-детектов.
    add_seq: HashMap<CoreId, u64>,
    /// Сигнатура входов, которые реально меняют tab-strip: AddToChart-детекты,
    /// split-настройка и явный запрос открыть монету на Main.
    last_sig: u64,
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
            add_seq: HashMap::new(),
            last_sig: initial_sig,
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
    fn ingest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let (epoch, theme, backend) = (self.epoch, self.theme.clone(), self.backend.clone());
        for (n, core, market, ttl) in fresh {
            let key_core = if split { Some(core) } else { None };
            backend.update(cx, |b, _| {
                if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                    b.desired.push((core, market.clone()));
                }
            });
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
                tab.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
            } else {
                let panel = cx.new(|cx| {
                    ChartPanel::new_addto(
                        backend.clone(),
                        n,
                        key_core,
                        epoch,
                        theme.clone(),
                        window,
                        cx,
                    )
                });
                panel.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
                self.add.push((n, key_core, panel));
                // Порядок вкладок: по (номер, ядро) — как egui sort_by_key.
                self.add.sort_by_key(|(num, c, _)| (*num, c.unwrap_or(0)));
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
        // Держим панель (не теряем): при закрытии окна вернём в стрип.
        self.detached.push((n, core, panel.clone()));
        if self.active == tab {
            self.active = Tab::Main;
        }
        // Снять own-pass с главного окна — на своём окне он перерегистрируется сам.
        panel.update(cx, |p, _| p.unregister_pass());
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(200.0), px(160.0)),
                size: size(px(900.0), px(620.0)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("MoonTerminal — Чарт {n}").into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        // Хост-вид окна откреп: его release (закрытие окна) → репин панели в стрип.
        let host = cx.new(|_| DetachedChartHost {
            panel: panel.clone(),
        });
        cx.observe_release(&host, move |this, _host, cx| {
            if let Some(p) = this
                .detached
                .iter()
                .position(|(num, c, _)| *num == n && *c == core)
            {
                let (num, c, pnl) = this.detached.remove(p);
                this.add.push((num, c, pnl));
                this.add.sort_by_key(|(num, c, _)| (*num, c.unwrap_or(0)));
                cx.notify();
            }
        })
        .detach();
        cx.open_window(opts, move |window, cx| {
            cx.new(|cx| {
                Root::new(host.clone(), window, cx).background_policy(MoonBackgroundPolicy::NoFill)
            })
        })
        .ok();
        cx.notify();
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

    /// Снять own-pass у НЕактивных вкладок: их панели не рендерятся (их render не
    /// зовётся), и без снятия их pas остаётся на окне и рисует застывший чарт поверх
    /// активного (BUG-2). Активная вкладка регистрирует pas в собственном render.
    fn sync_inactive_passes(&self, cx: &mut Context<Self>) {
        let active = self.active;
        let mut inactive: Vec<Entity<ChartPanel>> = Vec::new();
        if !matches!(active, Tab::Main) {
            inactive.push(self.main.clone());
        }
        for (n, c, panel) in &self.add {
            if Tab::Add(*n, *c) != active {
                inactive.push(panel.clone());
            }
        }
        for p in inactive {
            p.update(cx, |panel, _| panel.unregister_pass());
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
        self.sync_inactive_passes(cx);

        // Снимок вкладок — чтобы callbacks не держали borrow self.add.
        let mut tabs: Vec<(Tab, String, usize, bool)> =
            vec![(Tab::Main, "Main".to_string(), 0, false)];
        tabs.extend(self.add.iter().map(|(n, core, panel)| {
            (
                Tab::Add(*n, *core),
                self.add_label(*n, *core, cx),
                panel.read(cx).pane_count(),
                true,
            )
        }));
        let tab_keys = Rc::new(tabs.iter().map(|(tab, _, _, _)| *tab).collect::<Vec<_>>());
        let items = tabs
            .iter()
            .map(|(tab, label, count, detachable)| {
                let width = (label.chars().count() as f32 * 7.0
                    + if *count > 1 { 38.0 } else { 28.0 }
                    + if *detachable { 20.0 } else { 0.0 })
                .clamp(72.0, 168.0);
                let mut item = MoonTabItem::new(label.clone())
                    .width(width)
                    .selected(self.active == *tab)
                    .closable(*detachable);
                if *count > 1 {
                    item = item.badge(count.to_string());
                }
                item
            })
            .collect::<Vec<_>>();
        let view = cx.entity();
        let strip = MoonTabStrip::new("chart-tabs-strip")
            .padding_left(8.0)
            .gap(4.0)
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
            .child(strip)
            .child(div().flex_1().w_full().child(self.active_panel()))
    }
}

/// Хост-вид окна откреплённой чарт-вкладки: рендерит панель; его release (закрытие
/// окна) ChartTabs ловит через `observe_release` → возвращает панель в стрип.
struct DetachedChartHost {
    panel: Entity<ChartPanel>,
}

impl Render for DetachedChartHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.panel.clone())
    }
}
