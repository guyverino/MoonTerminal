//! Торговый тулбар: прикладная сборка терминала поверх MoonPalette.
//!
//! Логика остаётся терминальной: size/sell пока логируют todo, scale/live пишут в
//! `Backend`. Визуальные контролы берём из палитры, выведенной из HTML-эталона.

use gpui::*;

use moon_palette::{
    MoonAccent, MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonPalette,
    MoonPopover, MoonPopoverPlacement, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};

use crate::{Backend, design};

/// Высота полосы тулбара: 2-я строка header из HTML-эталона.
pub const TOOLBAR_H: f32 = design::TOOLBAR_H;

/// Пресеты масштаба цены (Y) — 1:1 с egui `dock/controls.rs::SCALES`. `None` = «Авто».
const SCALES: [(&str, Option<f32>, f32); 6] = [
    ("Авто", None, 48.0),
    ("50%", Some(0.50), 44.0),
    ("20%", Some(0.20), 44.0),
    ("10%", Some(0.10), 44.0),
    ("5%", Some(0.05), 38.0),
    ("2%", Some(0.02), 38.0),
];

/// Подписи полосок `size` / `sell` (как на стенде).
const SIZE_KEYS: [&str; 6] = ["F1", "F2", "F3", "F4", "F5", "F6"];
const SELL_KEYS: [&str; 6] = ["S1", "S2", "S3", "S4", "S5", "S6"];

fn toolbar_metric(
    id: &'static str,
    label: &'static str,
    value: &'static str,
    color: u32,
    width: f32,
    p: MoonPalette,
) -> impl IntoElement {
    MoonButton::new(id)
        .width(width)
        .variant(MoonButtonVariant::Neutral)
        .size(MoonButtonSize::Toolbar)
        .segment(
            MoonButtonSegment::new(label)
                .color(p.text_muted)
                .weight(400.0),
        )
        .text_segment(value, color, 500.0)
        .render()
}

/// Мелкая тусклая подпись группы (`size`/`sell`/`МАСШТАБ`) — стендовый `.strip-label`.
fn strip_label(text: &'static str, p: MoonPalette) -> impl IntoElement {
    div()
        .text_size(px(9.5))
        .font_family(design::ui_font())
        .text_color(rgb(p.text_muted))
        .child(text)
}

/// Вертикальный разделитель групп (стендовый `.divider`): тонкая линия высотой 16px.
fn divider(p: MoonPalette) -> impl IntoElement {
    design::vline(16.0, p)
}

fn size_strip() -> impl IntoElement {
    MoonSegmentedControl::new("toolbar-size-presets")
        .accent(MoonAccent::Amber)
        .items([
            MoonSegmentItem::new("F1", "0.01").width(54.0),
            MoonSegmentItem::new("F2", "0.025").width(61.0),
            MoonSegmentItem::new("F3", "0.05")
                .width(56.0)
                .selected(true),
            MoonSegmentItem::new("F4", "0.10").width(56.0),
            MoonSegmentItem::new("F5", "0.25").width(56.0),
            MoonSegmentItem::new("F6", "0.50").width(56.0),
        ])
        .on_click(|ix, _, _, _| log::info!("[ui] size {} (todo)", SIZE_KEYS[ix]))
        .render()
}

fn sell_strip() -> impl IntoElement {
    MoonSegmentedControl::new("toolbar-sell-presets")
        .accent(MoonAccent::Blue)
        .items([
            MoonSegmentItem::new("S1", "+1.0%").width(62.0),
            MoonSegmentItem::new("S2", "+2.0%").width(62.0),
            MoonSegmentItem::new("S3", "+3.0%")
                .width(62.0)
                .selected(true),
            MoonSegmentItem::new("S4", "+5.0%").width(62.0),
            MoonSegmentItem::new("S5", "+10%").width(56.0),
            MoonSegmentItem::new("S6", "mk%").width(52.0),
        ])
        .on_click(|ix, _, _, _| log::info!("[ui] sell {} (todo)", SELL_KEYS[ix]))
        .render()
}

fn scale_button(label: &'static str, selected: bool, width: f32) -> MoonButton {
    MoonButton::new(format!("scale-{label}"))
        .width(width)
        .variant(if selected {
            MoonButtonVariant::Amber
        } else {
            MoonButtonVariant::Soft
        })
        .size(MoonButtonSize::Toolbar)
        .selected(selected)
        .label(label)
}

fn scale_label(scale: Option<f32>) -> &'static str {
    SCALES
        .iter()
        .find(|(_, value, _)| *value == scale)
        .map(|(label, _, _)| *label)
        .unwrap_or("Авто")
}

fn scale_popover(scale: Option<f32>, backend: Entity<Backend>, p: MoonPalette) -> impl IntoElement {
    let selected_label = scale_label(scale);
    let trigger = MoonButton::new("toolbar-scale-trigger")
        .width(112.0)
        .variant(MoonButtonVariant::Neutral)
        .size(MoonButtonSize::Toolbar)
        .segment(
            MoonButtonSegment::new("МАСШТАБ")
                .color(p.text_muted)
                .weight(400.0),
        )
        .text_segment(selected_label, p.text, 500.0)
        .render();

    let mut menu = v_flex().gap(px(4.0));
    for (label, pct, _) in SCALES {
        let backend = backend.clone();
        menu = menu.child(
            scale_button(label, scale == pct, 104.0)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        b.price_scale = pct;
                        bcx.notify();
                    });
                })
                .render(),
        );
    }

    MoonPopover::new("toolbar-scale-popover")
        .placement(MoonPopoverPlacement::BottomEnd)
        .width(116.0)
        .close_on_content_click(true)
        .trigger(trigger)
        .content(menu)
}

/// Полоса тулбара: рисуется как обычный child `Shell` (между шапкой и доком), не dock-панель.
/// Читает текущий масштаб/follow из `backend`, клики пишут обратно (+notify → перерисовка).
pub fn toolbar(backend: &Entity<Backend>, cx: &App) -> impl IntoElement {
    let (scale, follow) = {
        let b = backend.read(cx);
        (b.price_scale, b.follow)
    };
    let p = MoonPalette::active(cx);

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(px(TOOLBAR_H))
        .items_center()
        .gap(px(6.0))
        .px(px(12.0))
        .bg(rgb(p.shell_high))
        .border_b_1()
        .border_color(rgb(p.border));

    row = row
        .child(toolbar_metric("toolbar-tp", "TP", "+3.0%", p.blue, 74.6, p))
        .child(toolbar_metric("toolbar-sl", "SL", "-2.0%", p.red, 74.6, p))
        .child(toolbar_metric("toolbar-lev", "Lev", "×1", p.text, 61.6, p))
        .child(divider(p))
        .child(strip_label("size", p))
        .child(size_strip())
        .child(divider(p))
        .child(strip_label("sell", p))
        .child(sell_strip())
        .child(divider(p))
        .child(scale_popover(scale, backend.clone(), p));

    let backend = backend.clone();
    row.child(
        MoonButton::new("live")
            .width(54.0)
            .variant(if follow {
                MoonButtonVariant::Green
            } else {
                MoonButtonVariant::Soft
            })
            .size(MoonButtonSize::Toolbar)
            .selected(follow)
            .label(if follow { "Live" } else { "Пауза" })
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    b.follow = !b.follow;
                    bcx.notify();
                });
            })
            .render(),
    )
}
