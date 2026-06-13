//! Окно настроек (порт egui `src/settings/*` + `window/settings_window.rs`).
//! Отдельное ОС-окно, редактирует ЖИВОЙ `Backend.config`: правки темы применяются
//! к чарту сразу (группы-окна читают config каждый кадр и пере-рендерят offscreen),
//! «Сохранить» пишет на диск (`AppConfig::save`). Вкладки: Подключения/Общие/
//! Интерфейс/Линии. Сейчас реализована «Интерфейс» (тема), остальные — заглушки.

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    slider::{Slider, SliderEvent, SliderState},
    v_flex, Root, StyledExt,
};

use crate::{hex, Backend};
use moon_core::config::{ChartTheme, FeedFlags, Language, OrdersStyle, Secret, ServerConfig};
use moon_core::market::MarketDataMode;
use moon_core::palette;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connections,
    General,
    Interface,
    Lines,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Connections, Tab::General, Tab::Interface, Tab::Lines];
    fn title(self) -> &'static str {
        match self {
            Tab::Connections => "Подключения",
            Tab::General => "Общие",
            Tab::Interface => "Интерфейс",
            Tab::Lines => "Линии",
        }
    }
}

/// Hsla (из color-picker) → sRGB [u8;3] для ChartTheme.
fn hsla_u8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

/// Color-picker, привязанный к полю темы: init из текущего config, на изменение —
/// пишет в `Backend.config.theme` (живое применение + notify групп-окон).
fn color_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> [u8; 3],
    set: fn(&mut ChartTheme, [u8; 3]),
) -> Entity<ColorPickerState> {
    let cur = get(&backend.read(cx).config.theme);
    let st = cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(hex(cur))));
    cx.subscribe(&st, move |this, _emitter, ev: &ColorPickerEvent, cx| {
        let ColorPickerEvent::Change(v) = ev;
        if let Some(h) = v {
            let c = hsla_u8(*h);
            this.backend.update(cx, |b, cx| {
                if let Some(p) = b.preview.as_mut() {
                    set(&mut p.theme, c);
                    cx.notify();
                }
            });
        }
    })
    .detach();
    st
}

/// Слайдер f32, привязанный к полю темы (живое применение).
#[allow(clippy::too_many_arguments)]
fn num_field(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> f32,
    set: fn(&mut ChartTheme, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<SliderState> {
    let cur = get(&backend.read(cx).config.theme);
    let st = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(cur));
    cx.subscribe(&st, move |this, _emitter, ev: &SliderEvent, cx| {
        let SliderEvent::Change(v) = ev;
        let f = v.start();
        this.backend.update(cx, |b, cx| {
            if let Some(p) = b.preview.as_mut() {
                set(&mut p.theme, f);
                cx.notify();
            }
        });
    })
    .detach();
    st
}

/// Color-picker поля OrdersStyle (пишет в draft.orders).
fn ord_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&OrdersStyle) -> [u8; 3],
    set: fn(&mut OrdersStyle, [u8; 3]),
) -> Entity<ColorPickerState> {
    let cur = get(&backend.read(cx).config.orders);
    let st = cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(hex(cur))));
    cx.subscribe(&st, move |this, _e, ev: &ColorPickerEvent, cx| {
        let ColorPickerEvent::Change(v) = ev;
        if let Some(h) = v {
            let c = hsla_u8(*h);
            this.backend.update(cx, |b, cx| {
                if let Some(p) = b.preview.as_mut() {
                    set(&mut p.orders, c);
                    cx.notify();
                }
            });
        }
    })
    .detach();
    st
}

/// Слайдер f32 поля OrdersStyle (пишет в draft.orders).
fn ord_slider(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    get: fn(&OrdersStyle) -> f32,
    set: fn(&mut OrdersStyle, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<SliderState> {
    let cur = get(&backend.read(cx).config.orders);
    let st = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(cur));
    cx.subscribe(&st, move |this, _e, ev: &SliderEvent, cx| {
        let SliderEvent::Change(v) = ev;
        let f = v.start();
        this.backend.update(cx, |b, cx| {
            if let Some(p) = b.preview.as_mut() {
                set(&mut p.orders, f);
                cx.notify();
            }
        });
    })
    .detach();
    st
}

/// Чекбокс ордер-стиля: (id, подпись, геттер, сеттер) — для `line_block`.
type Check = (&'static str, &'static str, fn(&OrdersStyle) -> bool, fn(&mut OrdersStyle, bool));

/// Редактор одной ордер-линии: цвет + 4 слайдера (маркеры используются не всеми).
struct LineEd {
    color: Entity<ColorPickerState>,
    thickness: Entity<SliderState>,
    marker_size: Entity<SliderState>,
    marker_thickness: Entity<SliderState>,
    knot_size: Entity<SliderState>,
}

/// Строит [`LineEd`] для поля `$line` OrdersStyle (fn-ptr аксессоры).
macro_rules! line_ed {
    ($b:expr, $w:expr, $cx:expr, $line:ident) => {
        LineEd {
            color: ord_color($b, $w, $cx, |o| o.$line.color, |o, v| o.$line.color = v),
            thickness: ord_slider($b, $cx, |o| o.$line.thickness, |o, v| o.$line.thickness = v, 0.5, 6.0, 0.1),
            marker_size: ord_slider($b, $cx, |o| o.$line.marker_size, |o, v| o.$line.marker_size = v, 2.0, 24.0, 0.5),
            marker_thickness: ord_slider($b, $cx, |o| o.$line.marker_thickness, |o, v| o.$line.marker_thickness = v, 0.5, 5.0, 0.1),
            knot_size: ord_slider($b, $cx, |o| o.$line.knot_size, |o, v| o.$line.knot_size = v, 1.0, 10.0, 0.5),
        }
    };
}

/// Состояние редактора ордер-линий (вкладка «Линии»).
struct Lines {
    buy: LineEd,
    sell: LineEd,
    stop: LineEd,
    trailing: LineEd,
    take_profit: LineEd,
    vstop: LineEd,
    pending_cond: LineEd,
    liq: LineEd,
    path_color: Entity<ColorPickerState>,
    path_thickness: Entity<SliderState>,
    active_alpha: Entity<SliderState>,
    closed_alpha: Entity<SliderState>,
    max_closed: Entity<SliderState>,
}

/// Ячейка цвета (свотч + подпись), фикс. ширина.
fn color_cell(label: &str, st: &Entity<ColorPickerState>) -> impl IntoElement {
    div().w(px(180.0)).child(ColorPicker::new(st).label(label.to_string()))
}

/// Строка слайдера: подпись+значение, затем слайдер.
fn slider_row(label: &str, st: &Entity<SliderState>, cx: &App) -> impl IntoElement {
    let val = st.read(cx).value().start();
    v_flex()
        .w_full()
        .gap_1()
        .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child(format!("{label}: {val:.2}")))
        .child(Slider::new(st))
}

/// Редактор одной строки сервера (вкладка «Подключения»): текст-поля + цвет.
struct ConnRow {
    name: Entity<InputState>,
    key: Entity<InputState>,
    group: Entity<InputState>,
    color: Entity<ColorPickerState>,
}

/// TextInput, привязанный к полю сервера `servers[i]` (пишет в draft).
fn conn_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: String,
    set: fn(&mut ServerConfig, String),
) -> Entity<InputState> {
    let st = cx.new(|cx| InputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &InputEvent, cx| {
        if matches!(ev, InputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(s) = p.servers.get_mut(i) {
                        set(s, val);
                        bcx.notify();
                    }
                }
            });
        }
    })
    .detach();
    st
}

/// Color-picker, привязанный к `servers[i].color` (пишет в draft).
fn conn_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: [u8; 3],
) -> Entity<ColorPickerState> {
    let st = cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(hex(init))));
    cx.subscribe(&st, move |this, _e, ev: &ColorPickerEvent, cx| {
        let ColorPickerEvent::Change(v) = ev;
        if let Some(h) = v {
            let c = hsla_u8(*h);
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(s) = p.servers.get_mut(i) {
                        s.color = c;
                        bcx.notify();
                    }
                }
            });
        }
    })
    .detach();
    st
}

/// Построить per-server editor-стейты из draft-серверов. Зовётся в new() и после
/// add/remove сервера (индексы в подписках свежие).
fn build_conn(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Vec<ConnRow> {
    let servers = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).servers.clone()
    };
    servers
        .iter()
        .enumerate()
        .map(|(i, s)| ConnRow {
            name: conn_input(window, cx, i, s.name.clone(), |s, v| s.name = v),
            key: conn_input(window, cx, i, s.key.expose().to_string(), |s, v| s.key = Secret::new(v)),
            group: conn_input(window, cx, i, s.group.clone(), |s, v| s.group = v),
            color: conn_color(window, cx, i, s.color),
        })
        .collect()
}

/// Состояние редактора темы (вкладка «Интерфейс»): по entity на каждое поле.
struct Iface {
    bg: Entity<ColorPickerState>,
    grid: Entity<ColorPickerState>,
    grid_alpha: Entity<SliderState>,
    cross: Entity<ColorPickerState>,
    cross_alpha: Entity<SliderState>,
    cross_thickness: Entity<SliderState>,
    halo_radius: Entity<SliderState>,
    halo_intensity: Entity<SliderState>,
    book_bg: Entity<ColorPickerState>,
    book_bid: Entity<ColorPickerState>,
    book_ask: Entity<ColorPickerState>,
    panel_bg: Entity<ColorPickerState>,
    closed_bg: Entity<ColorPickerState>,
}

pub struct SettingsView {
    backend: Entity<Backend>,
    active: Tab,
    /// Статус сохранения: (текст, ошибка?).
    status: Option<(String, bool)>,
    iface: Iface,
    lines: Lines,
    /// Per-server editor-стейты (вкладка «Подключения»); пересоздаётся при add/del.
    conn: Vec<ConnRow>,
}

impl SettingsView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let iface = Iface {
            bg: color_field(&backend, window, cx, |t| t.bg, |t, v| t.bg = v),
            grid: color_field(&backend, window, cx, |t| t.grid, |t, v| t.grid = v),
            grid_alpha: num_field(&backend, cx, |t| t.grid_alpha, |t, v| t.grid_alpha = v, 0.0, 1.0, 0.01),
            cross: color_field(&backend, window, cx, |t| t.cross, |t, v| t.cross = v),
            cross_alpha: num_field(&backend, cx, |t| t.cross_alpha, |t, v| t.cross_alpha = v, 0.0, 1.0, 0.01),
            cross_thickness: num_field(&backend, cx, |t| t.cross_thickness, |t, v| t.cross_thickness = v, 0.5, 4.0, 0.1),
            halo_radius: num_field(&backend, cx, |t| t.halo_radius, |t, v| t.halo_radius = v, 0.0, 120.0, 1.0),
            halo_intensity: num_field(&backend, cx, |t| t.halo_intensity, |t, v| t.halo_intensity = v, 0.0, 0.6, 0.01),
            book_bg: color_field(&backend, window, cx, |t| t.book_bg, |t, v| t.book_bg = v),
            book_bid: color_field(&backend, window, cx, |t| t.book_bid, |t, v| t.book_bid = v),
            book_ask: color_field(&backend, window, cx, |t| t.book_ask, |t, v| t.book_ask = v),
            panel_bg: color_field(&backend, window, cx, |t| t.panel_bg, |t, v| t.panel_bg = v),
            closed_bg: color_field(&backend, window, cx, |t| t.closed_bg, |t, v| t.closed_bg = v),
        };
        let lines = Lines {
            buy: line_ed!(&backend, window, cx, buy),
            sell: line_ed!(&backend, window, cx, sell),
            stop: line_ed!(&backend, window, cx, stop),
            trailing: line_ed!(&backend, window, cx, trailing),
            take_profit: line_ed!(&backend, window, cx, take_profit),
            vstop: line_ed!(&backend, window, cx, vstop),
            pending_cond: line_ed!(&backend, window, cx, pending_cond),
            liq: line_ed!(&backend, window, cx, liq),
            path_color: ord_color(&backend, window, cx, |o| o.path.color, |o, v| o.path.color = v),
            path_thickness: ord_slider(&backend, cx, |o| o.path.thickness, |o, v| o.path.thickness = v, 0.5, 6.0, 0.1),
            active_alpha: ord_slider(&backend, cx, |o| o.active_alpha, |o, v| o.active_alpha = v, 0.05, 1.0, 0.01),
            closed_alpha: ord_slider(&backend, cx, |o| o.closed_alpha, |o, v| o.closed_alpha = v, 0.0, 1.0, 0.01),
            max_closed: ord_slider(&backend, cx, |o| o.max_closed_orders as f32, |o, v| o.max_closed_orders = v as u32, 0.0, 5000.0, 50.0),
        };
        let conn = build_conn(&backend, window, cx);
        // Закрытие окна (drop view) → сбросить draft: чарт откатывается к config
        // (отмена несохранённых правок) — как egui (draft discarded on close).
        cx.on_release(|this, app| {
            this.backend.update(app, |b, cx| {
                b.preview = None;
                cx.notify();
            });
        })
        .detach();
        Self { backend, active: Tab::Interface, status: None, iface, lines, conn }
    }

    /// Коммит draft → config + запись на диск (валидация внутри AppConfig::save).
    /// draft остаётся (правки продолжаются), как egui (Save не закрывает окно).
    fn save(&mut self, cx: &mut Context<Self>) {
        let res = self.backend.update(cx, |b, _| {
            if let Some(p) = &b.preview {
                b.config = p.clone();
            }
            b.config.save()
        });
        self.status = Some(match res {
            Ok(()) => ("Сохранено".into(), false),
            Err(e) => (e.to_string(), true),
        });
        cx.notify();
    }

    /// Изменить срок хранения логов (клампим 0..=365), правит draft.
    fn adjust_ret(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let v = (p.log_retention_days as i32 + delta).clamp(0, 365) as u32;
                p.log_retention_days = v;
                bcx.notify();
            }
        });
        cx.notify();
    }

    fn general_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let muted = rgb(hex(palette::TEXT_2));
        // Снимок значений draft (settings открыто → preview Some; иначе config).
        let (lang, split, logf, ret) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (d.language, d.charts_split_by_core, d.log_to_file, d.log_retention_days)
        };
        let header = |t: &str| div().mt_2().font_bold().child(t.to_string());
        let hint = |t: &str| div().text_xs().text_color(muted).child(t.to_string());

        // Язык — сегментированный выбор (3 опции).
        let mut lang_row = h_flex().gap_2();
        for l in Language::ALL {
            let on = l == lang;
            let btn = Button::new(l.label()).label(l.label());
            let btn = if on { btn.primary() } else { btn.ghost() };
            lang_row = lang_row.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        p.language = l;
                        bcx.notify();
                    }
                });
                cx.notify();
            })));
        }

        v_flex()
            .w_full()
            .gap_2()
            .child(header("Язык"))
            .child(lang_row)
            .child(hint("Применяется после сохранения и перезапуска."))
            .child(header("Чарты"))
            .child(
                Checkbox::new("split")
                    .label("Отдельная вкладка чарта на ядро")
                    .checked(split)
                    .on_click(cx.listener(|this, ch: &bool, _w, cx| {
                        let v = *ch;
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                p.charts_split_by_core = v;
                                bcx.notify();
                            }
                        });
                        cx.notify();
                    })),
            )
            .child(header("Логи"))
            .child(
                Checkbox::new("logf")
                    .label("Писать лог в файл")
                    .checked(logf)
                    .on_click(cx.listener(|this, ch: &bool, _w, cx| {
                        let v = *ch;
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                p.log_to_file = v;
                                bcx.notify();
                            }
                        });
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(muted).child("Хранить логи, дней:"))
                    .child(
                        Button::new("ret-")
                            .ghost()
                            .label("−")
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_ret(-1, cx))),
                    )
                    .child(div().w(px(48.0)).child(format!("{ret}")))
                    .child(
                        Button::new("ret+")
                            .ghost()
                            .label("+")
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_ret(1, cx))),
                    ),
            )
            .child(hint("0 — хранить всё."))
    }

    /// Checkbox булева поля OrdersStyle (пишет в draft.orders, notify групп+view).
    fn ord_check(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        label: &'static str,
        get: fn(&OrdersStyle) -> bool,
        set: fn(&mut OrdersStyle, bool),
    ) -> impl IntoElement {
        let cur = {
            let b = self.backend.read(cx);
            get(&b.preview.as_ref().unwrap_or(&b.config).orders)
        };
        Checkbox::new(id).label(label).checked(cur).on_click(cx.listener(
            move |this, ch: &bool, _w, cx| {
                let v = *ch;
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        set(&mut p.orders, v);
                        bcx.notify();
                    }
                });
                cx.notify();
            },
        ))
    }

    /// Блок одной ордер-линии: заголовок, цвет, толщина, чекбоксы, маркеры (если есть).
    fn line_block(
        &self,
        cx: &Context<Self>,
        title: &str,
        ed: &LineEd,
        markers: bool,
        checks: &[Check],
    ) -> impl IntoElement {
        let mut checkrow = h_flex().gap_3().flex_wrap();
        for (id, label, get, set) in checks {
            checkrow = checkrow.child(self.ord_check(cx, id, label, *get, *set));
        }
        let mut col = v_flex()
            .w_full()
            .gap_1()
            .child(div().mt_2().font_bold().child(title.to_string()))
            .child(color_cell("Цвет", &ed.color))
            .child(slider_row("Толщина", &ed.thickness, cx))
            .child(checkrow);
        if markers {
            col = col
                .child(slider_row("Размер маркера", &ed.marker_size, cx))
                .child(slider_row("Толщина маркера", &ed.marker_thickness, cx))
                .child(slider_row("Размер узла", &ed.knot_size, cx));
        }
        col
    }

    fn lines_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        // Чекбоксы маркеров для линии с заданным префиксом id и аксессорами.
        let l = &self.lines;
        v_flex()
            .w_full()
            .gap_2()
            .child(div().font_bold().child("Линии ордеров"))
            .child(self.line_block(cx, "Покупка", &l.buy, true, &[
                ("buy-d", "Пунктир", |o| o.buy.dashed, |o, v| o.buy.dashed = v),
                ("buy-s", "Маркер начала", |o| o.buy.start_marker, |o, v| o.buy.start_marker = v),
                ("buy-e", "Маркер конца", |o| o.buy.end_marker, |o, v| o.buy.end_marker = v),
                ("buy-k", "Узлы", |o| o.buy.knots, |o, v| o.buy.knots = v),
            ]))
            .child(self.line_block(cx, "Продажа", &l.sell, true, &[
                ("sell-d", "Пунктир", |o| o.sell.dashed, |o, v| o.sell.dashed = v),
                ("sell-s", "Маркер начала", |o| o.sell.start_marker, |o, v| o.sell.start_marker = v),
                ("sell-e", "Маркер конца", |o| o.sell.end_marker, |o, v| o.sell.end_marker = v),
                ("sell-k", "Узлы", |o| o.sell.knots, |o, v| o.sell.knots = v),
            ]))
            .child(self.line_block(cx, "Стоп", &l.stop, true, &[
                ("stop-d", "Пунктир", |o| o.stop.dashed, |o, v| o.stop.dashed = v),
                ("stop-s", "Маркер начала", |o| o.stop.start_marker, |o, v| o.stop.start_marker = v),
                ("stop-e", "Маркер конца", |o| o.stop.end_marker, |o, v| o.stop.end_marker = v),
                ("stop-k", "Узлы", |o| o.stop.knots, |o, v| o.stop.knots = v),
            ]))
            .child(self.line_block(cx, "Трейлинг", &l.trailing, true, &[
                ("tr-d", "Пунктир", |o| o.trailing.dashed, |o, v| o.trailing.dashed = v),
                ("tr-s", "Маркер начала", |o| o.trailing.start_marker, |o, v| o.trailing.start_marker = v),
                ("tr-e", "Маркер конца", |o| o.trailing.end_marker, |o, v| o.trailing.end_marker = v),
                ("tr-k", "Узлы", |o| o.trailing.knots, |o, v| o.trailing.knots = v),
            ]))
            .child(self.line_block(cx, "Тейк-профит", &l.take_profit, true, &[
                ("tp-d", "Пунктир", |o| o.take_profit.dashed, |o, v| o.take_profit.dashed = v),
                ("tp-s", "Маркер начала", |o| o.take_profit.start_marker, |o, v| o.take_profit.start_marker = v),
                ("tp-e", "Маркер конца", |o| o.take_profit.end_marker, |o, v| o.take_profit.end_marker = v),
                ("tp-k", "Узлы", |o| o.take_profit.knots, |o, v| o.take_profit.knots = v),
            ]))
            .child(self.line_block(cx, "VStop", &l.vstop, true, &[
                ("vs-d", "Пунктир", |o| o.vstop.dashed, |o, v| o.vstop.dashed = v),
                ("vs-s", "Маркер начала", |o| o.vstop.start_marker, |o, v| o.vstop.start_marker = v),
                ("vs-e", "Маркер конца", |o| o.vstop.end_marker, |o, v| o.vstop.end_marker = v),
                ("vs-k", "Узлы", |o| o.vstop.knots, |o, v| o.vstop.knots = v),
            ]))
            .child(self.line_block(cx, "Ожидающее условие", &l.pending_cond, true, &[
                ("pc-d", "Пунктир", |o| o.pending_cond.dashed, |o, v| o.pending_cond.dashed = v),
                ("pc-s", "Маркер начала", |o| o.pending_cond.start_marker, |o, v| o.pending_cond.start_marker = v),
                ("pc-e", "Маркер конца", |o| o.pending_cond.end_marker, |o, v| o.pending_cond.end_marker = v),
                ("pc-k", "Узлы", |o| o.pending_cond.knots, |o, v| o.pending_cond.knots = v),
            ]))
            .child(self.line_block(cx, "Ликвидация", &l.liq, false, &[
                ("liq-d", "Пунктир", |o| o.liq.dashed, |o, v| o.liq.dashed = v),
            ]))
            .child(div().mt_2().font_bold().child("Путь (trail)"))
            .child(color_cell("Цвет", &l.path_color))
            .child(slider_row("Толщина", &l.path_thickness, cx))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(self.ord_check(cx, "path-show", "Показывать", |o| o.path.show, |o, v| o.path.show = v))
                    .child(self.ord_check(cx, "path-dash", "Пунктир", |o| o.path.dashed, |o, v| o.path.dashed = v)),
            )
            .child(div().mt_2().font_bold().child("Общее"))
            .child(slider_row("Прозрачность активных", &l.active_alpha, cx))
            .child(slider_row("Прозрачность закрытых", &l.closed_alpha, cx))
            .child(self.ord_check(cx, "pending-dash", "Пунктир у ожидающих покупок", |o| o.pending_dashed, |o, v| o.pending_dashed = v))
            .child(slider_row("Макс. закрытых ордеров", &l.max_closed, cx))
    }

    /// Checkbox булева поля сервера `servers[i]` (пишет в draft).
    fn srv_check(
        &self,
        cx: &Context<Self>,
        i: usize,
        suffix: &str,
        label: &'static str,
        get: fn(&ServerConfig) -> bool,
        set: fn(&mut ServerConfig, bool),
    ) -> impl IntoElement {
        let cur = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).servers.get(i).map(get).unwrap_or(false)
        };
        Checkbox::new(SharedString::from(format!("{suffix}-{i}")))
            .label(label)
            .checked(cur)
            .on_click(cx.listener(move |this, ch: &bool, _w, cx| {
                let v = *ch;
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        if let Some(s) = p.servers.get_mut(i) {
                            set(s, v);
                            bcx.notify();
                        }
                    }
                });
                cx.notify();
            }))
    }

    /// Checkbox feed-флага сервера `servers[i].feed` (пишет в draft).
    fn feed_check(
        &self,
        cx: &Context<Self>,
        i: usize,
        suffix: &str,
        label: &'static str,
        get: fn(&FeedFlags) -> bool,
        set: fn(&mut FeedFlags, bool),
    ) -> impl IntoElement {
        let cur = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).servers.get(i).map(|s| get(&s.feed)).unwrap_or(false)
        };
        Checkbox::new(SharedString::from(format!("{suffix}-{i}")))
            .label(label)
            .checked(cur)
            .on_click(cx.listener(move |this, ch: &bool, _w, cx| {
                let v = *ch;
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        if let Some(s) = p.servers.get_mut(i) {
                            set(&mut s.feed, v);
                            bcx.notify();
                        }
                    }
                });
                cx.notify();
            }))
    }

    /// Добавить сервер в draft (id = max+1) и пересобрать editor-стейты.
    fn add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let next = p.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                p.servers.push(ServerConfig {
                    id: next,
                    uid: 0,
                    name: format!("server {next}"),
                    active: true,
                    show_window: true,
                    feed: FeedFlags::default(),
                    key: Secret::new(""),
                    group: "default".into(),
                    market: "BTCUSDT".into(),
                    color: palette::ACCENT,
                    synthetic: false,
                });
                bcx.notify();
            }
        });
        let rows = build_conn(&self.backend, window, cx);
        self.conn = rows;
        cx.notify();
    }

    /// Удалить сервер `i` из draft и пересобрать editor-стейты.
    fn delete_server(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if i < p.servers.len() {
                    p.servers.remove(i);
                    bcx.notify();
                }
            }
        });
        let rows = build_conn(&self.backend, window, cx);
        self.conn = rows;
        cx.notify();
    }

    /// Карточка одного сервера: чекбоксы + текст-поля + цвет + удалить + feed-флаги.
    fn server_card(&self, cx: &Context<Self>, i: usize, row: &ConnRow) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(rgb(hex(palette::LIFT_HOVER)))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(self.srv_check(cx, i, "act", "Вкл", |s| s.active, |s, v| s.active = v))
                    .child(self.srv_check(cx, i, "win", "Окно", |s| s.show_window, |s, v| s.show_window = v))
                    .child(div().w(px(130.0)).child(Input::new(&row.name)))
                    .child(div().w(px(170.0)).child(Input::new(&row.key)))
                    .child(div().w(px(100.0)).child(Input::new(&row.group)))
                    .child(div().w(px(140.0)).child(ColorPicker::new(&row.color).label("Цвет")))
                    .child(
                        Button::new(SharedString::from(format!("del-{i}")))
                            .danger()
                            .label("✕")
                            .on_click(cx.listener(move |this, _, w, cx| this.delete_server(i, w, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.feed_check(cx, i, "f-ord", "orders", |f| f.orders, |f, v| f.orders = v))
                    .child(self.feed_check(cx, i, "f-det", "detects", |f| f.detects, |f, v| f.detects = v))
                    .child(self.feed_check(cx, i, "f-rep", "reports", |f| f.reports, |f, v| f.reports = v))
                    .child(self.feed_check(cx, i, "f-bal", "balance", |f| f.balance, |f, v| f.balance = v))
                    .child(self.feed_check(cx, i, "f-str", "strategies", |f| f.strategies, |f, v| f.strategies = v))
                    .child(self.feed_check(cx, i, "f-log", "log", |f| f.log, |f, v| f.log = v))
                    .child(self.feed_check(cx, i, "f-alr", "alerts", |f| f.alerts, |f, v| f.alerts = v))
                    .child(self.feed_check(cx, i, "f-arb", "arb", |f| f.arb, |f, v| f.arb = v)),
            )
    }

    fn connections_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let (mode, n_servers, groups) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (d.market_mode, d.servers.len(), d.groups.clone())
        };

        // Режим данных — сегментированный выбор (Select добавим в проходе по компонентам).
        let mut mode_row = h_flex().gap_2();
        for (m, lbl) in [(MarketDataMode::Dedup, "Дедуп (1/биржа)"), (MarketDataMode::PerCore, "На ядро")] {
            let on = m == mode;
            let b = Button::new(lbl).label(lbl);
            let b = if on { b.primary() } else { b.ghost() };
            mode_row = mode_row.child(b.on_click(cx.listener(move |this, _, _, cx| {
                this.backend.update(cx, |bk, bcx| {
                    if let Some(p) = bk.preview.as_mut() {
                        p.market_mode = m;
                        bcx.notify();
                    }
                });
                cx.notify();
            })));
        }

        let mut col = v_flex()
            .w_full()
            .gap_2()
            .child(div().font_bold().child("Источник данных"))
            .child(mode_row)
            .child(div().mt_2().font_bold().child("Серверы"));
        for i in 0..n_servers {
            if let Some(row) = self.conn.get(i) {
                col = col.child(self.server_card(cx, i, row));
            }
        }
        col = col.child(
            Button::new("add-srv")
                .label("+ Сервер")
                .on_click(cx.listener(|this, _, w, cx| this.add_server(w, cx))),
        );

        col = col.child(div().mt_2().font_bold().child("Группы"));
        for g in &groups {
            let name = g.name.clone();
            let active = g.active;
            let id = SharedString::from(format!("grp-{name}"));
            let n2 = name.clone();
            col = col.child(
                Checkbox::new(id)
                    .label(SharedString::from(name))
                    .checked(active)
                    .on_click(cx.listener(move |this, ch: &bool, _w, cx| {
                        let v = *ch;
                        let n = n2.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(gc) = p.groups.iter_mut().find(|g| g.name == n) {
                                    gc.active = v;
                                    bcx.notify();
                                }
                            }
                        });
                        cx.notify();
                    })),
            );
        }
        col
    }

    fn interface_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let muted = rgb(hex(palette::TEXT_2));
        let header = |t: &str| div().mt_2().font_bold().child(t.to_string());
        // Ячейка цвета: свотч + подпись.
        let color = |label: &str, st: &Entity<ColorPickerState>| {
            div()
                .w(px(180.0))
                .child(ColorPicker::new(st).label(label.to_string()))
        };
        // Строка слайдера: подпись+значение, затем сам слайдер.
        let slider = |label: &str, st: &Entity<SliderState>| {
            let val = st.read(cx).value().start();
            v_flex()
                .w_full()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{label}: {val:.2}")),
                )
                .child(Slider::new(st))
        };

        v_flex()
            .w_full()
            .gap_2()
            .child(header("Чарт"))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(color("Фон", &self.iface.bg))
                    .child(color("Сетка", &self.iface.grid)),
            )
            .child(slider("Прозрачность сетки", &self.iface.grid_alpha))
            .child(header("Перекрестие"))
            .child(h_flex().gap_3().flex_wrap().child(color("Цвет", &self.iface.cross)))
            .child(slider("Прозрачность", &self.iface.cross_alpha))
            .child(slider("Толщина", &self.iface.cross_thickness))
            .child(slider("Радиус ореола", &self.iface.halo_radius))
            .child(slider("Яркость ореола", &self.iface.halo_intensity))
            .child(header("Стакан"))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(color("Фон", &self.iface.book_bg))
                    .child(color("Bid", &self.iface.book_bid))
                    .child(color("Ask", &self.iface.book_ask)),
            )
            .child(header("Панели"))
            .child(h_flex().gap_3().flex_wrap().child(color("Фон панелей", &self.iface.panel_bg)))
            .child(header("Закрытый чарт"))
            .child(h_flex().gap_3().flex_wrap().child(color("Фон", &self.iface.closed_bg)))
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(muted)
                    .child("Изменения применяются сразу; «Сохранить» пишет тему на диск."),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = rgb(hex(palette::SURFACE_1));
        let panel = rgb(hex(palette::BG));
        let border = rgb(hex(palette::LIFT_HOVER));
        let accent = rgb(hex(palette::ACCENT));
        let muted = rgb(hex(palette::TEXT_2));

        // ── Полоска вкладок ─────────────────────────────────────────────────
        let mut tabs = h_flex().w_full().gap_1().px_2().bg(panel).border_b_1().border_color(border);
        for t in Tab::ALL {
            let on = self.active == t;
            let (tc, bb) = if on { (accent, accent) } else { (muted, panel) };
            tabs = tabs.child(
                div()
                    .id(t.title())
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_color(tc)
                    .border_b_2()
                    .border_color(bb)
                    .child(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = t;
                        cx.notify();
                    })),
            );
        }

        // ── Тело активной вкладки ───────────────────────────────────────────
        let content = match self.active {
            Tab::Interface => self.interface_tab(cx).into_any_element(),
            Tab::General => self.general_tab(cx).into_any_element(),
            Tab::Lines => self.lines_tab(cx).into_any_element(),
            Tab::Connections => self.connections_tab(cx).into_any_element(),
        };
        // Тело прокручивается (вкладки выше высоты окна): stateful div + overflow_y_scroll.
        let body = div()
            .id("settings-body")
            .flex_1()
            .w_full()
            .overflow_y_scroll()
            .child(v_flex().w_full().p_4().gap_2().child(content));

        // ── Подвал: Сохранить + статус ──────────────────────────────────────
        let status_el = match &self.status {
            Some((msg, err)) => div()
                .text_color(if *err { rgb(hex(palette::RED)) } else { rgb(hex(palette::GREEN)) })
                .child(msg.clone()),
            None => div(),
        };
        let footer = h_flex()
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .items_center()
            .bg(panel)
            .border_t_1()
            .border_color(border)
            .child(
                Button::new("save")
                    .primary()
                    .label("Сохранить")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .child(status_el);

        v_flex()
            .size_full()
            .bg(bg)
            .text_color(rgb(hex(palette::TEXT)))
            .text_sm()
            .child(tabs)
            .child(body)
            .child(footer)
    }
}

/// Открыть окно настроек (отдельное ОС-окно). Заводит draft = копия config (его
/// правят вкладки, чарт показывает его живьём). Повторный клик при уже открытом
/// окне игнорируем (draft уже есть) — иначе два окна делили бы один draft.
pub fn open(backend: Entity<Backend>, cx: &mut App) {
    if backend.read(cx).preview.is_some() {
        return;
    }
    backend.update(cx, |b, _| b.preview = Some(b.config.clone()));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(160.0), px(120.0)),
            size: size(px(860.0), px(580.0)),
        })),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal — Настройки".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(opts, |window, cx| {
        let view = cx.new(|cx| SettingsView::new(backend, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .ok();
}
