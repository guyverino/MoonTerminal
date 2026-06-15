//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.

use std::collections::HashMap;

use gpui::*;
use moon_palette::{MoonBackgroundPolicy, Panel, PanelEvent, PanelState, Root, h_flex, v_flex};

use crate::panels::ChartPanel;
use crate::{Backend, hex};
use moon_core::config::ChartTheme;
use moon_core::palette;
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
    /// Активная вкладка.
    active: Tab,
    /// Вкладка под курсором (для показа ✕ только при наведении). None = нет.
    hovered: Option<Tab>,
    /// Per-core курсор учтённых AddToChart-детектов.
    add_seq: HashMap<CoreId, u64>,
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
        // Дренаж backend → перерисовка (ingest/prune/open_request делаем в render).
        cx.observe(&backend, |_this, _b, cx| cx.notify()).detach();
        Self {
            backend,
            group,
            epoch,
            theme,
            main,
            add: Vec::new(),
            active: Tab::Main,
            hovered: None,
            add_seq: HashMap::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Дабл-клик по чарту AddToChart-вкладки → открыть монету на Main + переключиться.
    fn handle_open_request(&mut self, cx: &mut Context<Self>) {
        let req = self.backend.update(cx, |b, _| b.open_request.take());
        if let Some((core, market)) = req {
            self.main
                .update(cx, |p, pcx| p.open_market(core, market, pcx));
            self.active = Tab::Main;
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
            {
                tab.update(cx, |p, _| p.add_coin(core, &market, ttl));
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
                panel.update(cx, |p, _| p.add_coin(core, &market, ttl));
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
        if self.active == tab {
            self.active = Tab::Main;
        }
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
        cx.open_window(opts, |window, cx| {
            cx.new(|cx| {
                Root::new(panel.clone(), window, cx).background_policy(MoonBackgroundPolicy::NoFill)
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

        let accent = rgb(hex(palette::ACCENT));
        let muted = rgb(hex(palette::TEXT_2));
        let panel = rgb(hex(palette::SURFACE_1));
        let border = rgb(hex(palette::LIFT_HOVER));
        let bg0 = rgb(hex(palette::BG));

        // Вкладка (underline-стиль): подпись + опц. бейдж-счётчик панелей + ✕ ТОЛЬКО при
        // наведении. Одиночный клик — выбрать, ДВОЙНОЙ — открепить (только AddToChart).
        let tab = |id: SharedString,
                   label: String,
                   on: bool,
                   tab_id: Tab,
                   detachable: bool,
                   count: usize,
                   show_close: bool| {
            let (tc, bb) = if on { (accent, accent) } else { (muted, panel) };
            let mut row = h_flex()
                .id(id)
                .items_center()
                .gap_1()
                .px_3()
                .py_1()
                .cursor_pointer()
                .text_color(tc)
                .border_b_2()
                .border_color(bb)
                .child(label);
            if count > 1 {
                row = row.child(
                    div()
                        .px_1()
                        .rounded_full()
                        .bg(accent)
                        .text_color(bg0)
                        .text_xs()
                        .child(count.to_string()),
                );
            }
            if detachable && show_close {
                row = row.child(
                    div()
                        .id("cl")
                        .px_1()
                        .text_color(muted)
                        .cursor_pointer()
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add.retain(|(n, c, _)| Tab::Add(*n, *c) != tab_id);
                            if this.active == tab_id {
                                this.active = Tab::Main;
                            }
                            cx.notify();
                        })),
                );
            }
            row.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                if detachable && e.click_count() >= 2 {
                    this.detach(tab_id, cx); // двойной клик → открепить в окно
                } else {
                    let exists = matches!(tab_id, Tab::Main)
                        || this.add.iter().any(|(n, c, _)| Tab::Add(*n, *c) == tab_id);
                    if exists {
                        this.active = tab_id;
                    }
                }
                cx.notify();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(tab_id);
                } else if this.hovered == Some(tab_id) {
                    this.hovered = None;
                }
                cx.notify();
            }))
        };

        let mut strip = h_flex()
            .w_full()
            .gap_1()
            .px_2()
            .bg(panel)
            .border_b_1()
            .border_color(border)
            .child(tab(
                "tab-main".into(),
                // Main-вкладка всегда «Main» (в фуллскрине может быть много монет).
                "Main".to_string(),
                self.active == Tab::Main,
                Tab::Main,
                false,
                0,
                false,
            ));
        // Снимок (номер, ядро, счётчик панелей) — чтобы не держать &self.add при builder.
        let tabs: Vec<(u32, Option<CoreId>, usize)> = self
            .add
            .iter()
            .map(|(n, c, p)| (*n, *c, p.read(cx).pane_count()))
            .collect();
        for (n, core, count) in tabs {
            let tab_id = Tab::Add(n, core);
            let on = self.active == tab_id;
            let show_close = self.hovered == Some(tab_id);
            let label = self.add_label(n, core, cx);
            let id = SharedString::from(format!("tab-{n}-{}", core.unwrap_or(0)));
            strip = strip.child(tab(id, label, on, tab_id, true, count, show_close));
        }

        v_flex()
            .size_full()
            .child(strip)
            .child(div().flex_1().w_full().child(self.active_panel()))
    }
}
