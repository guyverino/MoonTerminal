//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use rust_i18n::t;
use moon_ui::components::{progress::Progress, tag::Tag};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonWindowFrame, h_flex,
};

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
            MoonWindowFrame::main("terminal-header-brand-drag", 0.0)
                .brand_cluster(cx)
                .flex_none()
                .h_full(),
        )
        .child(
            MoonWindowFrame::main("terminal-header-metrics-drag", 0.0)
                .drag_handle()
                .flex()
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .min_w_0()
                .overflow_hidden()
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
            MoonWindowFrame::main("terminal-header-spacer-drag", 0.0)
                .drag_handle()
                .h_full()
                .flex_1()
                .flex(),
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
                    t!("toolbar.strategies").to_string(),
                    {
                        let backend = backend.clone();
                        move |_, window, cx| {
                            strategies::open(backend.clone(), Some(window.window_handle()), cx)
                        }
                    },
                    p,
                    cx,
                ))
                .child(header_action(
                    "gear",
                    "⚙",
                    {
                        let backend = backend.clone();
                        move |_, window, cx| {
                            settings::open(backend.clone(), Some(window.window_handle()), cx)
                        }
                    },
                    p,
                    cx,
                ))
                .when(design::show_custom_window_controls(), |this| {
                    this.child(
                        MoonWindowFrame::main("terminal-header-controls", 0.0)
                            .show_controls(true)
                            .visual_controls(cx),
                    )
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
            div().w(px(64.0)).child(
                Progress::new("risk-meter")
                    .value(18.0)
                    .color(rgb(p.green))
                    .h(design::ui_px(cx, 4.0))
                    .rounded(design::ui_px(cx, 2.0)),
            ),
        )
        .child(div().text_color(rgb(p.green)).child("18%"))
}

fn exchange_pill(p: MoonPalette, cx: &App) -> impl IntoElement {
    Tag::new()
        .outline()
        .rounded_full()
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

fn header_action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    _p: MoonPalette,
    _cx: &App,
) -> impl IntoElement {
    let id: SharedString = id.into();
    MoonButton::new(id)
        .label(label)
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Panel)
        .on_click(on_click)
        .render()
}
