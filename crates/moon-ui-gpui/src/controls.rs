//! Торговый тулбар — порт верхней полосы стенда (egui `dock/toolbar.rs`): ОДНА тонкая
//! полоса на высоту кнопки с группами `SIZE` (F1..F6) / `SELL` (S1..S6) / `МАСШТАБ`
//! (пресеты Y) + `Live`. Рисуется компонентами gpui-component (`Button`), цвета — из
//! `moon_core::palette`. Полоса фиксированная (между шапкой и доком), как `TopBottomPanel`
//! egui — НЕ dock-панель (у dock-панелей свой таб-бар, полосой на высоту кнопки не сделать).
//!
//! Размеры/Продажа — действия-заглушки (лог, 1:1 с egui). Масштаб + Live правят вид чарта
//! через состояние в `Backend` (`price_scale`/`follow`), которое применяет `ChartPanel`.

use gpui::*;
use gpui_component::{button::Button, h_flex, Selectable, Sizable};

use crate::{hex, Backend};
use moon_core::palette;

/// Высота полосы тулбара (лог. px). Под xsmall-кнопку (20px) + вертикальные отступы.
pub const TOOLBAR_H: f32 = 34.0;

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

/// Мелкая тусклая подпись группы (`SIZE`/`SELL`/`МАСШТАБ`) — стендовый `.strip-label`.
fn strip_label(text: &'static str) -> impl IntoElement {
    div().text_xs().text_color(rgb(hex(palette::TEXT_3))).child(text)
}

/// Вертикальный разделитель групп (стендовый `.divider`): тонкая линия высотой 16px.
fn divider() -> impl IntoElement {
    div().w(px(1.0)).h(px(16.0)).bg(rgb(hex(palette::LIFT_HOVER)))
}

/// Кнопка-ключ полосы (F1.. / S1.. / пресет масштаба / Live): xsmall+outline, акцент при
/// `selected`. `on` подсвечивает активный пресет/Live; для F/S всегда false.
fn key_btn(id: impl Into<SharedString>, label: &str, on: bool) -> Button {
    Button::new(id.into()).label(label.to_string()).outline().xsmall().compact().selected(on)
}

/// Полоса тулбара: рисуется как обычный child `Shell` (между шапкой и доком), не dock-панель.
/// Читает текущий масштаб/follow из `backend`, клики пишут обратно (+notify → перерисовка).
pub fn toolbar(backend: &Entity<Backend>, cx: &App) -> impl IntoElement {
    let (scale, follow) = {
        let b = backend.read(cx);
        (b.price_scale, b.follow)
    };

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(px(TOOLBAR_H))
        .items_center()
        .gap_1p5()
        .px_3()
        .bg(rgb(hex(palette::SURFACE_1)))
        .border_b_1()
        .border_color(rgb(hex(palette::LIFT_HOVER)))
        .text_color(rgb(hex(palette::TEXT)));

    // --- SIZE: F1..F6 (действия-заглушки, как egui) ---
    row = row.child(strip_label("SIZE"));
    for k in SIZE_KEYS {
        row = row.child(key_btn(format!("size-{k}"), k, false).on_click(move |_, _, _| {
            log::info!("[ui] size {k} (todo)")
        }));
    }
    row = row.child(divider());

    // --- SELL: S1..S6 ---
    row = row.child(strip_label("SELL"));
    for k in SELL_KEYS {
        row = row.child(key_btn(format!("sell-{k}"), k, false).on_click(move |_, _, _| {
            log::info!("[ui] sell {k} (todo)")
        }));
    }
    row = row.child(divider());

    // --- МАСШТАБ: пресеты цены (Y). Активный — accent. Пишем в Backend.price_scale ---
    row = row.child(strip_label("МАСШТАБ"));
    for (label, pct) in SCALES {
        let backend = backend.clone();
        row = row.child(key_btn(format!("scale-{label}"), label, scale == pct).on_click(
            move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    b.price_scale = pct;
                    bcx.notify();
                });
            },
        ));
    }
    row = row.child(divider());

    // --- Live/Пауза: вид бежит за «сейчас» / заморожен. Пишем в Backend.follow ---
    let backend = backend.clone();
    row.child(
        key_btn("live", if follow { "Live" } else { "Пауза" }, follow).on_click(move |_, _, cx| {
            backend.update(cx, |b, bcx| {
                b.follow = !b.follow;
                bcx.notify();
            });
        }),
    )
}
