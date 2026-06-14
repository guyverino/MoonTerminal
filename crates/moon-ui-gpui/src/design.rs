//! MoonBot terminal design tokens extracted from
//! `R:\test\chart-test\for_dis\MoonBot Terminal Design.html`.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.

use gpui::*;
use moon_palette::{MoonMetrics, MoonPalette};

const P: MoonPalette = MoonPalette::TERMINAL;
const M: MoonMetrics = MoonMetrics::TERMINAL;

pub const HEADER_TOP_H: f32 = M.header_top_h;
pub const TOOLBAR_H: f32 = M.toolbar_h;
pub const STATUS_H: f32 = M.status_h;
pub const TABLE_HEAD_H: f32 = M.table_header_h;
pub const TABLE_ROW_H: f32 = M.table_row_h;

pub const HEADER: u32 = P.shell_high;
pub const PANEL: u32 = P.panel;
pub const PANEL_DARK: u32 = 0x17191C;
pub const LIFT: u32 = 0x1F2126;
pub const LIFT_HOVER: u32 = 0x26282D;
pub const BORDER: u32 = P.border;
pub const TEXT: u32 = P.text;
pub const TEXT_SOFT: u32 = P.text_soft;
pub const TEXT_MUTED: u32 = P.text_muted;
pub const GREEN: u32 = P.green;
pub const ORANGE: u32 = P.orange;

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

pub fn logo() -> Svg {
    svg().external_path(LOGO_SVG).w(px(83.0)).h(px(18.0))
}

pub fn vline(height: f32) -> impl IntoElement {
    div().w(px(1.0)).h(px(height)).bg(solid(BORDER))
}

pub fn top_pill(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(24.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(solid(BORDER))
        .bg(solid(PANEL))
        .text_size(px(11.0))
        .font_family(mono())
        .text_color(solid(TEXT_SOFT))
        .child(label.into())
}

pub fn status_dot(color: u32) -> impl IntoElement {
    div().w(px(5.0)).h(px(5.0)).rounded(px(999.0)).bg(solid(color))
}
