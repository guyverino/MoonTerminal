//! Попап настроек раскладки чарт-вкладки как ОТДЕЛЬНОЕ безрамочное ОС-окно (`WindowKind::PopUp`).
//!
//! Раньше попап был in-scene оверлеем — но own-pass рисует оси/readout ПОВЕРХ всей сцены GPUI
//! отдельной фазой рендера, и z-order GPUI на неё не влияет → текст осей просвечивал сквозь
//! попап. Отдельное ОС-окно решает это композитором ОС: оно поверх swapchain чарта без всяких
//! вырезаний зон и правок форка. Кроссплатформенно — только gpui-API (`WindowKind::PopUp`,
//! `observe_window_activation`, `remove_window`); каждый бэкенд сам делает окно безрамочным.
//!
//! Авто-закрытие — по потере фокуса окна (как настоящий popover): клик в любое другое окно
//! приложения снимает фокус → окно закрывается. Владелец (ChartTabs/DetachedChartHost) держит
//! только `WindowHandle` и зовёт `apply`/`closed`-колбэки.

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, MoonInputEvent, MoonInputState, MoonPalette, Root};

use super::layout_popup::{self, render_layout_popup};
use crate::chart_persist::StackLayoutMode;
use crate::design;

/// Применить раскладку (режим + раздельные высоты Fit/Scroll) к владельцу вкладки.
pub(super) type ApplyFn = Rc<dyn Fn(StackLayoutMode, Option<u16>, Option<u16>, &mut App)>;
/// Уведомить владельца, что окно попапа закрылось (сбросить handle, перерисовать кнопку ⚙).
pub(super) type ClosedFn = Rc<dyn Fn(&mut App)>;
/// Вкл/выкл стакан для вкладки (per-окно).
pub(super) type OrderbookFn = Rc<dyn Fn(bool, &mut App)>;

/// Размер окна-поповера (логич. px), посчитанный ДЕТЕРМИНИРОВАННО из метрик и масштаба слайдера
/// «Шрифт» (`design::ui_px`). Контент заполняет окно (`size_full`).
///
/// Почему не «авто по содержимому через `window.resize` каждый кадр»: на НЕ-первичном мониторе с
/// другим DPI каждый resize в этом форке gpui повторно триггерит `WM_DPICHANGED`-перемасштаб →
/// размер компаундится → окно безостановочно растёт (тот же баг, из-за которого существует
/// `DetachedChartHost.restore_size`). Поэтому размер считаем заранее и больше не трогаем.
///
/// Высоту берём под БОЛЬШИЙ режим (Fit — 2 строки примечания), чтобы Scroll (1 строка) лишь оставил
/// незаметный отступ снизу (фон тот же), а не обрезался. Ширину диктует сегмент-контрол (2×110).
pub(super) fn content_size(cx: &App) -> Size<Pixels> {
    let pad = f32::from(design::ui_px(cx, 8.0));
    let gap = f32::from(design::ui_px(cx, 8.0));
    let cap = f32::from(design::t_caption(cx)) + 6.0; // строка заголовка/примечания
    // Заголовок содержит иконку Micro (⧉) справа — строка чуть выше кегля.
    let title_h = cap.max(f32::from(design::ui_px(cx, 22.0)));
    let seg_h = f32::from(design::ui_px(cx, 30.0)); // сегмент-контрол Fit/Scroll
    let line_h = f32::from(design::ui_px(cx, 30.0)); // строка «Высота … [поле] px»
    let cb_h = f32::from(design::ui_px(cx, 22.0)); // чекбокс «Стакан»
    let border = 2.0;
    // title(с иконкой) + seg + height_line + hint(2 строки) + чекбокс, с гэпами + паддинг + рамка.
    let h = border + 2.0 * pad + title_h + gap + seg_h + gap + line_h + gap + 2.0 * cap + gap + cb_h + 6.0;
    // 2×110 сегмент + внутр. отступы/гэпы + паддинг + рамка.
    let w = 2.0 * 110.0 + 20.0 + 2.0 * pad + border;
    size(px(w), px(h))
}

/// Clear-цвет окна-поповера — ПРОЗРАЧНЫЙ: окно полупрозрачное (`WindowBackgroundAppearance::
/// Transparent`), сам фон (panel_high с alpha) даёт контент (`render_layout_popup`), а не закрытые
/// им пиксели остаются прозрачными. `cx` не нужен, оставлен для единообразия сигнатуры.
pub(super) fn clear_color(_cx: &App) -> Rgba {
    rgba(0x00000000)
}

/// Открыть окно-поповер на экранной точке `origin` размером `win_size`. Handle или None (отказ ОС).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub(super) fn open(
    origin: Point<Pixels>,
    win_size: Size<Pixels>,
    display_id: Option<DisplayId>,
    place_phys: crate::windowing::PopupPhysRect,
    clear_color: Rgba,
    mode: StackLayoutMode,
    height_fit: Option<u16>,
    height_scroll: Option<u16>,
    orderbook_enabled: bool,
    apply: ApplyFn,
    apply_all: ApplyFn,
    apply_all_label: SharedString,
    on_toggle_orderbook: OrderbookFn,
    closed: ClosedFn,
    cx: &mut App,
) -> Option<WindowHandle<Root>> {
    let bounds = Bounds {
        origin,
        size: win_size,
    };
    let mut opts = crate::windowing::popup_window_options(WindowBounds::Windowed(bounds), display_id);
    opts.window_clear_color = Some(clear_color);
    cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| {
            LayoutPopupWindow::new(
                mode,
                height_fit,
                height_scroll,
                orderbook_enabled,
                place_phys,
                apply,
                apply_all,
                apply_all_label,
                on_toggle_orderbook,
                closed,
                window,
                cx,
            )
        });
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    })
    .ok()
}

/// Содержимое окна-поповера: сегмент Fit/Scroll + поле высоты активного режима. Состояние
/// (режим + значения полей) живёт здесь; наружу только зовём `apply`. Поля высоты — свои,
/// создаются при открытии с текущими значениями.
pub(super) struct LayoutPopupWindow {
    mode: StackLayoutMode,
    fit_input: Entity<MoonInputState>,
    scroll_input: Entity<MoonInputState>,
    /// Высоты на момент открытия — фолбэк, если поле пустое/нечисловое (чтобы пустой ввод не сбросил
    /// сохранённое значение в дефолт).
    init_fit: Option<u16>,
    init_scroll: Option<u16>,
    apply: ApplyFn,
    /// Применить текущую раскладку КО ВСЕМ (область задаёт владелец: ко всем окнам / только чартам).
    apply_all: ApplyFn,
    /// Подпись кнопки «применить ко всем» (зависит от области).
    apply_all_label: SharedString,
    /// Состояние чекбокса «Стакан» + колбэк применения к вкладке.
    orderbook_enabled: bool,
    on_toggle_orderbook: OrderbookFn,
    closed: ClosedFn,
    /// Физ. screen-rect для корректирующего `SetWindowPos` на первом рендере (open_window кладёт
    /// мимо на смещённых мониторах). Применяется один раз → сбрасывается в None.
    place_phys: crate::windowing::PopupPhysRect,
    /// Окно уже было активным хотя бы раз — иначе первый (немедленный) тик наблюдателя активации
    /// закрыл бы попап до того, как ОС успела дать ему фокус.
    was_active: bool,
    /// Курсор уже был внутри окна — закрываем по уходу ТОЛЬКО после первого входа (при открытии
    /// курсор ещё на кнопке ⚙ в другом окне, не на попапе).
    was_hovered: bool,
    /// Закрытие уже инициировано — не дёргать `closed`/`remove_window` повторно.
    closing: bool,
    _subs: Vec<Subscription>,
}

impl LayoutPopupWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        mode: StackLayoutMode,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        orderbook_enabled: bool,
        place_phys: crate::windowing::PopupPhysRect,
        apply: ApplyFn,
        apply_all: ApplyFn,
        apply_all_label: SharedString,
        on_toggle_orderbook: OrderbookFn,
        closed: ClosedFn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        if let Some(h) = height_fit {
            fit_input.update(cx, |s, c| s.set_value(h.to_string(), window, c));
        }
        if let Some(h) = height_scroll {
            scroll_input.update(cx, |s, c| s.set_value(h.to_string(), window, c));
        }
        let mut subs = Vec::new();
        // Поля высоты: Blur (клик вне) / Enter → клампим и применяем (берём оба значения из полей).
        subs.push(cx.subscribe(
            &fit_input,
            |this, inp, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    this.commit(cx);
                    let _ = inp;
                }
            },
        ));
        subs.push(cx.subscribe(
            &scroll_input,
            |this, inp, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    this.commit(cx);
                    let _ = inp;
                }
            },
        ));
        // Авто-закрытие по потере фокуса окна.
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                this.close(window, cx);
            }
        }));
        Self {
            mode,
            fit_input,
            scroll_input,
            init_fit: height_fit,
            init_scroll: height_scroll,
            apply,
            apply_all,
            apply_all_label,
            orderbook_enabled,
            on_toggle_orderbook,
            closed,
            place_phys,
            was_active: false,
            was_hovered: false,
            closing: false,
            _subs: subs,
        }
    }

    /// Прочитать высоту из поля режима (клампится по правилам режима; пусто/мусор → init-фолбэк,
    /// чтобы не сбросить сохранённое значение).
    fn read_height(&self, mode: StackLayoutMode, cx: &App) -> Option<u16> {
        let (inp, init) = match mode {
            StackLayoutMode::Fit => (&self.fit_input, self.init_fit),
            StackLayoutMode::Scroll => (&self.scroll_input, self.init_scroll),
        };
        match inp.read(cx).value().to_string().trim().parse::<u16>() {
            Ok(raw) => Some(layout_popup::clamp_height(mode, raw)),
            Err(_) => init,
        }
    }

    /// Применить текущее состояние (режим + обе высоты из полей) к владельцу.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let hf = self.read_height(StackLayoutMode::Fit, cx);
        let hs = self.read_height(StackLayoutMode::Scroll, cx);
        let app: &mut App = cx;
        (self.apply)(self.mode, hf, hs, app);
    }

    /// Применить текущее состояние КО ВСЕМ (область задаёт колбэк владельца).
    fn apply_all_now(&mut self, cx: &mut Context<Self>) {
        let hf = self.read_height(StackLayoutMode::Fit, cx);
        let hs = self.read_height(StackLayoutMode::Scroll, cx);
        let app: &mut App = cx;
        (self.apply_all)(self.mode, hf, hs, app);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        // Зафиксировать текущие значения полей (на случай несохранённого ввода без Blur).
        self.commit(cx);
        let app: &mut App = cx;
        (self.closed)(app);
        window.remove_window();
    }
}

impl Render for LayoutPopupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Корректирующая постановка по абсолютным screen-px (один раз): open_window на смещённых
        // мониторах кладёт окно мимо. Делаем в render (а не в new) — после того как open завершил
        // своё размещение, чтобы перебить его.
        if let Some(rect) = self.place_phys.take() {
            crate::windowing::move_window_to_physical(window, rect);
        }
        let p = MoonPalette::active(cx);
        let entity = cx.entity();
        let entity_all = cx.entity();
        let entity_ob = cx.entity();
        let body = render_layout_popup(
            "chart-layout",
            self.mode,
            &self.fit_input,
            &self.scroll_input,
            self.orderbook_enabled,
            p,
            cx,
            move |mode, app| {
                entity.update(app, |this, cx| {
                    this.mode = mode;
                    this.commit(cx);
                    cx.notify();
                });
            },
            self.apply_all_label.to_string(),
            move |app| {
                entity_all.update(app, |this, cx| this.apply_all_now(cx));
            },
            move |checked, app| {
                entity_ob.update(app, |this, cx| {
                    this.orderbook_enabled = checked;
                    let app: &mut App = cx;
                    (this.on_toggle_orderbook)(checked, app);
                    cx.notify();
                });
            },
        );
        // size_full-обёртка с hover: уход курсора с окна (после первого входа) закрывает попап.
        // Это В ДОПОЛНЕНИЕ к закрытию по потере фокуса (клик в другое окно). Контент (body)
        // заполняет окно; размер окна задан заранее (content_size), resize не делаем.
        div()
            .id("chart-layout-popup-root")
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if *hovered {
                    this.was_hovered = true;
                } else if this.was_hovered {
                    this.close(window, cx);
                }
            }))
            .child(body)
    }
}
