//! Заглушка-панель (Активы/Лог/Отчёт) до подключения данных. Имя = panel_name (для
//! персиста раскладки фабрика восстанавливает по нему, заголовок известен по имени).

use gpui::*;
use gpui_component::dock::{Panel, PanelEvent};

use crate::hex;
use moon_core::palette;

/// Заглушка-панель (Активы/Лог/Отчёт) до подключения данных.
pub struct StubPanel {
    name: &'static str,
    title: SharedString,
    focus: FocusHandle,
}

impl StubPanel {
    pub fn new(name: &'static str, title: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self { name, title: title.into(), focus: cx.focus_handle() }
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
