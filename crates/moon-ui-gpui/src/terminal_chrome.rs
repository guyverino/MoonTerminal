//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex};

use crate::{Backend, design, settings, strategies};

pub fn header(
    group: &str,
    market_label: impl Into<SharedString>,
    backend: Entity<Backend>,
    p: MoonPalette,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(design::HEADER_TOP_H))
        .pl(px(design::titlebar_leading_inset()))
        .pr(px(design::HEADER_PAD_X))
        .gap(px(12.0))
        .bg(rgb(p.shell_high))
        .child(
            h_flex()
                .gap(px(12.0))
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .window_control_area(WindowControlArea::Drag)
                .child(design::logo())
                .child(design::vline(16.0, p))
                .child(design::top_pill(
                    "strat-pill",
                    format!("{} · {}", group, market_label.into()),
                    p,
                ))
                .child(metric("Session", "+$24.30", p.green, p))
                .child(metric("Real", "+$104.20", p.green, p))
                .child(metric("Unreal", "−$8.10", p.orange, p))
                .child(risk_meter(p)),
        )
        .child(
            div()
                .h_full()
                .flex_1()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(px(12.0))
                .items_center()
                .child(exchange_pill(p))
                .child(balance_label(p))
                .child(design::vline(16.0, p))
                .child(header_action(
                    "strategies",
                    "Стратегии",
                    {
                        let backend = backend.clone();
                        move |_, _, cx| strategies::open(backend.clone(), cx)
                    },
                    p,
                ))
                .child(header_action(
                    "gear",
                    "⚙",
                    {
                        let backend = backend.clone();
                        move |_, _, cx| settings::open(backend.clone(), cx)
                    },
                    p,
                ))
                .when(design::show_custom_window_controls(), |this| {
                    this.child(window_controls(p))
                }),
        )
}

fn metric(
    label: &'static str,
    value: &'static str,
    color: u32,
    p: MoonPalette,
) -> impl IntoElement {
    h_flex()
        .h(px(22.0))
        .gap(px(5.0))
        .font_family(design::mono())
        .text_size(px(11.0))
        .child(
            div()
                .text_size(px(9.0))
                .font_family(design::ui_font())
                .text_color(rgb(p.text_muted))
                .child(label),
        )
        .child(
            div()
                .text_color(rgb(color))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
}

fn risk_meter(p: MoonPalette) -> impl IntoElement {
    h_flex()
        .h(px(22.0))
        .gap(px(8.0))
        .font_family(design::mono())
        .text_size(px(11.0))
        .child(
            div()
                .text_size(px(9.0))
                .font_family(design::ui_font())
                .text_color(rgb(p.text_muted))
                .child("Risk"),
        )
        .child(
            div()
                .w(px(64.0))
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(rgb(p.panel))
                .child(div().w(px(12.0)).h(px(4.0)).bg(rgb(p.green))),
        )
        .child(div().text_color(rgb(p.green)).child("18%"))
}

fn exchange_pill(p: MoonPalette) -> impl IntoElement {
    h_flex()
        .h(px(24.0))
        .gap(px(7.0))
        .px(px(10.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .font_family(design::mono())
        .text_size(px(11.0))
        .text_color(rgb(p.text_soft))
        .child(design::status_dot(p.green))
        .child("Binance Futures")
        .child(div().text_color(rgb(p.text_muted)).child("▾"))
}

fn balance_label(p: MoonPalette) -> impl IntoElement {
    h_flex()
        .gap(px(0.0))
        .font_family(design::mono())
        .text_size(px(11.5))
        .text_color(rgb(p.text_soft))
        .child("Balance: ")
        .child(
            div()
                .text_color(rgb(p.text))
                .font_weight(FontWeight::SEMIBOLD)
                .child("50.00"),
        )
        .child(div().text_color(rgb(p.text_muted)).child(" /50 USDT"))
}

fn window_controls(p: MoonPalette) -> impl IntoElement {
    h_flex()
        .h(px(22.0))
        .gap(px(2.0))
        .font_family(design::mono())
        .text_size(px(11.0))
        .child(win_btn("—", p.text_soft))
        .child(win_btn("□", p.text_soft))
        .child(win_btn("×", p.orange))
}

fn win_btn(label: &'static str, color: u32) -> impl IntoElement {
    div()
        .w(px(26.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_color(rgb(color))
        .hover(|s| s.bg(design::alpha(0xFFFFFF, 0x08)))
        .child(label)
}

fn header_action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    p: MoonPalette,
) -> impl IntoElement {
    div()
        .id(id.into())
        .h(px(24.0))
        .flex()
        .items_center()
        .px(px(10.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .font_family(design::mono())
        .text_size(px(11.0))
        .text_color(rgb(p.text_soft))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(p.panel_high)).text_color(rgb(p.text)))
        .child(label.into())
        .on_click(on_click)
}
