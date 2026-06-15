//! Вкладка «Подключения» — порт egui `settings/connections.rs`: слева таблица ядер
//! (Акт·Окно·Имя·Ключ·Группа·[Данные n/8]·Цвет·Удалить·↻реконнект·●статус), справа
//! панель групп (галка·иконка·имя·👁показать·выбор иконки + пикер). Над ними — источник
//! рыночных данных (выпадающий). Правки идут в draft; статус/реконнект — через `Backend`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_palette::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonColorPicker,
    MoonColorPickerEvent, MoonColorPickerState, MoonDropdown, MoonInput, MoonInputEvent,
    MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonSelect, MoonTooltipView,
    StyledExt, h_flex, v_flex,
};

use super::{SettingsView, hsla_u8};
use crate::{Backend, hex};
use moon_core::config::{FeedFlags, GroupConfig, Secret, ServerConfig};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

/// Редактор одной строки сервера: текст-поля + цвет (entity-стейты компонентов).
pub(super) struct ConnRow {
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    group: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
}

/// 8 фид-флагов приёма данных ядра (локализованная подпись, геттер, сеттер) — для
/// поповера «Данные». Подписи = строки локали `conn.tip.*` (RU), к каждой в поповере
/// добавляется суффикс «(фильтр на клиенте)» (`conn.filter_note`).
const FEED_FLAGS: [(&str, fn(&FeedFlags) -> bool, fn(&mut FeedFlags, bool)); 8] = [
    ("Открытые ордера", |f| f.orders, |f, v| f.orders = v),
    ("Детекты", |f| f.detects, |f, v| f.detects = v),
    (
        "Отчёты по закрытым ордерам → SQLite",
        |f| f.reports,
        |f, v| f.reports = v,
    ),
    ("Балансы / аккаунт", |f| f.balance, |f, v| f.balance = v),
    ("Стратегии", |f| f.strategies, |f, v| f.strategies = v),
    ("Серверный лог", |f| f.log, |f, v| f.log = v),
    ("Chart-алерты / текст", |f| f.alerts, |f, v| f.alerts = v),
    ("Арбитраж", |f| f.arb, |f, v| f.arb = v),
];

fn u32_rgb(c: u32) -> [u8; 3] {
    [
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    ]
}

/// TextInput, привязанный к полю сервера `servers[i]` (пишет в draft).
fn conn_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: String,
    get: fn(&ServerConfig) -> String,
    set: fn(&mut ServerConfig, String),
) -> Entity<MoonInputState> {
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(s) = p.servers.get_mut(i) {
                        if get(s) != val {
                            set(s, val);
                            bcx.notify();
                        }
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
) -> Entity<MoonColorPickerState> {
    let st =
        cx.new(|cx| MoonColorPickerState::new(window, cx).default_value(rgb(hex(init)).into()));
    cx.subscribe(&st, move |this, _e, ev: &MoonColorPickerEvent, cx| {
        let MoonColorPickerEvent::Change(h) = ev;
        let c = hsla_u8(*h);
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if let Some(s) = p.servers.get_mut(i) {
                    if s.color != c {
                        s.color = c;
                        bcx.notify();
                    }
                }
            }
        });
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
            name: conn_input(
                window,
                cx,
                i,
                s.name.clone(),
                |s| s.name.clone(),
                |s, v| s.name = v,
            ),
            // Ключ — поле пароля (порт egui `.password(true)`): символы скрыты, рядом
            // переключатель видимости (mask_toggle), чтобы при необходимости показать.
            key: {
                let st = conn_input(
                    window,
                    cx,
                    i,
                    s.key.expose().to_string(),
                    |s| s.key.expose().to_string(),
                    |s, v| s.key = Secret::new(v),
                );
                st.update(cx, |st, c| st.set_masked(true, window, c));
                st
            },
            group: conn_input(
                window,
                cx,
                i,
                s.group.clone(),
                |s| s.group.clone(),
                |s, v| s.group = v,
            ),
            color: conn_color(window, cx, i, s.color),
        })
        .collect()
}

/// Кружок статуса подключения ядра (порт egui `status_dot`): зелёный=Ready, акцент=
/// подключается, красный=ошибка, серый=неактивно/нет. `active=false` → всегда серый.
/// Тултип поясняет состояние (для Failed — текст ошибки), как egui `on_hover_text`.
fn status_dot(
    i: usize,
    active: bool,
    status: Option<&ConnStatus>,
    p: MoonPalette,
) -> impl IntoElement {
    let (color, tip) = match status {
        _ if !active => (
            p.text_soft,
            "Не подключается (галка «Акт» снята)".to_string(),
        ),
        Some(ConnStatus::Ready) => (p.green, "Подключено".to_string()),
        Some(ConnStatus::Connecting) => (p.amber, "Подключение…".to_string()),
        Some(ConnStatus::Stage(s)) => (p.amber, format!("Подключение: {s}")),
        Some(ConnStatus::Failed(e)) => (p.red, format!("Ошибка: {e}")),
        Some(ConnStatus::Disconnected) => (p.text_soft, "Отключено".to_string()),
        None => (
            p.text_soft,
            "Нет данных (сохрани настройки, чтобы подключиться)".to_string(),
        ),
    };
    div()
        .id(SharedString::from(format!("st-{i}")))
        .w(px(10.0))
        .h(px(10.0))
        .rounded_full()
        .bg(rgb(color))
        .tooltip(move |_window, cx| {
            cx.new(|_| MoonTooltipView::new(tip.clone()).max_width(320.0))
                .into()
        })
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
            b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .servers
                .get(i)
                .map(get)
                .unwrap_or(false)
        };
        let mut checkbox = MoonCheckbox::new(SharedString::from(format!("{suffix}-{i}")))
            .checked(cur)
            .size(MoonCheckboxSize::Compact)
            .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                let v = *ch;
                let changed = this.backend.update(cx, |b, bcx| {
                    let mut changed = false;
                    if let Some(p) = b.preview.as_mut() {
                        if let Some(s) = p.servers.get_mut(i) {
                            if get(s) != v {
                                set(s, v);
                                bcx.notify();
                                changed = true;
                            }
                        }
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            }));
        if !label.is_empty() {
            checkbox = checkbox.label(label);
        }
        checkbox
    }

    /// Добавить сервер в draft (id = max+1) и пересобрать editor-стейты.
    fn add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_color = u32_rgb(MoonPalette::active(cx).amber);
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
                    color: default_color,
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
            let s = b.preview.as_ref().unwrap_or(&b.config).servers.get(i);
            s.map(|s| s.feed.clone()).unwrap_or_default()
        };
        let on = FEED_FLAGS.iter().filter(|(_, g, _)| g(&feed)).count();
        let tinted = on < FEED_FLAGS.len();

        let mut items = Vec::new();
        for (ix, (lbl, get, set)) in FEED_FLAGS.iter().copied().enumerate() {
            let cur = get(&feed);
            let backend = self.backend.clone();
            items.push(
                MoonMenuItem::with_key(
                    format!("feed-{i}-{ix}"),
                    format!("{lbl} (фильтр на клиенте)"),
                )
                .checked(cur)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            if let Some(s) = p.servers.get_mut(i) {
                                set(&mut s.feed, !cur);
                                bcx.notify();
                            }
                        }
                    });
                }),
            );
        }

        MoonDropdown::new(SharedString::from(format!("feed-{i}")))
            .label(format!("{on}/8"))
            .trigger_variant(if tinted {
                MoonButtonVariant::Amber
            } else {
                MoonButtonVariant::Neutral
            })
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(52.0)
            .menu_width(272.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .items(items)
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
            div()
                .id(SharedString::from(format!("rec-tip-{i}")))
                .tooltip(|_window, cx| cx.new(|_| MoonTooltipView::new("Переподключить")).into())
                .child(
                    MoonButton::new(SharedString::from(format!("rec-{i}")))
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .width(24.0)
                        .label("↻")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.backend.update(cx, |b, bcx| {
                                b.reconnect_request.push(core_id);
                                bcx.notify();
                            });
                        }))
                        .render(),
                )
                .into_any_element()
        } else {
            div().w(px(24.0)).into_any_element()
        };
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .py_0p5()
            .child(div().w(px(28.0)).child(self.srv_check(
                cx,
                i,
                "act",
                "",
                |s| s.active,
                |s, v| s.active = v,
            )))
            .child(div().w(px(34.0)).child(self.srv_check(
                cx,
                i,
                "win",
                "",
                |s| s.show_window,
                |s, v| s.show_window = v,
            )))
            .child(
                div().w(px(150.0)).child(
                    MoonInput::new(SharedString::from(format!("name-{i}")))
                        .state(&row.name)
                        .small(),
                ),
            )
            .child(
                div().w(px(200.0)).child(
                    MoonInput::new(SharedString::from(format!("key-{i}")))
                        .state(&row.key)
                        .small()
                        .mask_toggle(),
                ),
            )
            .child(
                div().w(px(110.0)).child(
                    MoonInput::new(SharedString::from(format!("group-{i}")))
                        .state(&row.group)
                        .small(),
                ),
            )
            .child(self.feed_popover(cx, i))
            .child(MoonColorPicker::new(&row.color))
            .child(
                MoonButton::new(SharedString::from(format!("del-{i}")))
                    .danger()
                    .size(MoonButtonSize::Micro)
                    .width(24.0)
                    .label("x")
                    .on_click(cx.listener(move |this, _, w, cx| this.delete_server(i, w, cx)))
                    .render(),
            )
            .child(recon)
            .child(status_dot(
                i,
                active,
                status.as_ref(),
                MoonPalette::active(cx),
            ))
    }

    /// Заголовок колонки таблицы серверов (тусклая подпись фикс. ширины).
    fn col_head(label: &str, w: f32, p: MoonPalette) -> impl IntoElement {
        div()
            .w(px(w))
            .text_xs()
            .text_color(rgb(p.text_soft))
            .child(label.to_string())
    }

    /// Заголовок колонки с тултипом (порт egui `head_tip`): для сокращённых подписей
    /// галок «Акт»/«Окн» и кнопки «Данные».
    fn col_head_tip(
        id: &'static str,
        label: &str,
        w: f32,
        tip: &'static str,
        p: MoonPalette,
    ) -> impl IntoElement {
        div()
            .id(id)
            .w(px(w))
            .text_xs()
            .text_color(rgb(p.text_soft))
            .child(label.to_string())
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tip).max_width(360.0))
                    .into()
            })
    }

    /// Вкладка «Подключения» — порт egui `settings/connections.rs`: источник данных
    /// (выпадающий), таблица ядер слева, панель групп (с иконками/👁/пикером) справа.
    pub(super) fn connections_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
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
                d.servers
                    .iter()
                    .map(|s| (s.id, s.active))
                    .collect::<Vec<_>>(),
                d.groups
                    .iter()
                    .map(|g| (g.name.clone(), g.active, g.icon))
                    .collect::<Vec<_>>(),
            )
        };
        // Предзагрузить иконки (групп + весь набор, если открыт пикер) — texture() берёт
        // &mut self.icons, поэтому грузим ДО построения UI, потом читаем из карты.
        let picking = self.picking.clone();
        let mut icon_tex: HashMap<u32, Option<Arc<RenderImage>>> = HashMap::new();
        for (_, _, icon) in &groups {
            icon_tex
                .entry(*icon)
                .or_insert_with(|| self.icons.texture(*icon));
        }
        let pick_ids: Vec<u32> = if picking.is_some() {
            (0..self.icons.count).collect()
        } else {
            Vec::new()
        };
        for id in &pick_ids {
            icon_tex
                .entry(*id)
                .or_insert_with(|| self.icons.texture(*id));
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
                    .child(Self::col_head_tip("h-act", "Акт", 28.0, "Подключаться к ядру", p))
                    .child(Self::col_head_tip("h-win", "Окн", 34.0, "Рисовать окно/чарт. Выкл = headless: данные в БД/память без окна", p))
                    .child(Self::col_head("Имя", 150.0, p))
                    .child(Self::col_head("Ключ", 200.0, p))
                    .child(Self::col_head("Группа", 110.0, p))
                    .child(Self::col_head_tip("h-data", "Данные", 52.0, "Приём данных от ядра. Серая = принимаем всё; цветная = часть категорий выключена. Клик — настроить.", p)),
            );
        for (i, (id, active)) in servers.iter().enumerate() {
            if let Some(row) = self.conn.get(i) {
                let st = status.get(id).cloned();
                servers_col = servers_col.child(self.server_row(cx, i, row, *id, *active, st));
            }
        }
        servers_col = servers_col.child(
            MoonButton::new("add-srv")
                .outline()
                .small()
                .width(130.0)
                .label("+ Добавить ядро")
                .on_click(cx.listener(|this, _, w, cx| this.add_server(w, cx)))
                .render(),
        );

        // ── Правая колонка: группы ───────────────────────────────────────────
        let mut groups_col = v_flex()
            .w(px(240.0))
            .gap_1()
            .child(div().font_bold().child("Группы"));
        // Нет групп (ни у одного сервера не задана) → поясняющий хинт (порт egui
        // `conn.no_groups`), как и в оригинале вместо пустого списка.
        if groups.is_empty() {
            groups_col = groups_col.child(
                div()
                    .text_color(rgb(p.text_soft))
                    .child("задай группы серверам слева"),
            );
        }
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
                        MoonCheckbox::new(SharedString::from(format!("grp-{name}")))
                            .checked(*active)
                            .size(MoonCheckboxSize::Compact)
                            .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                                let v = *ch;
                                let n = nm_act.clone();
                                this.backend.update(cx, |b, bcx| {
                                    if let Some(p) = b.preview.as_mut() {
                                        if let Some(gc) = p.groups.iter_mut().find(|g| g.name == n)
                                        {
                                            gc.active = v;
                                            bcx.notify();
                                        }
                                    }
                                });
                                cx.notify();
                            })),
                    )
                    .child(ico_el)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_bold()
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("eye-tip-{name}")))
                            .tooltip(|_window, cx| {
                                cx.new(|_| MoonTooltipView::new("Показать окно группы"))
                                    .into()
                            })
                            .child(
                                MoonButton::new(SharedString::from(format!("eye-{name}")))
                                    .ghost()
                                    .size(MoonButtonSize::Micro)
                                    .width(34.0)
                                    .label("win")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let n = nm_eye.clone();
                                        this.backend.update(cx, |b, bcx| {
                                            b.show_group_request.push(n);
                                            bcx.notify();
                                        });
                                    }))
                                    .render(),
                            ),
                    )
                    .child(
                        MoonButton::new(SharedString::from(format!("pick-{name}")))
                            .outline()
                            .size(MoonButtonSize::Micro)
                            .width(54.0)
                            .label("Иконка")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.picking = Some(nm_pick.clone());
                                cx.notify();
                            }))
                            .render(),
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
                        .hover(move |s| s.bg(rgb(p.panel_high)))
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
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(rgb(p.text_soft))
                                .child(format!("Иконка для «{pick}»")),
                        )
                        .child(
                            MoonButton::new("pick-close")
                                .ghost()
                                .size(MoonButtonSize::Micro)
                                .width(24.0)
                                .label("x")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.picking = None;
                                    cx.notify();
                                }))
                                .render(),
                        ),
                )
                .child(
                    div()
                        .id("icon-picker")
                        .max_h(px(220.0))
                        .overflow_y_scroll()
                        .child(grid),
                );
        }

        v_flex()
            .w_full()
            .gap_2()
            // Источник рыночных данных — выпадающий список (порт egui ComboBox).
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("market-src-lbl")
                            .font_bold()
                            .child("Источник рыночных данных")
                            .tooltip(|_window, cx| {
                                cx.new(|_| MoonTooltipView::new(
                                    "Откуда брать крестики и стакан. Дедуп: одно ядро-провайдер на биржу тянет рынок за всех (экономно при многих ядрах). По ядрам: каждый чарт берёт рынок со своего ядра (без дедупа).",
                                )
                                .max_width(420.0))
                                .into()
                            }),
                    )
                    .child(
                        div().w(px(260.0)).child(
                            MoonSelect::new(&self.mode)
                                .trigger_size(MoonButtonSize::Action)
                                .menu_width(260.0)
                                .menu_size(MoonMenuSize::Compact),
                        ),
                    ),
            )
            .child(h_flex().w_full().gap_4().items_start().child(servers_col).child(groups_col))
    }
}
