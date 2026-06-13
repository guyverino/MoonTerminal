//! Торговый тулбар — порт верхней полосы стенда (egui `dock/toolbar.rs`): ОДНА тонкая
//! полоса на высоту кнопки с группами `SIZE` (F1..F6) / `SELL` (S1..S6) / `МАСШТАБ`
//! (пресеты Y) + `Live`. Кнопки — ТОЧНЫЙ порт `shell::widgets::seg_btn` (lift-фон,
//! hairline-рамка, акцент при active, светлее+акцент на ховере), а не дефолтный
//! gpui-component Button (у него своя геометрия/цвета — расходится со стендом).
//!
//! Размеры/Продажа — действия-заглушки (лог, 1:1 с egui). Масштаб + Live правят вид чарта
//! через состояние в `Backend` (`price_scale`/`follow`), которое применяет `ChartPanel`.

use gpui::prelude::FluentBuilder;
use gpui::*;

use gpui_component::h_flex;

use crate::{hex, Backend};
use moon_core::palette;

/// Высота полосы тулбара (лог. px) — под кнопку 28px + вертикальные отступы. Тонкая.
pub const TOOLBAR_H: f32 = 36.0;

/// Высота кнопки-сегмента (стендовый `seg_btn`: 28px).
const BTN_H: f32 = 28.0;
/// Фикс. ширина кнопок-ключей F1../S1.. (стендовый `key_strip`: 34px).
const KEY_W: f32 = 34.0;

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

/// Сплошной цвет палитры → gpui.
fn solid(c: [u8; 3]) -> Rgba {
    rgb(hex(c))
}

/// Акцент с альфой (0..=255) — для подсветки active/hover, как `gamma_multiply` egui.
fn accent_a(a: u32) -> Rgba {
    rgba((hex(palette::ACCENT) << 8) | (a & 0xff))
}

/// Рамка в покое — еле видимая «hairline» (белый ~9%), стендовый `theme::BORDER`.
fn hairline() -> Rgba {
    rgba(0xffff_ff18)
}

/// Кнопка-сегмент — ТОЧНЫЙ порт `shell::widgets::seg_btn`: высота 28, радиус 4,
/// `active` → заливка accent@16% + рамка accent@70% + яркий текст; покой → lift-фон,
/// hairline-рамка, приглушённый текст; ховер → lift-hover + accent@55% рамка + яркий
/// текст. `fixed_w` — фикс. ширина (ключи 34px), иначе по тексту (паддинг 8px).
fn seg(id: impl Into<SharedString>, label: &str, active: bool, fixed_w: Option<f32>) -> Stateful<Div> {
    let base = div()
        .id(id.into())
        .h(px(BTN_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border_1()
        .text_size(px(11.5))
        .cursor_pointer()
        .map(|d| match fixed_w {
            Some(w) => d.w(px(w)),
            None => d.px(px(8.0)),
        })
        .child(label.to_string());
    if active {
        base.bg(accent_a(0x29)) // accent @ ~16%
            .border_color(accent_a(0xb3)) // accent @ ~70%
            .text_color(solid(palette::TEXT))
    } else {
        base.bg(solid(palette::LIFT))
            .border_color(hairline())
            .text_color(solid(palette::TEXT_2))
            .hover(|s| {
                s.bg(solid(palette::LIFT_HOVER))
                    .border_color(accent_a(0x8c)) // accent @ ~55%
                    .text_color(solid(palette::TEXT))
            })
    }
}

/// Мелкая тусклая подпись группы (`SIZE`/`SELL`/`МАСШТАБ`) — стендовый `.strip-label`.
fn strip_label(text: &'static str) -> impl IntoElement {
    div().text_size(px(9.5)).text_color(solid(palette::TEXT_3)).child(text)
}

/// Вертикальный разделитель групп (стендовый `.divider`): тонкая линия высотой 16px.
fn divider() -> impl IntoElement {
    div().w(px(1.0)).h(px(16.0)).bg(rgba(0xffff_ff12))
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
        .gap(px(5.0))
        .px_3()
        .bg(solid(palette::SURFACE_1))
        .border_b_1()
        .border_color(solid(palette::LIFT_HOVER));

    // --- SIZE: F1..F6 (действия-заглушки, как egui) ---
    row = row.child(strip_label("SIZE"));
    for k in SIZE_KEYS {
        row = row.child(
            seg(format!("size-{k}"), k, false, Some(KEY_W))
                .on_click(move |_, _, _| log::info!("[ui] size {k} (todo)")),
        );
    }
    row = row.child(divider());

    // --- SELL: S1..S6 ---
    row = row.child(strip_label("SELL"));
    for k in SELL_KEYS {
        row = row.child(
            seg(format!("sell-{k}"), k, false, Some(KEY_W))
                .on_click(move |_, _, _| log::info!("[ui] sell {k} (todo)")),
        );
    }
    row = row.child(divider());

    // --- МАСШТАБ: пресеты цены (Y). Активный — accent. Пишем в Backend.price_scale ---
    row = row.child(strip_label("МАСШТАБ"));
    for (label, pct) in SCALES {
        let backend = backend.clone();
        row = row.child(seg(format!("scale-{label}"), label, scale == pct, None).on_click(
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
        seg("live", if follow { "Live" } else { "Пауза" }, follow, None).on_click(move |_, _, cx| {
            backend.update(cx, |b, bcx| {
                b.follow = !b.follow;
                bcx.notify();
            });
        }),
    )
}
