//! Панель ордера (right dock): BUY/SELL/Cancel/Panic. Порт egui `dock/order.rs`.
//! Действия пока заглушки-лог; форму ввода/реальные ордера прикрутим позже.

use gpui::*;
use moon_palette::{MoonButton, MoonButtonSize, MoonButtonVariant, Panel, PanelEvent, v_flex};

pub struct OrderPanel {
    focus: FocusHandle,
}
impl OrderPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
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
            .child(action(
                "buy",
                "BUY",
                MoonButtonVariant::Green,
                false,
                || log::info!("BUY"),
            ))
            .child(action(
                "sell",
                "SELL",
                MoonButtonVariant::OutlineRed,
                false,
                || log::info!("SELL"),
            ))
            .child(action(
                "cancel",
                "Cancel Buy",
                MoonButtonVariant::Amber,
                false,
                || log::info!("Cancel"),
            ))
            .child(action(
                "panic",
                "PANIC SELL",
                MoonButtonVariant::Danger,
                true,
                || log::info!("PANIC"),
            ))
    }
}

fn action(
    id: &'static str,
    label: &'static str,
    variant: MoonButtonVariant,
    strong: bool,
    f: impl Fn() + 'static,
) -> impl IntoElement {
    MoonButton::new(id)
        .full_width()
        .variant(variant)
        .size(MoonButtonSize::Pill)
        .selected(strong)
        .label(label)
        .on_click(move |_, _, _| f())
        .render()
}
