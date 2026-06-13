//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль (gpui-component TabPanel не даёт): активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные gpui-Dock-панели.

use std::collections::HashMap;

use gpui::*;
use gpui_component::{
    dock::{Panel, PanelEvent, PanelState},
    h_flex, v_flex, Root,
};

use crate::panels::ChartPanel;
use crate::{hex, Backend};
use moon_core::config::ChartTheme;
use moon_core::palette;
use moon_core::session::CoreId;

pub struct ChartTabs {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    /// Main-чарт (вкладка 0).
    main: Entity<ChartPanel>,
    /// AddToChart-вкладки (номер N → панель), отсортированы по N.
    add: Vec<(u32, Entity<ChartPanel>)>,
    /// Активная вкладка: 0 = Main, N = AddToChart-N.
    active: u32,
    /// Вкладка под курсором (для показа ✕ только при наведении). None = нет.
    hovered: Option<u32>,
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
        let main = cx.new(|cx| ChartPanel::new(backend.clone(), focus_open, epoch, theme.clone(), window, cx));
        // Дренаж backend → перерисовка (ingest/prune/open_request делаем в render).
        cx.observe(&backend, |_this, _b, cx| cx.notify()).detach();
        Self {
            backend,
            group,
            epoch,
            theme,
            main,
            add: Vec::new(),
            active: 0,
            hovered: None,
            add_seq: HashMap::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Дабл-клик по чарту AddToChart-вкладки → открыть монету на Main + переключиться.
    fn handle_open_request(&mut self, cx: &mut Context<Self>) {
        let req = self.backend.update(cx, |b, _| b.open_request.take());
        if let Some((core, market)) = req {
            self.main.update(cx, |p, pcx| p.open_market(core, market, pcx));
            self.active = 0;
        }
    }

    /// Ингест AddToChart-детектов (add_to_chart>0) → создать/наполнить вкладку N.
    /// БЕЗ авто-перехода: active не трогаем (порт «не уводить на чарт при детекте»).
    fn ingest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (fresh, cursors): (Vec<(u32, CoreId, String, f64)>, Vec<(CoreId, u64)>) = {
            let b = self.backend.read(cx);
            let mut fresh = Vec::new();
            let mut cursors = Vec::new();
            for s in b.session.sessions().iter().filter(|s| s.group == self.group) {
                let id = s.id;
                let Some(d) = b.session.store().core(id) else { continue };
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
            (fresh, cursors)
        };
        for (id, mx) in cursors {
            self.add_seq.insert(id, mx);
        }
        if fresh.is_empty() {
            return;
        }
        let (epoch, theme, backend) = (self.epoch, self.theme.clone(), self.backend.clone());
        for (n, core, market, ttl) in fresh {
            backend.update(cx, |b, _| {
                if !b.desired.iter().any(|(c, m)| *c == core && m == &market) {
                    b.desired.push((core, market.clone()));
                }
            });
            if let Some((_, tab)) = self.add.iter().find(|(num, _)| *num == n) {
                tab.update(cx, |p, _| p.add_coin(core, &market, ttl));
            } else {
                let panel = cx.new(|cx| {
                    ChartPanel::new_addto(backend.clone(), n, epoch, theme.clone(), window, cx)
                });
                panel.update(cx, |p, _| p.add_coin(core, &market, ttl));
                self.add.push((n, panel));
                self.add.sort_by_key(|(num, _)| *num);
                // active НЕ меняем — не уводим пользователя на новую вкладку.
            }
        }
    }

    /// Отцепить AddToChart-вкладку N в отдельное ОС-окно (убрать из стрипа).
    fn detach(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(pos) = self.add.iter().position(|(num, _)| *num == n) else { return };
        let (_, panel) = self.add.remove(pos);
        if self.active == n {
            self.active = 0;
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
        cx.open_window(opts, |window, cx| cx.new(|cx| Root::new(panel.clone(), window, cx)))
            .ok();
        cx.notify();
    }

    /// Активная панель (Main или AddToChart-N) для показа.
    fn active_panel(&self) -> Entity<ChartPanel> {
        if self.active == 0 {
            self.main.clone()
        } else {
            self.add
                .iter()
                .find(|(n, _)| *n == self.active)
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| self.main.clone())
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

        // Вкладка (underline-стиль, как в egui-полоске): подпись + опц. бейдж-счётчик
        // панелей + ✕ ТОЛЬКО при наведении. Одиночный клик — выбрать, ДВОЙНОЙ —
        // открепить в ОС-окно (только AddToChart; Main не открепляется). Никаких
        // постоянных кнопок рядом — детач это жест по самой вкладке (порт egui).
        let tab = |id: SharedString,
                   label: String,
                   on: bool,
                   n: u32,
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
            // Бейдж-счётчик открытых панелей (как кружок с числом у egui-вкладок).
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
            // ✕ закрыть — проявляется только при наведении на вкладку.
            if detachable && show_close {
                row = row.child(
                    div()
                        .id(SharedString::from(format!("cl-{n}")))
                        .px_1()
                        .text_color(muted)
                        .cursor_pointer()
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add.retain(|(num, _)| *num != n);
                            if this.active == n {
                                this.active = 0;
                            }
                            cx.notify();
                        })),
                );
            }
            row.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                if detachable && e.click_count() >= 2 {
                    this.detach(n, cx); // двойной клик → открепить в окно
                } else if n == 0 || this.add.iter().any(|(num, _)| *num == n) {
                    // одиночный → выбрать (но не «оживлять» только что закрытую ✕ вкладку)
                    this.active = n;
                }
                cx.notify();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(n);
                } else if this.hovered == Some(n) {
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
                self.main.read(cx).title_text(),
                self.active == 0,
                0,
                false,
                0,
                false,
            ));
        for (n, p) in &self.add {
            let n = *n;
            let on = self.active == n;
            let count = p.read(cx).pane_count();
            let show_close = self.hovered == Some(n);
            strip = strip.child(tab(
                SharedString::from(format!("tab-{n}")),
                format!("Чарт {n}"),
                on,
                n,
                true,
                count,
                show_close,
            ));
        }

        v_flex()
            .size_full()
            .child(strip)
            .child(div().flex_1().w_full().child(self.active_panel()))
    }
}
