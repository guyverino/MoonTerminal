//! Вкладка «Общие» — порт egui `settings/general.rs`: язык интерфейса (выпадающий
//! список), отдельная чарт-вкладка на ядро, лог в файлы + срок хранения. Правки идут
//! в draft, применяются после «Сохранить» (язык/чарты — на перезапуске/пересборке окон).

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    select::Select,
    v_flex, StyledExt,
};

use super::SettingsView;
use crate::hex;
use moon_core::palette;

impl SettingsView {
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

    /// Вкладка «Общие» — порт egui `settings/general.rs` точь-в-точь: язык (выпадающий
    /// список) + хинт; разделитель; чекбокс «чарт-вкладка на ядро» + хинт; разделитель;
    /// чекбокс «писать лог в файлы» + хинт; срок хранения (число) + хинт.
    pub(super) fn general_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let muted = rgb(hex(palette::TEXT_2));
        let (split, logf, ret) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (d.charts_split_by_core, d.log_to_file, d.log_retention_days)
        };
        let hint = |t: &str| div().text_color(muted).child(t.to_string());

        v_flex()
            .w_full()
            .gap_1()
            // Язык интерфейса — выпадающий список.
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().font_bold().child("Язык интерфейса"))
                    .child(div().w(px(220.0)).child(Select::new(&self.lang))),
            )
            .child(hint("Применяется после сохранения."))
            .child(super::separator())
            // Отдельная чарт-вкладка на каждое ядро.
            .child(
                Checkbox::new("split")
                    .label("Отдельная чарт-вкладка на каждое ядро")
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
            .child(hint("AddToChart: вкл — 1-HL-ядро (своя вкладка на ядро), выкл — все ядра в одной 1-HL."))
            .child(super::separator())
            // Логи в файлы + срок хранения.
            .child(
                Checkbox::new("logf")
                    .label("Писать лог в файлы")
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
            .child(hint("Лог приложения и ядер пишется в logs/<дата>_<источник>.log (по файлу на источник в день)."))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().child("Хранить лог, дней"))
                    .child(
                        Button::new("ret-")
                            .ghost()
                            .label("−")
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_ret(-1, cx))),
                    )
                    .child(div().w(px(56.0)).text_center().child(format!("{ret} дн.")))
                    .child(
                        Button::new("ret+")
                            .ghost()
                            .label("+")
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_ret(1, cx))),
                    ),
            )
            .child(hint("Файлы старше указанного срока удаляются при запуске и раз в сутки. 0 — хранить всё."))
    }
}
