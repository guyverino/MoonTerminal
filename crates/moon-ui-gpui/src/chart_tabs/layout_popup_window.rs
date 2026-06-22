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

/// Применить раскладку (режим + раздельные высоты Fit/Scroll) к владельцу вкладки.
pub(super) type ApplyFn = Rc<dyn Fn(StackLayoutMode, Option<u16>, Option<u16>, &mut App)>;
/// Уведомить владельца, что окно попапа закрылось (сбросить handle, перерисовать кнопку ⚙).
pub(super) type ClosedFn = Rc<dyn Fn(&mut App)>;

/// Логический размер окошка (безрамочное). Контент заполняет окно целиком (без «рамки в рамке»);
/// высота с запасом под примечание. При сильном увеличении шрифта контент может не влезть.
pub(super) const POPUP_W: f32 = 300.0;
pub(super) const POPUP_H: f32 = 154.0;

/// Цвет фона (clear) окна-поповера под активную тему = панельный `panel_high`. Лишние поля окна
/// за контентом заливаются им, чтобы окно выглядело сплошной плашкой попапа.
pub(super) fn clear_color(cx: &App) -> Rgba {
    let hex = MoonPalette::active(cx).panel_high;
    rgba((hex << 8) | 0xFF)
}

/// Открыть окно-поповер на экранной точке `origin`. Возвращает handle (None при отказе ОС).
#[allow(clippy::too_many_arguments)]
pub(super) fn open(
    origin: Point<Pixels>,
    display_id: Option<DisplayId>,
    clear_color: Rgba,
    mode: StackLayoutMode,
    height_fit: Option<u16>,
    height_scroll: Option<u16>,
    apply: ApplyFn,
    closed: ClosedFn,
    cx: &mut App,
) -> Option<WindowHandle<Root>> {
    let bounds = Bounds {
        origin,
        size: size(px(POPUP_W), px(POPUP_H)),
    };
    let mut opts = crate::windowing::popup_window_options(WindowBounds::Windowed(bounds), display_id);
    opts.window_clear_color = Some(clear_color);
    cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| {
            LayoutPopupWindow::new(mode, height_fit, height_scroll, apply, closed, window, cx)
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
    closed: ClosedFn,
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
    fn new(
        mode: StackLayoutMode,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        apply: ApplyFn,
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
            closed,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let entity = cx.entity();
        let body = render_layout_popup(
            "chart-layout",
            self.mode,
            &self.fit_input,
            &self.scroll_input,
            p,
            cx,
            move |mode, app| {
                entity.update(app, |this, cx| {
                    this.mode = mode;
                    this.commit(cx);
                    cx.notify();
                });
            },
        );
        // size_full-обёртка с hover: уход курсора с окна (после первого входа) закрывает попап.
        // Это В ДОПОЛНЕНИЕ к закрытию по потере фокуса (клик в другое окно).
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
