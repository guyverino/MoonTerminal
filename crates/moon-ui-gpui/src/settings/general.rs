//! Вкладка «Общие» — порт egui `settings/general.rs`: язык интерфейса (выпадающий
//! список), отдельная чарт-вкладка на ядро, лог в файлы + срок хранения. Правки идут
//! в draft, применяются после «Сохранить» (язык/чарты — на перезапуске/пересборке окон).

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize, MoonMenuSize, MoonPalette,
    MoonSelect, StyledExt, h_flex, rgba_from, v_flex,
};

use super::SettingsView;
use crate::design;

impl SettingsView {
    /// Изменить срок хранения логов (клампим 0..=365), правит draft.
    fn adjust_ret(&mut self, delta: i32, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                let v = (p.log_retention_days as i32 + delta).clamp(0, 365) as u32;
                if p.log_retention_days != v {
                    p.log_retention_days = v;
                    bcx.notify();
                    changed = true;
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Изменить высоту графика в скролл-стеке (шаг 20px, кламп 120..=2000), правит draft.
    fn adjust_stack_height(&mut self, delta: i32, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                let v = (p.chart_stack_height as i32 + delta).clamp(120, 2000) as u16;
                if p.chart_stack_height != v {
                    p.chart_stack_height = v;
                    bcx.notify();
                    changed = true;
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Переключить bool-поле draft-конфига (общий помощник для галок стека).
    fn toggle_cfg_bool(
        &mut self,
        v: bool,
        get: impl Fn(&mut moon_core::config::AppConfig) -> &mut bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                let slot = get(p);
                if *slot != v {
                    *slot = v;
                    bcx.notify();
                    changed = true;
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Вкладка «Общие» — порт egui `settings/general.rs` точь-в-точь: язык (выпадающий
    /// список) + хинт; разделитель; чекбокс «чарт-вкладка на ядро» + хинт; разделитель;
    /// чекбокс «писать лог в файлы» + хинт; срок хранения (число) + хинт.
    pub(super) fn general_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let (split, scroll, compress, stack_h, logf, ret) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.charts_split_by_core,
                d.charts_stack_scroll,
                d.charts_stack_compress,
                d.chart_stack_height,
                d.log_to_file,
                d.log_retention_days,
            )
        };
        let hint = |t: &str| div().text_color(muted).child(t.to_string());

        v_flex()
            .w_full()
            .gap_1()
            // Язык интерфейса — выпадающий список.
            .child(
                h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(div().font_bold().child("Язык интерфейса"))
                    .child(
                        div().w(px(220.0)).child(
                            MoonSelect::new(&self.lang)
                                .trigger_size(MoonButtonSize::Action)
                                .menu_width(220.0)
                                .menu_size(MoonMenuSize::Compact),
                        ),
                    ),
            )
            .child(hint("Применяется после сохранения."))
            .child(super::separator(p, cx))
            // Отдельная чарт-вкладка на каждое ядро.
            .child(
                MoonCheckbox::new("split")
                    .label("Отдельная чарт-вкладка на каждое ядро")
                    .checked(split)
                    .size(MoonCheckboxSize::Normal)
                    .on_change(cx.listener(|this, ch: &bool, _w, cx| {
                        let v = *ch;
                        let changed = this.backend.update(cx, |b, bcx| {
                            let mut changed = false;
                            if let Some(p) = b.preview.as_mut() {
                                if p.charts_split_by_core != v {
                                    p.charts_split_by_core = v;
                                    bcx.notify();
                                    changed = true;
                                }
                            }
                            changed
                        });
                        if changed {
                            cx.notify();
                        }
                    })),
            )
            .child(hint("AddToChart по умолчанию: вкл — своя вкладка на ядро, выкл — все ядра группы в одной. Колонка «Связка» у ядра (Подключения) переопределяет это: ядра с одинаковым именем связки сводятся в одну вкладку."))
            .child(super::separator(p, cx))
            // Раскладка вкладки с несколькими графиками: скролл / как раньше (масштаб по
            // вертикали). Высота графика и «сжимать по заполнению» активны только при скролле.
            .child(
                MoonCheckbox::new("stack-scroll")
                    .label("Показывать графики со скроллом")
                    .checked(scroll)
                    .size(MoonCheckboxSize::Normal)
                    .on_change(cx.listener(|this, ch: &bool, _w, cx| {
                        let v = *ch;
                        this.toggle_cfg_bool(v, |p| &mut p.charts_stack_scroll, cx);
                    })),
            )
            .child(hint("Вкл — каждый график фиксированной высоты, вертикальный скролл. Выкл (как раньше) — графики делят высоту окна (масштаб по вертикали)."))
            // Высота графика — активна только при скролле (как срок хранения при логах).
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(div().text_color(if scroll { rgba_from(p.text, 1.0) } else { muted }).child("Высота графика, px"))
                    .child(
                        MoonButton::new("stack-h-")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(24.0)
                            .label("-")
                            .disabled(!scroll)
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_stack_height(-20, cx)))
                            .render(),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .text_center()
                            .text_color(if scroll { rgba_from(p.text, 1.0) } else { muted })
                            .child(format!("{stack_h} px")),
                    )
                    .child(
                        MoonButton::new("stack-h+")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(24.0)
                            .label("+")
                            .disabled(!scroll)
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_stack_height(20, cx)))
                            .render(),
                    ),
            )
            // Сжимать по заполнению — только в скролл-режиме.
            .child(
                MoonCheckbox::new("stack-compress")
                    .label("Сжимать по заполнению (без скролла)")
                    .checked(compress)
                    .disabled(!scroll)
                    .size(MoonCheckboxSize::Normal)
                    .on_change(cx.listener(|this, ch: &bool, _w, cx| {
                        let v = *ch;
                        this.toggle_cfg_bool(v, |p| &mut p.charts_stack_compress, cx);
                    })),
            )
            .child(hint("Скролл не появляется: графики рисуются заданной высоты, пока не упрутся в конец окна, затем сжимаются (как без скролла)."))
            .child(super::separator(p, cx))
            // Логи в файлы + срок хранения.
            .child(
                MoonCheckbox::new("logf")
                    .label("Писать лог в файлы")
                    .checked(logf)
                    .size(MoonCheckboxSize::Normal)
                    .on_change(cx.listener(|this, ch: &bool, _w, cx| {
                        let v = *ch;
                        let changed = this.backend.update(cx, |b, bcx| {
                            let mut changed = false;
                            if let Some(p) = b.preview.as_mut() {
                                if p.log_to_file != v {
                                    p.log_to_file = v;
                                    bcx.notify();
                                    changed = true;
                                }
                            }
                            changed
                        });
                        if changed {
                            cx.notify();
                        }
                    })),
            )
            .child(hint("Лог приложения и ядер пишется в logs/<дата>_<источник>.log (по файлу на источник в день)."))
            // Срок хранения активен только при включённой записи лога (порт
            // egui `add_enabled_ui(cfg.log_to_file, ...)`): кнопки −/+ задизейблены,
            // значение/подписи тусклые, пока «Писать лог в файлы» выключено.
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(div().text_color(if logf { rgba_from(p.text, 1.0) } else { muted }).child("Хранить лог, дней"))
                    .child(
                        MoonButton::new("ret-")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(24.0)
                            .label("-")
                            .disabled(!logf)
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_ret(-1, cx)))
                            .render(),
                    )
                    .child(
                        div()
                            .w(px(56.0))
                            .text_center()
                            .text_color(if logf { rgba_from(p.text, 1.0) } else { muted })
                            .child(format!("{ret} дн.")),
                    )
                    .child(
                        MoonButton::new("ret+")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(24.0)
                            .label("+")
                            .disabled(!logf)
                            .on_click(cx.listener(|this, _, _, cx| this.adjust_ret(1, cx)))
                            .render(),
                    ),
            )
            .child(hint("Файлы старше указанного срока удаляются при запуске и раз в сутки. 0 — хранить всё."))
    }
}
