//! Торговый тулбар: прикладная сборка терминала поверх MoonPalette.
//!
//! Логика остаётся терминальной: size/sell пока логируют todo, scale/live пишут в
//! `Backend`. Визуальные контролы берём из палитры, выведенной из HTML-эталона.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonDropdown, MoonInput, MoonInputState, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonSlider, MoonSliderState,
    MoonTooltipView, h_flex, v_flex,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use crate::shell::Shell;
use crate::{Backend, design};

/// Границы слайдеров торговых метрик `(min, max, step)` (по смыслу ядра). Использует и
/// `Shell` при создании состояний слайдеров.
pub const TP_NORMAL: (f32, f32, f32) = (1.0, 100.0, 1.0); // x_tmode off: 1..100%
pub const TP_EXT: (f32, f32, f32) = (100.0, 900.0, 10.0); // x_tmode on («s9»): 100..900%
pub const SL_BOUNDS: (f32, f32, f32) = (-20.0, 1.0, 0.01); // знаковый: -20..+1%
pub const LEV_BOUNDS: (f32, f32, f32) = (1.0, 125.0, 1.0);

/// Формат значения с сотыми и запятой-разделителем (локаль): `50` → "50,00".
pub fn fmt_field2(v: f32) -> String {
    format!("{v:.2}").replace('.', ",")
}

/// Со знаком (для SL, который может быть и +, и −): `1` → "+1,00", `-20` → "-20,00".
pub fn fmt_field2_signed(v: f32) -> String {
    format!("{v:+.2}").replace('.', ",")
}

/// Торговая метрика тулбара с собственным попапом (слайдер + поле ввода).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TradeMetric {
    Tp,
    Sl,
    Lev,
}

impl TradeMetric {
    fn id(self) -> &'static str {
        match self {
            TradeMetric::Tp => "toolbar-tp",
            TradeMetric::Sl => "toolbar-sl",
            TradeMetric::Lev => "toolbar-lev",
        }
    }

    fn label(self) -> &'static str {
        match self {
            TradeMetric::Tp => "TP",
            TradeMetric::Sl => "SL",
            TradeMetric::Lev => "Lev",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            TradeMetric::Lev => "×",
            _ => "%",
        }
    }

    fn title(self) -> String {
        match self {
            TradeMetric::Tp => t!("toolbar.tp_title").to_string(),
            TradeMetric::Sl => t!("toolbar.sl_title").to_string(),
            TradeMetric::Lev => t!("toolbar.lev_title").to_string(),
        }
    }

    /// Текущее значение метрики активного ядра (для сидирования слайдера/инпута при открытии).
    /// Lev зависит ОТ ЯДРА И ТЕКУЩЕЙ МОНЕТЫ: плечо рынка main-чарта из ассетов активного ядра.
    pub fn current(self, b: &Backend, group: &str) -> Option<f32> {
        let core = b.active_trade_core(group)?;
        let cd = b.session.store().core(core)?;
        match self {
            TradeMetric::Tp => cd.client_settings.as_ref().map(|s| s.take_profit_pct as f32),
            TradeMetric::Sl => cd.client_settings.as_ref().map(|s| s.stop_loss_pct),
            TradeMetric::Lev => {
                let (_, market) = b.main_chart_target(group)?;
                cd.assets
                    .rows
                    .iter()
                    .find(|r| r.market == market)
                    .map(|r| r.leverage as f32)
            }
        }
    }

}

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

/// Кнопка-триггер торговой метрики. Клик открывает/закрывает её попап в `Shell` (overlay со
/// слайдером/полем; закрытие по клику вне/уводе мыши — как у попапа раскладки чарта, т.к.
/// `MoonPopover` форка не закрывается по клику вне — `appearance(false)`, см. FORK_BUGS).
fn metric_button(
    metric: TradeMetric,
    value_str: String,
    color: u32,
    width: f32,
    open: bool,
    shell: Entity<Shell>,
    p: MoonPalette,
) -> impl IntoElement {
    MoonButton::new(metric.id())
        .width(width)
        .variant(if open {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Neutral
        })
        .size(MoonButtonSize::Toolbar)
        .selected(open)
        .segment(
            MoonButtonSegment::new(metric.label())
                .color(p.text_muted)
                .weight(400.0),
        )
        .text_segment(value_str, color, 500.0)
        .on_click(move |_, window, app| {
            shell.update(app, |this, cx| this.toggle_metric_popup(metric, window, cx));
        })
        .render()
}

/// Контент попапа метрики (overlay-бокс со своим фоном/рамкой): заголовок + слайдер + поле;
/// для TP — ещё чекбокс расширенного диапазона `x_tmode`/«s9». Рисуется `Shell` поверх дока
/// на абсолютной позиции под кнопкой. `slider` уже выбран вызывающим (для TP — обычный/
/// расширенный по `extended`).
#[allow(clippy::too_many_arguments)]
pub fn metric_popup_content(
    metric: TradeMetric,
    slider: &Entity<MoonSliderState>,
    fine_slider: &Entity<MoonSliderState>,
    input: &Entity<MoonInputState>,
    extended: bool,
    hedge_on: bool,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut content = v_flex()
        .w(px(220.0))
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(metric.title()),
        )
        .child(
            MoonSlider::new(slider)
                .id(format!("{}-slider", metric.id()))
                .height(18.0),
        )
        .child(
            h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(
                    div().w(px(72.0)).child(
                        MoonInput::new(SharedString::from(format!("{}-input", metric.id())))
                            .state(input)
                            .small(),
                    ),
                )
                .child(div().text_color(rgb(p.text_muted)).child(metric.unit())),
        );

    if matches!(metric, TradeMetric::Tp) {
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-tp-ext")
                .label(t!("toolbar.tp_ext").to_string())
                .checked(extended)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let ext = *ch;
                    let b = backend.read(app);
                    let Some(core) = b.active_trade_core(&group) else {
                        return;
                    };
                    let cur = b
                        .session
                        .store()
                        .core(core)
                        .and_then(|d| d.client_settings.as_ref())
                        .map(|s| s.take_profit_pct)
                        .unwrap_or(0.0);
                    if let Err(error) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::TakeProfit { pct: cur, extended: ext },
                    ) {
                        log::warn!("tp extended toggle failed: {error}");
                    }
                }),
        );
        // Файн-слайдер: суб-процентный TP (0..основной_TP, шаг 0.01) через scalp. Диапазон
        // задаёт `Shell` пересозданием `fine_slider` при открытии (границы слайдера фикс.).
        content = content
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("toolbar.tp_fine").to_string()),
            )
            .child(
                MoonSlider::new(fine_slider)
                    .id("toolbar-tp-fine-slider")
                    .height(18.0),
            );
    }

    if matches!(metric, TradeMetric::Lev) {
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-hedge")
                .label(t!("toolbar.hedge").to_string())
                .checked(hedge_on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let on = *ch;
                    let b = backend.read(app);
                    let Some(core) = b.active_trade_core(&group) else {
                        return;
                    };
                    if let Err(error) = b.session.set_hedge_mode(core, on) {
                        log::warn!("set hedge mode failed: {error}");
                    }
                }),
        );
    }
    content.into_any_element()
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

/// Ширины кнопок продажи (визуал как был).
const SELL_W: [f32; 6] = [62.0, 62.0, 62.0, 62.0, 56.0, 52.0];

/// Полоса fixed-sell пресетов (S1-S6). Значения — из `ClientSettings` активного ядра
/// (видимые проценты), выбранный пресет подсвечен (`fixed_sell_slot`). Нет ядра/настроек —
/// прочерки. Запись (смена пресета) — Этап 4: пока клик логируется.
fn sell_strip(
    pcts: Option<[f64; 6]>,
    sel_slot: Option<usize>,
    backend: Entity<Backend>,
    group: &str,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let value = match pcts {
                Some(p) => format!("+{:.1}%", p[i]),
                None => "—".to_string(),
            };
            let mut it = MoonSegmentItem::new(SELL_KEYS[i], value).width(SELL_W[i]);
            if sel_slot == Some(i + 1) {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let group = group.to_string();
    MoonSegmentedControl::new("toolbar-sell-presets")
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            // Клик S1-S6 = выбрать fixed-sell слот (1-based) активного ядра.
            backend.update(cx, |b, _| {
                let Some(core) = b.active_trade_core(&group) else {
                    return;
                };
                if let Err(error) = b
                    .session
                    .edit_client_settings(core, ClientSettingsEdit::SelectFixedSellSlot(ix + 1))
                {
                    log::warn!("select fixed-sell slot failed: {error}");
                }
            });
        })
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
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(CoreId, usize)>,
    size_input: &Entity<MoonInputState>,
    shell: &Entity<Shell>,
    open_metric: Option<TradeMetric>,
    cx: &App,
) -> impl IntoElement {
    let (scale, follow, focus_core, size_values, size_sel, tp_str, sl_str, lev_str, sell_pcts, sell_slot) = {
        let b = backend.read(cx);
        // Активное торговое ядро = выбор в селекторе шапки (sticky-override) ИЛИ ядро
        // открытого фуллскрином Main-чарта. Все торговые контролы (размеры/TP/SL/Lev/sell)
        // читают ЕГО. Нет ядра → дефолтные размеры, прочерки, клики игнор.
        let focus_core = b.active_trade_core(group);
        let (size_values, size_sel) = match focus_core {
            Some(core) => b.manual_order_size_state(core),
            None => (
                moon_core::config::servers::default_order_sizes(""),
                SIZE_SEL_DEFAULT,
            ),
        };
        let core_data = focus_core.and_then(|c| b.session.store().core(c));
        let cs = core_data.and_then(|d| d.client_settings.as_ref());
        let tp_str = cs
            .map(|s| format!("{}%", fmt_field2(s.take_profit_pct as f32)))
            .unwrap_or_else(|| "—".to_string());
        // SL знаковый: «+1,00%» / «-20,00%» (а не «--» из ручного минуса перед отрицательным).
        let sl_str = cs
            .map(|s| format!("{}%", fmt_field2_signed(s.stop_loss_pct)))
            .unwrap_or_else(|| "—".to_string());
        let sell_pcts = cs.map(|s| s.fixed_sell_pcts);
        let sell_slot = cs.map(|s| s.fixed_sell_slot);
        // Lev = плечо монеты main-чарта на активном ядре (per-core, per-coin) из ассетов.
        let lev_str = TradeMetric::Lev
            .current(b, group)
            .filter(|l| *l > 0.0)
            .map(|l| format!("×{}", l as i32))
            .unwrap_or_else(|| "—".to_string());
        (
            b.price_scale,
            b.follow,
            focus_core,
            size_values,
            size_sel,
            tp_str,
            sl_str,
            lev_str,
            sell_pcts,
            sell_slot,
        )
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
        .child(metric_button(
            TradeMetric::Tp,
            tp_str,
            p.blue,
            74.6,
            open_metric == Some(TradeMetric::Tp),
            shell.clone(),
            p,
        ))
        .child(metric_button(
            TradeMetric::Sl,
            sl_str,
            p.red,
            74.6,
            open_metric == Some(TradeMetric::Sl),
            shell.clone(),
            p,
        ))
        .child(metric_button(
            TradeMetric::Lev,
            lev_str,
            p.text,
            61.6,
            open_metric == Some(TradeMetric::Lev),
            shell.clone(),
            p,
        ))
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
        .child(sell_strip(sell_pcts, sell_slot, backend.clone(), group))
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
