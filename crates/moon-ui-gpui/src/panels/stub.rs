//! Заглушка-панель (Активы/Лог/Отчёт) до подключения данных. Имя = panel_name (для
//! персиста раскладки фабрика восстанавливает по нему, заголовок известен по имени).
//! Кнопка «⧉» откпрепляет панель в отдельное окно (убирает из дока + окно открепления).

use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState, PanelView, TabPanel},
};

use crate::detached::DetachedSpec;
use crate::{hex, Backend};
use moon_core::palette;

/// Заглушка-панель (Активы/Лог/Отчёт) до подключения данных.
pub struct StubPanel {
    name: &'static str,
    title: SharedString,
    /// Группа окна-владельца — для персиста раскладки и открепления (спека).
    group: String,
    /// Общий backend — для записи спеки открепления / репина.
    backend: Entity<Backend>,
    /// Таб-панель дока, в которой живёт эта панель (для самоудаления при откреплении).
    tab: Option<WeakEntity<TabPanel>>,
    focus: FocusHandle,
}

impl StubPanel {
    pub fn new(
        name: &'static str,
        title: impl Into<SharedString>,
        group: String,
        backend: Entity<Backend>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self { name, title: title.into(), group, backend, tab: None, focus: cx.focus_handle() }
    }
}

impl EventEmitter<PanelEvent> for StubPanel {}
impl Focusable for StubPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for StubPanel {
    fn panel_name(&self) -> &'static str {
        self.name
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group(self.name, &self.group)
    }
    /// Запоминаем таб-панель-владельца — нужна, чтобы убрать себя из дока при откреплении.
    fn on_added_to(&mut self, tab_panel: WeakEntity<TabPanel>, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab = Some(tab_panel);
    }
    /// Кнопка «⧉»: убрать панель из дока + открыть в отдельном окне + записать спеку
    /// (персист → на старте восстановится отцепленной). Порт egui `open_detached`.
    fn toolbar_buttons(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let name = self.name;
        let tab = self.tab.clone();
        let me = cx.entity().downgrade();
        Some(vec![Button::new(SharedString::from(format!("detach-{name}")))
            .ghost()
            .label("⧉")
            .tooltip("В отдельное окно")
            .on_click(move |_, window, app| {
                // Убрать себя из дока (taб-панель + собственный entity как PanelView).
                if let (Some(tab), Some(me)) = (tab.as_ref().and_then(|t| t.upgrade()), me.upgrade()) {
                    let arc: Arc<dyn PanelView> = Arc::new(me);
                    tab.update(app, |tp, cx| tp.remove_panel(arc, window, cx));
                }
                // Открыть окно открепления + записать спеку.
                let spec = DetachedSpec::new(group.clone(), name.to_string());
                crate::detached::spawn(app, &backend, &spec);
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            })])
    }
}
impl Render for StubPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.name)
            .size_full()
            .p_4()
            .track_focus(&self.focus)
            .text_color(rgb(hex(palette::TEXT_2)))
            .child(format!("{} — скоро", self.title))
    }
}
