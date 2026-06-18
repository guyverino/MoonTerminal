//! Вкладка «Интерфейс» — тема оформления (порт egui `settings/interface.rs`): цвета
//! графика/перекрестия/стакана/панелей/закрытого графика + слайдеры. Правки идут в
//! draft (живое превью), «Сохранить» пишет theme.toml. Состояние редактора — [`Iface`].

use gpui::*;
use moon_ui::{
    MoonColorPickerEvent, MoonColorPickerState, MoonPalette, MoonSliderEvent, MoonSliderState,
    v_flex,
};

use super::{SettingsView, color_row, hsla_u8, section, separator, slider_row};
use crate::{Backend, hex};
use moon_core::config::{AppConfig, ChartTheme};

/// Состояние редактора темы: по entity на каждое поле.
pub(super) struct Iface {
    ui_font_delta: Entity<MoonSliderState>,
    bg: Entity<MoonColorPickerState>,
    grid: Entity<MoonColorPickerState>,
    grid_alpha: Entity<MoonSliderState>,
    cross: Entity<MoonColorPickerState>,
    cross_alpha: Entity<MoonSliderState>,
    cross_thickness: Entity<MoonSliderState>,
    halo_radius: Entity<MoonSliderState>,
    halo_intensity: Entity<MoonSliderState>,
    book_bg: Entity<MoonColorPickerState>,
    book_bid: Entity<MoonColorPickerState>,
    book_ask: Entity<MoonColorPickerState>,
    panel_bg: Entity<MoonColorPickerState>,
    closed_bg: Entity<MoonColorPickerState>,
}

/// Слайдер f32, привязанный к общему AppConfig, а не к теме чарта.
fn app_num_field(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    get: fn(&AppConfig) -> f32,
    set: fn(&mut AppConfig, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        get(b.preview.as_ref().unwrap_or(&b.config))
    };
    let st = cx.new(|_| {
        MoonSliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(cur)
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonSliderEvent, cx| {
        let MoonSliderEvent::Change(f) = ev else {
            return;
        };
        let f = f.end();
        this.backend.update(cx, |b, cx| {
            if let Some(p) = b.preview.as_mut() {
                if get(p) != f {
                    set(p, f);
                    crate::install_moon_theme_for_config(p, cx);
                    cx.notify();
                }
            }
        });
    })
    .detach();
    st
}

/// Color-picker, привязанный к полю темы: init из текущего config, на изменение —
/// пишет в `Backend.preview.theme` (живое применение + notify групп-окон).
fn color_field(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&ChartTheme) -> [u8; 3],
    set: fn(&mut ChartTheme, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = get(&backend.read(cx).config.theme);
    let st = cx.new(|cx| MoonColorPickerState::new(window, cx).default_value(rgb(hex(cur)).into()));
    cx.subscribe(&st, move |this, _emitter, ev: &MoonColorPickerEvent, cx| {
        let MoonColorPickerEvent::Change(h) = ev;
        let c = hsla_u8(*h);
        this.backend.update(cx, |b, cx| {
            if let Some(p) = b.preview.as_mut() {
                if get(&p.theme) != c {
                    set(&mut p.theme, c);
                    cx.notify();
                }
            }
        });
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
) -> Entity<MoonSliderState> {
    let cur = get(&backend.read(cx).config.theme);
    let st = cx.new(|_| {
        MoonSliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(cur)
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonSliderEvent, cx| {
        let MoonSliderEvent::Change(f) = ev else {
            return;
        };
        let f = f.end();
        this.backend.update(cx, |b, cx| {
            if let Some(p) = b.preview.as_mut() {
                if get(&p.theme) != f {
                    set(&mut p.theme, f);
                    cx.notify();
                }
            }
        });
    })
    .detach();
    st
}

/// Собрать редактор темы из текущего draft (зовётся из `SettingsView::new`).
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Iface {
    Iface {
        ui_font_delta: app_num_field(
            backend,
            cx,
            |c| c.ui_font_delta,
            |c, v| c.ui_font_delta = v,
            -2.0,
            6.0,
            1.0,
        ),
        bg: color_field(backend, window, cx, |t| t.bg, |t, v| t.bg = v),
        grid: color_field(backend, window, cx, |t| t.grid, |t, v| t.grid = v),
        grid_alpha: num_field(
            backend,
            cx,
            |t| t.grid_alpha,
            |t, v| t.grid_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        cross: color_field(backend, window, cx, |t| t.cross, |t, v| t.cross = v),
        cross_alpha: num_field(
            backend,
            cx,
            |t| t.cross_alpha,
            |t, v| t.cross_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        cross_thickness: num_field(
            backend,
            cx,
            |t| t.cross_thickness,
            |t, v| t.cross_thickness = v,
            0.5,
            4.0,
            0.1,
        ),
        halo_radius: num_field(
            backend,
            cx,
            |t| t.halo_radius,
            |t, v| t.halo_radius = v,
            0.0,
            120.0,
            1.0,
        ),
        halo_intensity: num_field(
            backend,
            cx,
            |t| t.halo_intensity,
            |t, v| t.halo_intensity = v,
            0.0,
            0.6,
            0.01,
        ),
        book_bg: color_field(backend, window, cx, |t| t.book_bg, |t, v| t.book_bg = v),
        book_bid: color_field(backend, window, cx, |t| t.book_bid, |t, v| t.book_bid = v),
        book_ask: color_field(backend, window, cx, |t| t.book_ask, |t, v| t.book_ask = v),
        panel_bg: color_field(backend, window, cx, |t| t.panel_bg, |t, v| t.panel_bg = v),
        closed_bg: color_field(backend, window, cx, |t| t.closed_bg, |t, v| t.closed_bg = v),
    }
}

impl SettingsView {
    /// Вкладка «Интерфейс» — порт egui `settings/interface.rs` точь-в-точь: секции
    /// График(фон/сетка) · Перекрестие · Стакан · Панели · Закрытый график, цветовые
    /// ряды (свотч+подпись) и слайдеры; разделители между секциями; хинт внизу.
    pub(super) fn interface_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let i = &self.iface;
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .gap_1()
            // UI: шрифты и масштаб
            .child(section("Интерфейс: шрифт", p, cx))
            .child(slider_row("Прибавка к размеру шрифта", &i.ui_font_delta, cx))
            .child(separator(p, cx))
            // График: фон и сетка
            .child(section("График: фон и сетка", p, cx))
            .child(color_row("Цвет фона графика", &i.bg, p, cx))
            .child(color_row("Цвет сетки", &i.grid, p, cx))
            .child(slider_row("Видимость сетки", &i.grid_alpha, cx))
            .child(separator(p, cx))
            // График: перекрестие
            .child(section("График: перекрестие", p, cx))
            .child(color_row("Цвет перекрестия", &i.cross, p, cx))
            .child(slider_row("Прозрачность линий", &i.cross_alpha, cx))
            .child(slider_row("Толщина линий", &i.cross_thickness, cx))
            .child(slider_row("Радиус ореола", &i.halo_radius, cx))
            .child(slider_row("Яркость ореола", &i.halo_intensity, cx))
            .child(separator(p, cx))
            // Стакан
            .child(section("Стакан", p, cx))
            .child(color_row("Фон стакана", &i.book_bg, p, cx))
            .child(color_row("Цвет покупок (bid)", &i.book_bid, p, cx))
            .child(color_row("Цвет продаж (ask)", &i.book_ask, p, cx))
            .child(separator(p, cx))
            // Панели
            .child(section("Панели", p, cx))
            .child(color_row(
                "Фон панелей (тулбар, ордер, док, статус)",
                &i.panel_bg,
                p,
                cx,
            ))
            .child(separator(p, cx))
            // Закрытый график
            .child(section("Закрытый график", p, cx))
            .child(color_row("Фон пустого контейнера", &i.closed_bg, p, cx))
            .child(
                div()
                    .mt_2()
                    .text_color(rgb(p.text_soft))
                    .child("Меняется вживую. «Сохранить» пишет theme.toml рядом с программой — им можно делиться."),
            )
    }
}
