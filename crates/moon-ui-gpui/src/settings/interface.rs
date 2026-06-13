//! Вкладка «Интерфейс» — тема оформления (порт egui `settings/interface.rs`): цвета
//! графика/перекрестия/стакана/панелей/закрытого графика + слайдеры. Правки идут в
//! draft (живое превью), «Сохранить» пишет theme.toml. Состояние редактора — [`Iface`].

use gpui::*;
use gpui_component::{
    color_picker::{ColorPickerEvent, ColorPickerState},
    slider::{SliderEvent, SliderState},
    v_flex,
};

use super::{color_row, hsla_u8, section, separator, slider_row, SettingsView};
use crate::{hex, Backend};
use moon_core::config::ChartTheme;
use moon_core::palette;

/// Состояние редактора темы: по entity на каждое поле.
pub(super) struct Iface {
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

/// Color-picker, привязанный к полю темы: init из текущего config, на изменение —
/// пишет в `Backend.preview.theme` (живое применение + notify групп-окон).
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

/// Собрать редактор темы из текущего draft (зовётся из `SettingsView::new`).
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Iface {
    Iface {
        bg: color_field(backend, window, cx, |t| t.bg, |t, v| t.bg = v),
        grid: color_field(backend, window, cx, |t| t.grid, |t, v| t.grid = v),
        grid_alpha: num_field(backend, cx, |t| t.grid_alpha, |t, v| t.grid_alpha = v, 0.0, 1.0, 0.01),
        cross: color_field(backend, window, cx, |t| t.cross, |t, v| t.cross = v),
        cross_alpha: num_field(backend, cx, |t| t.cross_alpha, |t, v| t.cross_alpha = v, 0.0, 1.0, 0.01),
        cross_thickness: num_field(backend, cx, |t| t.cross_thickness, |t, v| t.cross_thickness = v, 0.5, 4.0, 0.1),
        halo_radius: num_field(backend, cx, |t| t.halo_radius, |t, v| t.halo_radius = v, 0.0, 120.0, 1.0),
        halo_intensity: num_field(backend, cx, |t| t.halo_intensity, |t, v| t.halo_intensity = v, 0.0, 0.6, 0.01),
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
        v_flex()
            .w_full()
            .gap_1()
            // График: фон и сетка
            .child(section("График: фон и сетка"))
            .child(color_row("Цвет фона графика", &i.bg))
            .child(color_row("Цвет сетки", &i.grid))
            .child(slider_row("Видимость сетки", &i.grid_alpha, cx))
            .child(separator())
            // График: перекрестие
            .child(section("График: перекрестие"))
            .child(color_row("Цвет перекрестия", &i.cross))
            .child(slider_row("Прозрачность линий", &i.cross_alpha, cx))
            .child(slider_row("Толщина линий", &i.cross_thickness, cx))
            .child(slider_row("Радиус ореола", &i.halo_radius, cx))
            .child(slider_row("Яркость ореола", &i.halo_intensity, cx))
            .child(separator())
            // Стакан
            .child(section("Стакан"))
            .child(color_row("Фон стакана", &i.book_bg))
            .child(color_row("Цвет покупок (bid)", &i.book_bid))
            .child(color_row("Цвет продаж (ask)", &i.book_ask))
            .child(separator())
            // Панели
            .child(section("Панели"))
            .child(color_row("Фон панелей (тулбар, ордер, док, статус)", &i.panel_bg))
            .child(separator())
            // Закрытый график
            .child(section("Закрытый график"))
            .child(color_row("Фон пустого контейнера", &i.closed_bg))
            .child(
                div()
                    .mt_2()
                    .text_color(rgb(hex(palette::TEXT_2)))
                    .child("Меняется вживую. «Сохранить» пишет theme.toml рядом с программой — им можно делиться."),
            )
    }
}
