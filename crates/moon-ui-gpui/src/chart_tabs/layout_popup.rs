//! Попап настроек раскладки чарт-вкладки: режим (Fit/Scroll) + высота ТОЛЬКО активного режима.
//! Per-tab. Рендер общий для полоски вкладок главного окна и шапки выносного окна; обработчики
//! (применение к нужному стеку + persist) задаёт вызывающий.
//!
//! Семантика: Fit=0 → растяжение (делят окно); Fit≥20 → COMPRESS (фикс. высота без скролла);
//! Scroll → фикс. высота слота + скролл. Допустимый диапазон высоты — [MIN_H, MAX_H].

use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonInputState,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use crate::chart_persist::StackLayoutMode;
use crate::design;

/// Порядок режимов в сегмент-контроле попапа (два положения).
pub(super) const POPUP_MODES: [StackLayoutMode; 2] = [StackLayoutMode::Fit, StackLayoutMode::Scroll];

/// Границы высоты слота (px). Меньше MIN (кроме 0 у Fit = растяжение) и больше MAX вводить нельзя.
pub(super) const MIN_H: u16 = 20;
pub(super) const MAX_H: u16 = 4000;

fn mode_label(m: StackLayoutMode) -> &'static str {
    match m {
        StackLayoutMode::Fit => "FIT",
        StackLayoutMode::Scroll => "SCROLL",
    }
}

/// Маленькое окошко настроек раскладки. Показывает поле высоты ТОЛЬКО для текущего режима.
/// `height_fit_input`/`height_scroll_input` — раздельные поля (подписку на Blur/Enter держит
/// вызывающий). `on_pick_mode` вызывается при выборе режима. Позиционируется вызывающим.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_layout_popup<F, G>(
    id: &str,
    current: StackLayoutMode,
    height_fit_input: &Entity<MoonInputState>,
    height_scroll_input: &Entity<MoonInputState>,
    p: MoonPalette,
    cx: &App,
    on_pick_mode: F,
    apply_all_label: String,
    on_apply_all: G,
) -> AnyElement
where
    F: Fn(StackLayoutMode, &mut App) + 'static,
    G: Fn(&mut App) + 'static,
{
    let sel = POPUP_MODES.iter().position(|m| *m == current).unwrap_or(0);
    let items: Vec<MoonSegmentItem> = POPUP_MODES
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut it = MoonSegmentItem::new("", mode_label(*m)).width(110.0);
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

    // Поле + примечание — только для активного режима.
    let (input, label, hint) = match current {
        StackLayoutMode::Fit => (
            height_fit_input,
            t!("chart.layout.height_fit").to_string(),
            t!("chart.layout.height_fit_hint").to_string(),
        ),
        StackLayoutMode::Scroll => (
            height_scroll_input,
            t!("chart.layout.height_scroll").to_string(),
            t!("chart.layout.height_scroll_hint").to_string(),
        ),
    };
    // "Высота X  [поле]  px"
    let height_line = h_flex()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .child(div().text_color(rgb(p.text)).child(label))
        .child(
            div()
                .w(px(64.0))
                .child(MoonInput::new(SharedString::from(format!("{id}-input"))).state(input).small()),
        )
        .child(div().text_color(rgb(p.text_muted)).child("px"));
    // Примечание под полем (многострочное по '\n').
    let hint_block = v_flex().children(hint.split('\n').map(|line| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(line.to_string())
    }));

    // Кнопка «применить ко всем» (подпись задаёт вызывающий по области действия: ко всем окнам /
    // только чартам). Действие — `on_apply_all`.
    let apply_all_btn = MoonButton::new(SharedString::from(format!("{id}-apply-all")))
        .label(apply_all_label)
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Ghost)
        .on_click(move |_, _w, app| on_apply_all(app))
        .render();

    // Контент заполняет всё окно-поповер (его размер считается детерминированно в
    // layout_popup_window::content_size). Рамка = border_1 (один кант поверх чарта); БЕЗ
    // rounded/shadow/фикс-ширины — окно прямоугольное, контент = всё окно.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .size_full()
        // Фон с alpha ~0.8 (0xCC) — окно-поповер полупрозрачное (см. popup_window_options).
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgba((p.panel_high << 8) | 0xCC))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart.layout.title").to_string()),
        )
        .child(seg)
        .child(height_line)
        .child(hint_block)
        .child(apply_all_btn)
        .into_any_element()
}

/// Клампинг введённой высоты: Fit допускает 0 (растяжение), иначе [MIN_H, MAX_H]; Scroll — всегда
/// [MIN_H, MAX_H].
pub(super) fn clamp_height(mode: StackLayoutMode, raw: u16) -> u16 {
    match mode {
        StackLayoutMode::Fit if raw == 0 => 0,
        _ => raw.clamp(MIN_H, MAX_H),
    }
}
