//! Попап настроек раскладки чарт-вкладки: режим (FIT/SCROLL/COMPRESS) + высота слота.
//! Per-tab (заменил глобальные настройки). Рендер общий для полоски вкладок главного окна и
//! шапки выносного окна; обработчики (применение к нужному стеку + persist) задаёт вызывающий.

use gpui::*;
use moon_ui::{
    MoonAccent, MoonInput, MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl,
    h_flex, v_flex,
};
use rust_i18n::t;

use crate::chart_persist::StackLayoutMode;
use crate::design;

/// Порядок режимов в сегмент-контроле попапа.
pub(super) const POPUP_MODES: [StackLayoutMode; 3] = [
    StackLayoutMode::Fit,
    StackLayoutMode::Scroll,
    StackLayoutMode::Compress,
];

fn mode_label(m: StackLayoutMode) -> &'static str {
    match m {
        StackLayoutMode::Fit => "FIT",
        StackLayoutMode::Scroll => "SCROLL",
        StackLayoutMode::Compress => "COMPRESS",
    }
}

/// Маленькое окошко настроек раскладки. `current` — выбранный режим (per-tab или дефолт),
/// `height_input` — поле высоты (подписку на Blur/Enter держит вызывающий). `on_pick_mode`
/// вызывается при выборе режима. Позиционируется вызывающим (он оборачивает в `.absolute()`).
pub(super) fn render_layout_popup<F>(
    id: &str,
    current: StackLayoutMode,
    height_input: &Entity<MoonInputState>,
    p: MoonPalette,
    cx: &App,
    on_pick_mode: F,
) -> AnyElement
where
    F: Fn(StackLayoutMode, &mut App) + 'static,
{
    let sel = POPUP_MODES.iter().position(|m| *m == current).unwrap_or(0);
    let items: Vec<MoonSegmentItem> = POPUP_MODES
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut it = MoonSegmentItem::new("", mode_label(*m)).width(82.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(format!("{id}-mode"))
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(m) = POPUP_MODES.get(ix) {
                on_pick_mode(*m, cx);
            }
        })
        .render();

    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w(px(280.0))
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(px(6.0))
        .shadow_md()
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart.layout.title").to_string()),
        )
        .child(seg)
        .child(
            h_flex()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(p.text_soft))
                        .child(t!("chart.layout.height").to_string()),
                )
                .child(
                    div()
                        .w(px(90.0))
                        .child(MoonInput::new(format!("{id}-height")).state(height_input).small()),
                ),
        )
        .into_any_element()
}
