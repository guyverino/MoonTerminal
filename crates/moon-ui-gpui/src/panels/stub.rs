//! Заглушка-панель (Активы/Лог/Отчёт) до подключения данных. Имя = panel_name (для
//! персиста раскладки фабрика восстанавливает по нему, заголовок известен по имени).
//! Кнопка «⧉» откпрепляет панель в отдельное окно (убирает из дока + окно открепления).

use gpui::*;
use moon_palette::{
    DockArea, MoonButton, MoonButtonSize, MoonPalette, Panel, PanelEvent, PanelState,
};

use crate::Backend;
use crate::detached::DetachedSpec;

/// Заглушка-панель (Активы/Лог/Отчёт) до подключения данных.
pub struct StubPanel {
    name: &'static str,
    title: SharedString,
    /// Группа окна-владельца — для персиста раскладки и открепления (спека).
    group: String,
    /// Общий backend — для записи спеки открепления / репина.
    backend: Entity<Backend>,
    /// DockArea-владелец — нужен для самоудаления при откреплении.
    dock: Option<WeakEntity<DockArea>>,
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
        Self {
            name,
            title: title.into(),
            group,
            backend,
            dock: None,
            focus: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for StubPanel {}
impl Focusable for StubPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for StubPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        self.name
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group(self.name, &self.group)
    }
    /// Запоминаем dock-владельца — нужен, чтобы убрать себя из дока при откреплении.
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    /// Кнопка «⧉»: убрать панель из дока + открыть в отдельном окне + записать спеку
    /// (персист → на старте восстановится отцепленной). Порт egui `open_detached`.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let name = self.name;
        let dock = self.dock.clone();
        Some(vec![
            MoonButton::new(SharedString::from(format!("detach-{name}")))
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .on_click(move |_, window, app| {
                    // Убрать себя из дока.
                    if let Some(dock) = dock.as_ref().and_then(|d| d.upgrade()) {
                        dock.update(app, |area, cx| {
                            area.remove_panel_by_name(name, window, cx);
                        });
                    }
                    // Открыть окно открепления + записать спеку.
                    let spec = DetachedSpec::new(group.clone(), name.to_string());
                    crate::detached::spawn(app, &backend, &spec);
                    backend.update(app, |b, _| {
                        b.detached.push(spec);
                        b.detached_dirty = true;
                    });
                })
                .render()
                .into_any_element(),
        ])
    }
}
impl Render for StubPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .id(self.name)
            .size_full()
            .p_4()
            .track_focus(&self.focus)
            .text_color(rgb(p.text_soft))
            .child(format!("{} — скоро", self.title))
    }
}
