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
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0))
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .gap(design::ui_px(cx, 12.0))
        .bg(rgb(p.shell_high))
        .child(
            h_flex()
                .gap(design::ui_px(cx, 12.0))
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
                    cx,
                ))
                .child(metric("Session", "+$24.30", p.green, p, cx))
                .child(metric("Real", "+$104.20", p.green, p, cx))
                .child(metric("Unreal", "−$8.10", p.orange, p, cx))
                .child(risk_meter(p, cx)),
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
                .gap(design::ui_px(cx, 12.0))
                .items_center()
                .child(exchange_pill(p, cx))
                .child(balance_label(p, cx))
                .child(design::vline(16.0, p))
                .child(header_action(
                    "strategies",
                    "Стратегии",
                    {
                        let backend = backend.clone();
                        move |_, _, cx| strategies::open(backend.clone(), cx)
                    },
                    p,
                    cx,
                ))
                .child(header_action(
                    "gear",
                    "⚙",
                    {
                        let backend = backend.clone();
                        move |_, _, cx| settings::open(backend.clone(), cx)
                    },
                    p,
                    cx,
                ))
                .when(design::show_custom_window_controls(), |this| {
                    this.child(window_controls(p, cx))
                }),
        )
}

fn metric(
    label: &'static str,
    value: &'static str,
    color: u32,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
        .gap(design::ui_px(cx, 5.0))
        .font_family(design::mono())
        .text_size(design::text_px(cx, 11.0))
        .child(
            div()
                .text_size(design::text_px(cx, 9.0))
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

fn risk_meter(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
        .gap(design::ui_px(cx, 8.0))
        .font_family(design::mono())
        .text_size(design::text_px(cx, 11.0))
        .child(
            div()
                .text_size(design::text_px(cx, 9.0))
                .font_family(design::ui_font())
                .text_color(rgb(p.text_muted))
                .child("Risk"),
        )
        .child(
            div()
                .w(px(64.0))
                .h(design::ui_px(cx, 4.0))
                .rounded(design::ui_px(cx, 2.0))
                .bg(rgb(p.panel))
                .child(div().w(px(12.0)).h(design::ui_px(cx, 4.0)).bg(rgb(p.green))),
        )
        .child(div().text_color(rgb(p.green)).child("18%"))
}

fn exchange_pill(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .h(design::fit_h_px(cx, 24.0, 13.0, 5.5))
        .gap(design::ui_px(cx, 7.0))
        .px(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 999.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .font_family(design::mono())
        .text_size(design::text_px(cx, 11.0))
        .text_color(rgb(p.text_soft))
        .child(design::status_dot(p.green, cx))
        .child("Binance Futures")
        .child(div().text_color(rgb(p.text_muted)).child("▾"))
}

fn balance_label(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .gap(px(0.0))
        .font_family(design::mono())
        .text_size(design::text_px(cx, 11.5))
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

fn window_controls(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .h(design::fit_h_px(cx, 22.0, 11.0, 5.5))
        .gap(design::ui_px(cx, 2.0))
        .font_family(design::mono())
        .text_size(design::text_px(cx, 11.0))
        .child(win_btn("—", p.text_soft, cx))
        .child(win_btn("□", p.text_soft, cx))
        .child(win_btn("×", p.orange, cx))
}

fn win_btn(label: &'static str, color: u32, cx: &App) -> impl IntoElement {
    div()
        .w(design::ui_px(cx, 26.0))
        .h(design::fit_h_px(cx, 22.0, 11.0, 5.5))
        .flex()
        .items_center()
        .justify_center()
        .rounded(design::ui_px(cx, 4.0))
        .text_color(rgb(color))
        .hover(|s| s.bg(design::alpha(0xFFFFFF, 0x08)))
        .child(label)
}

fn header_action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(id.into())
        .h(design::fit_h_px(cx, 24.0, 13.0, 5.5))
        .flex()
        .items_center()
        .px(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .font_family(design::mono())
        .text_size(design::text_px(cx, 11.0))
        .text_color(rgb(p.text_soft))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(p.panel_high)).text_color(rgb(p.text)))
        .child(label.into())
        .on_click(on_click)
}
