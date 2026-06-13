//! Панель ордера (right dock): BUY/SELL/Cancel/Panic. Порт egui `dock/order.rs`.
//! Действия пока заглушки-лог; форму ввода/реальные ордера прикрутим позже.

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    v_flex,
};

pub struct OrderPanel {
    focus: FocusHandle,
}
impl OrderPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle() }
    }
}
impl EventEmitter<PanelEvent> for OrderPanel {}
impl Focusable for OrderPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrderPanel {
    fn panel_name(&self) -> &'static str {
        "Order"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Ордер")
    }
}
impl Render for OrderPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("order-panel")
            .size_full()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            .child(Button::new("buy").success().label("BUY").on_click(|_, _, _| log::info!("BUY")))
            .child(Button::new("sell").danger().label("SELL").on_click(|_, _, _| log::info!("SELL")))
            .child(Button::new("cancel").warning().label("Cancel Buy").on_click(|_, _, _| log::info!("Cancel")))
            .child(Button::new("panic").danger().label("PANIC SELL").on_click(|_, _, _| log::info!("PANIC")))
    }
}
