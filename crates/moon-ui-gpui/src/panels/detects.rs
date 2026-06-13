//! Лента детектов — откпрепляемая панель (порт egui `DetectRibbon`). Втягивает детекты
//! ядер группы с `SoundAlert=Yes` и `AddToChart==0` (AddToChart-детекты — в чарт-вкладки),
//! держит `KeepAlert` секунд, новые сверху. Клик → открыть монету на Main (через
//! `Backend.open_request`, который читает Shell).

use std::collections::{HashMap, VecDeque};

use gpui::*;
use gpui_component::{
    dock::{Panel, PanelEvent, PanelState},
    h_flex, v_flex,
};

use crate::{hex, Backend};
use moon_chart::paint::now_unix_ms;
use moon_core::palette;
use moon_core::session::CoreId;

/// Кнопка ленты детектов (порт `src/dock/detects.rs::RibbonItem`).
struct DetectItem {
    core: CoreId,
    core_name: String,
    market: String,
    color: [u8; 3],
    born_ms: f64,
    ttl_ms: f64,
}

pub struct DetectsPanel {
    backend: Entity<Backend>,
    group: String,
    items: VecDeque<DetectItem>,
    last_seq: HashMap<CoreId, u64>,
    focus: FocusHandle,
}

const MAX_DETECT_BTNS: usize = 48;

impl DetectsPanel {
    pub fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        cx.observe(&backend, |this, backend, cx| {
            this.ingest(backend.read(cx));
            this.prune(now_unix_ms());
            cx.notify();
        })
        .detach();
        Self { backend, group, items: VecDeque::new(), last_seq: HashMap::new(), focus: cx.focus_handle() }
    }

    /// Втянуть свежие детекты ядер группы (seq > курсора, sound_alert, не AddToChart).
    fn ingest(&mut self, b: &Backend) {
        let cores: Vec<(CoreId, String, [u8; 3])> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| {
                let color = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == s.id)
                    .map(|sv| sv.color)
                    .unwrap_or(palette::ACCENT);
                (s.id, s.name.clone(), color)
            })
            .collect();
        for (id, name, color) in cores {
            let Some(d) = b.session.store().core(id) else { continue };
            let last = self.last_seq.get(&id).copied().unwrap_or(0);
            let mut fresh: Vec<&moon_core::feed::DetectRow> = Vec::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                fresh.push(det);
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(id, fresh[0].seq);
            for det in fresh.iter().rev() {
                if !det.sound_alert || det.add_to_chart > 0 {
                    continue;
                }
                let ttl = (det.keep_alert_secs.max(1) as f64) * 1000.0;
                if let Some(it) = self.items.iter_mut().find(|it| it.core == id && it.market == det.market) {
                    it.born_ms = det.time_ms;
                    it.ttl_ms = ttl;
                    it.color = color;
                } else {
                    self.items.push_back(DetectItem {
                        core: id,
                        core_name: name.clone(),
                        market: det.market.clone(),
                        color,
                        born_ms: det.time_ms,
                        ttl_ms: ttl,
                    });
                }
            }
        }
        while self.items.len() > MAX_DETECT_BTNS {
            self.items.pop_front();
        }
    }

    fn prune(&mut self, now_ms: f64) {
        self.items.retain(|it| now_ms - it.born_ms < it.ttl_ms);
    }

    /// Открыть монету на Main: запрос в Backend (Shell откроет чарт) + убрать кнопку.
    fn open(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.items.retain(|it| !(it.core == core && it.market == market));
        self.backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market.clone()));
            bcx.notify();
        });
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for DetectsPanel {}
impl Focusable for DetectsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for DetectsPanel {
    fn panel_name(&self) -> &'static str {
        "Detects"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Детекты")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Detects", &self.group)
    }
}
impl Render for DetectsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_unix_ms();
        let mut col = v_flex().id("detects").size_full().gap_1().p_2().track_focus(&self.focus);
        // Новые сверху.
        for (i, it) in self.items.iter().enumerate().rev() {
            let secs = ((it.ttl_ms - (now - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            let glow = rgb(hex(it.color));
            let (core, market) = (it.core, it.market.clone());
            col = col.child(
                div()
                    .id(SharedString::from(format!("det-{i}")))
                    .w_full()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(glow)
                    .bg(rgb(hex(palette::LIFT)))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(div().child(it.market.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(hex(palette::TEXT_2)))
                                    .child(format!("{secs}s · {}", it.core_name)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open(core, market.clone(), cx);
                    })),
            );
        }
        col
    }
}
