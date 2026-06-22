//! Торговый тулбар: прикладная сборка терминала поверх MoonPalette.
//!
//! Логика остаётся терминальной: size/sell пока логируют todo, scale/live пишут в
//! `Backend`. Визуальные контролы берём из палитры, выведенной из HTML-эталона.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonDropdown,
    MoonInput, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, MoonTooltipView, h_flex,
};

use moon_core::session::CoreId;

use crate::{Backend, design};

/// Высота полосы тулбара: 2-я строка header из HTML-эталона.
pub const TOOLBAR_H: f32 = design::TOOLBAR_H;

/// Пресеты масштаба цены (Y) — 1:1 с egui `dock/controls.rs::SCALES`. `None` = «Авто».
const SCALES: [(&str, Option<f32>); 6] = [
    ("Авто", None),
    ("50%", Some(0.50)),
    ("20%", Some(0.20)),
    ("10%", Some(0.10)),
    ("5%", Some(0.05)),
    ("2%", Some(0.02)),
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
fn strip_label(text: &'static str, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .text_size(design::t_caption(cx))
        .font_family(design::ui_font())
        .text_color(rgb(p.text_muted))
        .child(text)
}

/// Вертикальный разделитель групп (стендовый `.divider`): тонкая линия высотой 16px.
fn divider(p: MoonPalette) -> impl IntoElement {
    design::vline(16.0, p)
}

/// Ширины кнопок размера (как в исходном тулбаре) — визуал не меняем.
const SIZE_W: [f32; 6] = [54.0, 61.0, 56.0, 56.0, 56.0, 56.0];

/// Дефолтный выбранный пресет размера (F3), когда у ядра ещё нет своего выбора.
const SIZE_SEL_DEFAULT: usize = 2;

/// Компактная подпись значения размера: целые без дроби (USDT: "1000"), дробные —
/// без хвостовых нулей (BTC: "0.05").
fn fmt_size(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Полоса пресетов размера ордера (F1-F6). Значения — из конфига ядра (или дефолт по
/// базе BTC/USDT), выбор хранится per-core в `Backend::order_size_sel`. Одиночный клик —
/// выбор; дабл-клик — запрос инлайн-редактирования значения (`order_size_edit_req`), Shell
/// открывает инпут поверх кнопки. `edit_ix` — индекс редактируемой сейчас кнопки (рисуем
/// поверх неё `input`). `core=None` (нет ядра группы) → клики игнорируются.
fn size_strip(
    values: [f64; 6],
    sel: usize,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let mut it = MoonSegmentItem::new(SIZE_KEYS[i], fmt_size(values[i])).width(SIZE_W[i]);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-size-presets")
        .accent(MoonAccent::Amber)
        .items(items)
        .on_click(move |ix, event, _w, cx| {
            let Some(core) = core else {
                return;
            };
            // Дабл-клик → редактирование значения кнопки; одиночный → выбор пресета.
            let dbl = matches!(event, ClickEvent::Mouse(m) if m.up.click_count >= 2);
            backend.update(cx, |b, bcx| {
                if dbl {
                    b.order_size_edit_req = Some((core, ix));
                } else {
                    b.order_size_sel.insert(core, ix);
                }
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
            });
        })
        .render();

    let mut root = div().relative().flex().items_center().child(seg);
    // Инпут поверх редактируемой кнопки (absolute по сумме ширин предыдущих).
    if let Some(ix) = edit_ix.filter(|i| *i < 6) {
        let left: f32 = SIZE_W.iter().take(ix).sum();
        root = root.child(
            div()
                .absolute()
                .left(px(left))
                .top(px(0.0))
                .w(px(SIZE_W[ix]))
                .h_full()
                .child(MoonInput::new("toolbar-size-edit").state(input).small()),
        );
    }
    root
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

fn scale_label(scale: Option<f32>) -> &'static str {
    SCALES
        .iter()
        .find(|(_, value)| *value == scale)
        .map(|(label, _)| *label)
        .unwrap_or("Авто")
}

pub(crate) fn scale_dropdown(
    scale: Option<f32>,
    group: &str,
    backend: Entity<Backend>,
    p: MoonPalette,
) -> impl IntoElement {
    let selected_label = scale_label(scale);
    let mut items = Vec::with_capacity(SCALES.len());
    for (label, pct) in SCALES {
        let backend = backend.clone();
        let group = group.to_string();
        items.push(
            MoonMenuItem::with_key(format!("scale-{label}"), label)
                .selected(scale == pct)
                .checked(scale == pct)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        // Масштаб ПО-ВКЛАДОЧНЫЙ: тулбар лишь запрашивает (++rev) — ChartTabs
                        // применит к АКТИВНОЙ панели. price_scale тут = желаемое значение.
                        b.price_scale = pct;
                        b.price_scale_group = Some(group.clone());
                        b.price_scale_rev = b.price_scale_rev.wrapping_add(1);
                        bcx.notify();
                    });
                }),
        );
    }

    // Лупа вместо слова «МАСШТАБ» + «А» для Авто (компактнее); подсказка «Масштаб» — тултипом.
    let trigger_val = if scale.is_none() {
        "А"
    } else {
        selected_label
    };
    div()
        .id("toolbar-scale-tip")
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("toolbar.scale").to_string()))
                .into()
        })
        .child(
            MoonDropdown::new("toolbar-scale-dropdown")
                .trigger_width(72.0)
                .trigger_variant(MoonButtonVariant::Neutral)
                .trigger_size(MoonButtonSize::Toolbar)
                .menu_width(116.0)
                .menu_size(MoonMenuSize::Compact)
                .segment(
                    MoonButtonSegment::new("🔍")
                        .color(p.text_muted)
                        .weight(400.0),
                )
                .segment(
                    MoonButtonSegment::new(trigger_val)
                        .color(p.text)
                        .weight(500.0),
                )
                .items(items),
        )
}

/// Дропдаун масштаба для AddToChart-stack: пишет масштаб во все отдельные ChartPanel внутри
/// stack-а. Это сохраняет Delphi-модель "один график = одна сущность", но управление масштабом
/// остаётся единым для окна/вкладки.
pub(crate) fn scale_dropdown_for_add_stack(
    scale: Option<f32>,
    stack: Entity<crate::chart_tabs::AddChartStack>,
    p: MoonPalette,
) -> impl IntoElement {
    let selected_label = scale_label(scale);
    let mut items = Vec::with_capacity(SCALES.len());
    for (label, pct) in SCALES {
        let stack = stack.clone();
        items.push(
            MoonMenuItem::with_key(format!("scale-stack-{label}"), label)
                .selected(scale == pct)
                .checked(scale == pct)
                .on_click(move |_, _, cx| {
                    stack.update(cx, |st, scx| st.set_scale(pct, scx));
                }),
        );
    }

    // Лупа вместо слова «МАСШТАБ» + «А» для Авто (компактнее); подсказка «Масштаб» — тултипом.
    let trigger_val = if scale.is_none() {
        "А"
    } else {
        selected_label
    };
    div()
        .id("detached-stack-scale-tip")
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("toolbar.scale").to_string()))
                .into()
        })
        .child(
            MoonDropdown::new("detached-stack-scale-dropdown")
                .trigger_width(72.0)
                .trigger_variant(MoonButtonVariant::Neutral)
                .trigger_size(MoonButtonSize::Toolbar)
                .menu_width(116.0)
                .menu_size(MoonMenuSize::Compact)
                .segment(
                    MoonButtonSegment::new("🔍")
                        .color(p.text_muted)
                        .weight(400.0),
                )
                .segment(
                    MoonButtonSegment::new(trigger_val)
                        .color(p.text)
                        .weight(500.0),
                )
                .items(items),
        )
}

/// Полоса тулбара: рисуется как обычный child `Shell` (между шапкой и доком), не dock-панель.
/// Читает текущий масштаб/follow из `backend`, клики пишут обратно (+notify → перерисовка).
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(CoreId, usize)>,
    size_input: &Entity<MoonInputState>,
    cx: &App,
) -> impl IntoElement {
    let (scale, follow, focus_core, size_values, size_sel) = {
        let b = backend.read(cx);
        // Фокусное ядро = ядро ОТКРЫТОГО ФУЛСКРИНОМ Main-чарта этой группы. Размеры показываем
        // и редактируем для него (открыл монету на байбите → размеры байбита, на бинансе →
        // бинанса). Нет открытого фулскрина → нет ядра (дефолтные значения, клики игнор).
        let focus_core = b.main_chart_target(group).map(|(core, _)| core);
        let (size_values, size_sel) = match focus_core {
            Some(core) => {
                let base = b.session.core_base(core).unwrap_or("");
                let sizes = b
                    .config
                    .servers
                    .iter()
                    .find(|s| s.id == core)
                    .map(|s| s.order_sizes_or_default(base))
                    .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(base));
                let sel = b
                    .order_size_sel
                    .get(&core)
                    .copied()
                    .unwrap_or(SIZE_SEL_DEFAULT)
                    .min(5);
                (sizes, sel)
            }
            None => (
                moon_core::config::servers::default_order_sizes(""),
                SIZE_SEL_DEFAULT,
            ),
        };
        (b.price_scale, b.follow, focus_core, size_values, size_sel)
    };
    let p = MoonPalette::active(cx);

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(design::fit_h_px(cx, TOOLBAR_H, 13.0, 9.5))
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .bg(rgb(p.shell_high))
        .border_b_1()
        .border_color(rgb(p.border));

    row = row
        .child(toolbar_metric("toolbar-tp", "TP", "+3.0%", p.blue, 74.6, p))
        .child(toolbar_metric("toolbar-sl", "SL", "-2.0%", p.red, 74.6, p))
        .child(toolbar_metric("toolbar-lev", "Lev", "×1", p.text, 61.6, p))
        .child(divider(p))
        .child(strip_label("size", p, cx))
        .child(size_strip(
            size_values,
            size_sel,
            // Редактируем инпутом только если запрос относится к ФОКУСНОМУ ядру тулбара.
            size_edit
                .filter(|(c, _)| Some(*c) == focus_core)
                .map(|(_, i)| i),
            size_input,
            backend.clone(),
            focus_core,
        ))
        .child(divider(p))
        .child(strip_label("sell", p, cx))
        .child(sell_strip())
        .child(divider(p))
        .child(scale_dropdown(scale, group, backend.clone(), p));

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
            .label(if follow {
                t!("toolbar.live").to_string()
            } else {
                t!("toolbar.pause").to_string()
            })
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    b.follow = !b.follow;
                    bcx.notify();
                });
            })
            .render(),
    )
}
