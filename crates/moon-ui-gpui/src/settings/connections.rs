//! Вкладка «Подключения» — порт egui `settings/connections.rs`: слева таблица ядер
//! (Акт·Окно·Имя·Ключ·Группа·[Данные n/8]·Цвет·Удалить·↻реконнект·●статус), справа
//! панель групп (галка·иконка·имя·👁показать·выбор иконки + пикер). Над ними — источник
//! рыночных данных (выпадающий). Правки идут в draft; статус/реконнект — через `Backend`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    select::Select,
    v_flex, Sizable, StyledExt,
};

use super::{hsla_u8, SettingsView};
use crate::{hex, Backend};
use moon_core::config::{FeedFlags, GroupConfig, Secret, ServerConfig};
use moon_core::feed::ConnStatus;
use moon_core::palette;
use moon_core::session::CoreId;

/// Редактор одной строки сервера: текст-поля + цвет (entity-стейты компонентов).
pub(super) struct ConnRow {
    name: Entity<InputState>,
    key: Entity<InputState>,
    group: Entity<InputState>,
    color: Entity<ColorPickerState>,
}

/// 8 фид-флагов приёма данных ядра (подпись, геттер, сеттер) — для поповера «Данные».
const FEED_FLAGS: [(&str, fn(&FeedFlags) -> bool, fn(&mut FeedFlags, bool)); 8] = [
    ("orders", |f| f.orders, |f, v| f.orders = v),
    ("detects", |f| f.detects, |f, v| f.detects = v),
    ("reports", |f| f.reports, |f, v| f.reports = v),
    ("balance", |f| f.balance, |f, v| f.balance = v),
    ("strategies", |f| f.strategies, |f, v| f.strategies = v),
    ("log", |f| f.log, |f, v| f.log = v),
    ("alerts", |f| f.alerts, |f, v| f.alerts = v),
    ("arb", |f| f.arb, |f, v| f.arb = v),
];

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

/// Построить per-server editor-стейты из draft-серверов. Зовётся в `SettingsView::new`
/// и после add/remove сервера (индексы в подписках свежие).
pub(super) fn build_conn(
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

/// Кружок статуса подключения ядра (порт egui `status_dot`): зелёный=Ready, акцент=
/// подключается, красный=ошибка, серый=неактивно/нет. `active=false` → всегда серый.
fn status_dot(active: bool, status: Option<&ConnStatus>) -> impl IntoElement {
    let color = match status {
        _ if !active => palette::TEXT_2,
        Some(ConnStatus::Ready) => palette::GREEN,
        Some(ConnStatus::Connecting) | Some(ConnStatus::Stage(_)) => palette::ACCENT,
        Some(ConnStatus::Failed(_)) => palette::RED,
        _ => palette::TEXT_2,
    };
    div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(hex(color)))
}

impl SettingsView {
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

    /// Поповер «Данные n/8» (порт egui `feed_button`): кнопка с числом включённых
    /// фид-флагов ядра; клик раскрывает 8 чекбоксов приёма данных (пишут в draft).
    fn feed_popover(&self, cx: &Context<Self>, i: usize) -> impl IntoElement {
        let feed = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).servers.get(i).map(|s| s.feed.clone())
        }
        .unwrap_or_default();
        let on = FEED_FLAGS.iter().filter(|(_, g, _)| g(&feed)).count();
        let backend = self.backend.clone();
        Popover::new(SharedString::from(format!("feed-{i}")))
            .trigger(
                Button::new(SharedString::from(format!("feedbtn-{i}")))
                    .outline()
                    .xsmall()
                    .label(format!("{on}/8")),
            )
            .content(move |_state, _window, cx| {
                let pop = cx.entity();
                let mut col = v_flex().gap_1().p_2().min_w(px(180.0));
                for (lbl, get, set) in FEED_FLAGS {
                    let cur = {
                        let b = backend.read(cx);
                        b.preview.as_ref().unwrap_or(&b.config).servers.get(i).map(|s| get(&s.feed)).unwrap_or(false)
                    };
                    let backend = backend.clone();
                    let pop = pop.clone();
                    col = col.child(
                        Checkbox::new(SharedString::from(format!("feed-{i}-{lbl}")))
                            .label(lbl)
                            .checked(cur)
                            .on_click(move |v: &bool, _w, app| {
                                let v = *v;
                                backend.update(app, |b, bcx| {
                                    if let Some(p) = b.preview.as_mut() {
                                        if let Some(s) = p.servers.get_mut(i) {
                                            set(&mut s.feed, v);
                                            bcx.notify();
                                        }
                                    }
                                });
                                pop.update(app, |_, c| c.notify());
                            }),
                    );
                }
                col
            })
    }

    /// Строка сервера в таблице (порт egui `servers_panel` row): Акт·Окно·Имя·Ключ·
    /// Группа·[Данные]·Цвет·Удалить·↻реконнект·●статус.
    fn server_row(
        &self,
        cx: &Context<Self>,
        i: usize,
        row: &ConnRow,
        core_id: CoreId,
        active: bool,
        status: Option<ConnStatus>,
    ) -> impl IntoElement {
        // Реконнект — только для активных ядер (у неактивных нет сессии).
        let recon: AnyElement = if active {
            Button::new(SharedString::from(format!("rec-{i}")))
                .ghost()
                .xsmall()
                .label("↻")
                .tooltip("Переподключить")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.backend.update(cx, |b, bcx| {
                        b.reconnect_request.push(core_id);
                        bcx.notify();
                    });
                }))
                .into_any_element()
        } else {
            div().w(px(24.0)).into_any_element()
        };
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .py_0p5()
            .child(div().w(px(28.0)).child(self.srv_check(cx, i, "act", "", |s| s.active, |s, v| s.active = v)))
            .child(div().w(px(34.0)).child(self.srv_check(cx, i, "win", "", |s| s.show_window, |s, v| s.show_window = v)))
            .child(div().w(px(150.0)).child(Input::new(&row.name)))
            .child(div().w(px(200.0)).child(Input::new(&row.key)))
            .child(div().w(px(110.0)).child(Input::new(&row.group)))
            .child(self.feed_popover(cx, i))
            .child(ColorPicker::new(&row.color))
            .child(
                Button::new(SharedString::from(format!("del-{i}")))
                    .danger()
                    .xsmall()
                    .label("✕")
                    .on_click(cx.listener(move |this, _, w, cx| this.delete_server(i, w, cx))),
            )
            .child(recon)
            .child(status_dot(active, status.as_ref()))
    }

    /// Заголовок колонки таблицы серверов (тусклая подпись фикс. ширины).
    fn col_head(label: &str, w: f32) -> impl IntoElement {
        div().w(px(w)).text_xs().text_color(rgb(hex(palette::TEXT_2))).child(label.to_string())
    }

    /// Вкладка «Подключения» — порт egui `settings/connections.rs`: источник данных
    /// (выпадающий), таблица ядер слева, панель групп (с иконками/👁/пикером) справа.
    pub(super) fn connections_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Живой статус ядер для точек.
        let status = self.backend.read(cx).session.status_map();
        // Синхронизировать группы draft с именами групп серверов (создать недостающие,
        // убрать сироты) — порт egui groups_panel.
        self.backend.update(cx, |b, _| {
            if let Some(p) = b.preview.as_mut() {
                let mut names: Vec<String> = p.servers.iter().map(|s| s.group.clone()).collect();
                names.sort();
                names.dedup();
                p.groups.retain(|g| names.contains(&g.name));
                for n in &names {
                    if !p.groups.iter().any(|g| &g.name == n) {
                        p.groups.push(GroupConfig::new(n.clone()));
                    }
                }
            }
        });
        // Снимки серверов (id, active) и групп (name, active, icon).
        let (servers, groups) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.servers.iter().map(|s| (s.id, s.active)).collect::<Vec<_>>(),
                d.groups.iter().map(|g| (g.name.clone(), g.active, g.icon)).collect::<Vec<_>>(),
            )
        };
        // Предзагрузить иконки (групп + весь набор, если открыт пикер) — texture() берёт
        // &mut self.icons, поэтому грузим ДО построения UI, потом читаем из карты.
        let picking = self.picking.clone();
        let mut icon_tex: HashMap<u32, Option<Arc<RenderImage>>> = HashMap::new();
        for (_, _, icon) in &groups {
            icon_tex.entry(*icon).or_insert_with(|| self.icons.texture(*icon));
        }
        let pick_ids: Vec<u32> = if picking.is_some() { (0..self.icons.count).collect() } else { Vec::new() };
        for id in &pick_ids {
            icon_tex.entry(*id).or_insert_with(|| self.icons.texture(*id));
        }

        // ── Левая колонка: таблица серверов ──────────────────────────────────
        let mut servers_col = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(div().font_bold().child("Ядра (серверы)"))
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(Self::col_head("Акт", 28.0))
                    .child(Self::col_head("Окн", 34.0))
                    .child(Self::col_head("Имя", 150.0))
                    .child(Self::col_head("Ключ", 200.0))
                    .child(Self::col_head("Группа", 110.0))
                    .child(Self::col_head("Данные", 52.0)),
            );
        for (i, (id, active)) in servers.iter().enumerate() {
            if let Some(row) = self.conn.get(i) {
                let st = status.get(id).cloned();
                servers_col = servers_col.child(self.server_row(cx, i, row, *id, *active, st));
            }
        }
        servers_col = servers_col.child(
            Button::new("add-srv")
                .outline()
                .label("+ Ядро")
                .on_click(cx.listener(|this, _, w, cx| this.add_server(w, cx))),
        );

        // ── Правая колонка: группы ───────────────────────────────────────────
        let mut groups_col = v_flex()
            .w(px(240.0))
            .gap_1()
            .child(div().font_bold().child("Группы"));
        for (name, active, icon) in &groups {
            let nm_act = name.clone();
            let nm_eye = name.clone();
            let nm_pick = name.clone();
            let ico_el: AnyElement = match icon_tex.get(icon).and_then(|t| t.clone()) {
                Some(arc) => img(arc).w(px(20.0)).h(px(20.0)).into_any_element(),
                None => div().w(px(20.0)).h(px(20.0)).into_any_element(),
            };
            groups_col = groups_col.child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        Checkbox::new(SharedString::from(format!("grp-{name}")))
                            .checked(*active)
                            .on_click(cx.listener(move |this, ch: &bool, _w, cx| {
                                let v = *ch;
                                let n = nm_act.clone();
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
                    )
                    .child(ico_el)
                    .child(div().flex_1().min_w_0().truncate().font_bold().child(name.clone()))
                    .child(
                        Button::new(SharedString::from(format!("eye-{name}")))
                            .ghost()
                            .xsmall()
                            .label("👁")
                            .tooltip("Показать окно группы")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let n = nm_eye.clone();
                                this.backend.update(cx, |b, bcx| {
                                    b.show_group_request.push(n);
                                    bcx.notify();
                                });
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("pick-{name}")))
                            .outline()
                            .xsmall()
                            .label("Иконка")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.picking = Some(nm_pick.clone());
                                cx.notify();
                            })),
                    ),
            );
        }
        // Пикер иконок для выбранной группы.
        if let Some(pick) = picking {
            let mut grid = h_flex().w_full().flex_wrap().gap_1();
            for id in pick_ids {
                let cell: AnyElement = match icon_tex.get(&id).and_then(|t| t.clone()) {
                    Some(arc) => img(arc).w(px(22.0)).h(px(22.0)).into_any_element(),
                    None => continue,
                };
                let nm = pick.clone();
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!("ico-{id}")))
                        .p_0p5()
                        .cursor_pointer()
                        .rounded(px(4.0))
                        .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
                        .child(cell)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let n = nm.clone();
                            this.backend.update(cx, |b, bcx| {
                                if let Some(p) = b.preview.as_mut() {
                                    if let Some(g) = p.groups.iter_mut().find(|g| g.name == n) {
                                        g.icon = id;
                                        bcx.notify();
                                    }
                                }
                            });
                            this.picking = None;
                            cx.notify();
                        })),
                );
            }
            groups_col = groups_col
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().text_xs().text_color(rgb(hex(palette::TEXT_2))).child(format!("Иконка для «{pick}»")))
                        .child(
                            Button::new("pick-close")
                                .ghost()
                                .xsmall()
                                .label("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.picking = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(div().id("icon-picker").max_h(px(220.0)).overflow_y_scroll().child(grid));
        }

        v_flex()
            .w_full()
            .gap_2()
            // Источник рыночных данных — выпадающий список (порт egui ComboBox).
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().font_bold().child("Источник рыночных данных"))
                    .child(div().w(px(260.0)).child(Select::new(&self.mode))),
            )
            .child(h_flex().w_full().gap_4().items_start().child(servers_col).child(groups_col))
    }
}
