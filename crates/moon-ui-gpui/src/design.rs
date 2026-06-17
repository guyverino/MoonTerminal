//! MoonBot terminal design tokens extracted from
//! `R:\test\chart-test\for_dis\MoonBot Terminal Design.html`.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.

use gpui::*;
use moon_palette::{MoonMetrics, MoonPalette};
use std::path::PathBuf;

const M: MoonMetrics = MoonMetrics::TERMINAL;

pub const HEADER_TOP_H: f32 = M.header_top_h;
pub const TOOLBAR_H: f32 = M.toolbar_h;
pub const STATUS_H: f32 = M.status_h;
pub const TABLE_HEAD_H: f32 = M.table_header_h;
pub const TABLE_ROW_H: f32 = M.table_row_h;
pub const HEADER_PAD_X: f32 = 12.0;

/// Transparent macOS titlebars keep native traffic-light buttons over the client
/// area. Keep terminal chrome content and drag hitboxes out of that strip.
pub fn titlebar_leading_inset() -> f32 {
    if cfg!(target_os = "macos") {
        76.0
    } else {
        HEADER_PAD_X
    }
}

pub fn show_custom_window_controls() -> bool {
    !cfg!(target_os = "macos")
}

pub const LOGO_SVG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/brand/moonbot-logo.svg"
);

pub fn solid(hex: u32) -> Rgba {
    rgb(hex)
}

pub fn alpha(hex: u32, a: u32) -> Rgba {
    rgba((hex << 8) | (a & 0xff))
}

pub fn mono() -> SharedString {
    SharedString::from("Geist Mono")
}

pub fn ui_font() -> SharedString {
    SharedString::from("Inter")
}

pub fn logo() -> impl IntoElement {
    img(PathBuf::from(LOGO_SVG)).w(px(83.3)).h(px(18.0))
}

pub fn vline(height: f32, p: MoonPalette) -> impl IntoElement {
    div().w(px(1.0)).h(px(height)).bg(rgb(p.border))
}

pub fn top_pill(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    p: MoonPalette,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(24.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .text_size(px(11.0))
        .font_family(mono())
        .text_color(rgb(p.text_soft))
        .child(label.into())
}

pub fn status_dot(color: u32) -> impl IntoElement {
    div()
        .w(px(5.0))
        .h(px(5.0))
        .rounded(px(999.0))
        .bg(solid(color))
}
