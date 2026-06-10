//! Вкладка «Подключения»: слева сервера, справа группы.
//! Сервер: Акт · Окн · Имя · Ключ · Группа · [Данные] · Цвет(детекта) · Удалить.
//! Host/Port не вводим — они зашиты в ключе. Галки приёма данных свёрнуты в одну
//! кнопку «Данные» (серая = принимаем всё; цвет сервера = часть выключена).
//! Группа: Акт(вся группа) · иконка · имя · выбор иконки.

use super::SettingsTab;
use crate::config::servers::{default_color, default_group, default_market};
use crate::config::{AppConfig, GroupConfig, Secret, ServerConfig};
use crate::icons::IconSet;
use crate::market::MarketDataMode;
use crate::shell::theme;

#[derive(Default)]
pub struct ConnectionsTab {
    /// Для какой группы открыт picker иконок.
    picking: Option<String>,
}

const H: f32 = 22.0;

impl SettingsTab for ConnectionsTab {
    fn title(&self) -> String {
        t!("tab.connections").to_string()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &mut AppConfig,
        icons: &mut IconSet,
        status: &super::CoreStatuses,
        actions: &mut super::SettingsActions,
    ) {
        let picking = &mut self.picking;
        // Источник рыночных данных (глобально для всех ядер) — над таблицами.
        egui::TopBottomPanel::top("market_src_bar")
            .resizable(false)
            .show_inside(ui, |ui| {
                market_mode_row(ui, cfg);
            });
        // Группы — справа в SidePanel: он резервирует свою ширину ДО соседа, поэтому
        // широкая таблица серверов не может вытолкнуть его за край окна (как было с
        // двумя set_width-колонками — set_width лишь мягкий максимум, add_sized всё
        // равно раздувал левую колонку и сдвигал правую off-screen).
        let total = ui.available_width();
        let right_w = (total / 5.0).clamp(170.0, 240.0);
        egui::SidePanel::right("groups_side")
            .resizable(false)
            .exact_width(right_w)
            .show_inside(ui, |ui| {
                groups_panel(ui, cfg, icons, picking, actions);
            });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            servers_panel(ui, cfg, status, actions);
        });
    }
}

/// Глобальный тумблер источника рыночных данных (дедуп по провайдеру / по ядрам).
fn market_mode_row(ui: &mut egui::Ui, cfg: &mut AppConfig) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(t!("conn.market_src")).strong())
            .on_hover_text(t!("conn.market_src_tip"));
        let selected = match cfg.market_mode {
            MarketDataMode::Dedup => t!("conn.market_dedup"),
            MarketDataMode::PerCore => t!("conn.market_percore"),
        };
        egui::ComboBox::from_id_salt("market_mode")
            .selected_text(selected.to_string())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut cfg.market_mode,
                    MarketDataMode::Dedup,
                    t!("conn.market_dedup").to_string(),
                );
                ui.selectable_value(
                    &mut cfg.market_mode,
                    MarketDataMode::PerCore,
                    t!("conn.market_percore").to_string(),
                );
            })
            .response
            .on_hover_text(t!("conn.market_src_tip"));
    });
    ui.add_space(4.0);
}

fn servers_panel(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    status: &super::CoreStatuses,
    actions: &mut super::SettingsActions,
) {
    ui.label(egui::RichText::new(t!("conn.servers_heading")).strong());
    ui.add_space(6.0);

    let w_act = 20.0;
    let w_win = 20.0;
    let w_data = 46.0; // кнопка-свёртка галок приёма
    // Color-кнопка рисуется в свою НАТУРАЛЬНУЮ ширину (≈interact_size), а
    // allocate_ui её не ограничивает — поэтому берём реальную ширину из стиля,
    // иначе строка окажется шире бюджета и «Удал» уедет под правую панель.
    let w_color = ui.spacing().interact_size.x;
    let w_del = 44.0;
    let w_recon = 24.0; // кнопка переподключения
    let w_status = 16.0; // кружок статуса подключения в конце строки
    let sp = ui.spacing().item_spacing.x;
    // Фиксированные (нерастяжимые) ширины + межвиджетные отступы. В строке 10
    // виджетов → 9 промежутков. Остаток делим между 3 текстовыми полями.
    let fixed = w_act + w_win + w_data + w_color + w_del + w_recon + w_status;
    let gaps = sp * 9.0;
    let flex = (ui.available_width() - fixed - gaps - 4.0).max(120.0);
    let w_name = flex * 0.30;
    let w_key = flex * 0.45;
    let w_group = flex * 0.25;

    egui::ScrollArea::vertical()
        .id_salt("servers_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                head_tip(ui, w_act, t!("conn.col.act"), t!("conn.tip.act"));
                head_tip(ui, w_win, t!("conn.col.win"), t!("conn.tip.win"));
                head(ui, w_name, t!("conn.col.name"));
                head(ui, w_key, t!("conn.col.key"));
                head(ui, w_group, t!("conn.col.group"));
                head_tip(ui, w_data, t!("conn.col.data"), t!("conn.tip.flags"));
                head(ui, w_color, t!("conn.col.color"));
                head(ui, w_del, "");
                head(ui, w_recon, "");
                head(ui, w_status, "");
            });

            let mut remove = None;
            for i in 0..cfg.servers.len() {
                let s = &mut cfg.servers[i];
                ui.horizontal(|ui| {
                    flag(ui, w_act, &mut s.active, t!("conn.tip.act"));
                    flag(ui, w_win, &mut s.show_window, t!("conn.tip.win"));
                    ui.add_sized([w_name, H], egui::TextEdit::singleline(&mut s.name));
                    ui.add_sized(
                        [w_key, H],
                        egui::TextEdit::singleline(s.key.buffer_mut())
                            .password(true)
                            .hint_text("key"),
                    );
                    ui.add_sized([w_group, H], egui::TextEdit::singleline(&mut s.group));
                    feed_button(ui, s, w_data);
                    ui.allocate_ui(egui::vec2(w_color, H), |ui| {
                        let mut col =
                            egui::Color32::from_rgb(s.color[0], s.color[1], s.color[2]);
                        if ui.color_edit_button_srgba(&mut col).changed() {
                            s.color = [col.r(), col.g(), col.b()];
                        }
                    });
                    let del = t!("conn.delete").to_string();
                    if theme::seg_btn_h(ui, &del, false, Some(w_del), false, H).clicked() {
                        remove = Some(i);
                    }
                    // Реконнект — только для активных ядер (у неактивных нет сессии).
                    if s.active {
                        let r = theme::seg_btn_h(ui, "↻", false, Some(w_recon), false, H)
                            .on_hover_text(t!("conn.reconnect"));
                        if r.clicked() {
                            actions.reconnect.push(s.id);
                        }
                    } else {
                        ui.allocate_exact_size(egui::vec2(w_recon, H), egui::Sense::hover());
                    }
                    status_dot(ui, w_status, s.active, status.get(&s.id));
                });
            }
            if let Some(i) = remove {
                cfg.servers.remove(i);
            }
        });

    ui.add_space(8.0);
    let add = t!("conn.add_core").to_string();
    if theme::seg_btn_h(ui, &add, false, None, false, H).clicked() {
        let next_id = cfg.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        cfg.servers.push(ServerConfig {
            id: next_id,
            uid: 0, // стабильный uid проставит AppConfig::save (ensure_uids)
            name: format!("server {next_id}"),
            active: true,
            show_window: true,
            feed: crate::config::FeedFlags::default(),
            key: Secret::default(),
            group: default_group(),
            market: default_market(),
            color: default_color(),
        });
    }
}

/// Кнопка «Данные» — свёртка 8 галок приёма в попап. Подпись = «n/8» (сколько
/// включено). Все включены → обычная серая кнопка; есть выключенные → заливка
/// градиентом цвета сервера (сигнал «часть категорий не принимаем»).
fn feed_button(ui: &mut egui::Ui, s: &mut ServerConfig, w: f32) {
    let on = [
        s.feed.orders,
        s.feed.detects,
        s.feed.reports,
        s.feed.balance,
        s.feed.strategies,
        s.feed.log,
        s.feed.alerts,
        s.feed.arb,
    ];
    let on_count = on.iter().filter(|b| **b).count();
    let all_on = on_count == on.len();
    let tint = (!all_on).then(|| egui::Color32::from_rgb(s.color[0], s.color[1], s.color[2]));

    let label = format!("{on_count}/{}", on.len());
    let resp = theme::seg_btn_tinted(ui, &label, Some(w), H, tint).on_hover_text(t!("conn.tip.flags"));
    let popup_id = ui.make_persistent_id(("feed_popup", s.id));
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    let note = t!("conn.filter_note");
    egui::popup_below_widget(
        ui,
        popup_id,
        &resp,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            ui.set_min_width(200.0);
            ui.checkbox(&mut s.feed.orders, format!("{} ({})", t!("conn.tip.orders"), note));
            ui.checkbox(&mut s.feed.detects, format!("{} ({})", t!("conn.tip.detects"), note));
            ui.checkbox(&mut s.feed.reports, format!("{} ({})", t!("conn.tip.reports"), note));
            ui.checkbox(&mut s.feed.balance, format!("{} ({})", t!("conn.tip.balance"), note));
            ui.checkbox(&mut s.feed.strategies, format!("{} ({})", t!("conn.tip.strat"), note));
            ui.checkbox(&mut s.feed.log, format!("{} ({})", t!("conn.tip.log"), note));
            ui.checkbox(&mut s.feed.alerts, format!("{} ({})", t!("conn.tip.alerts"), note));
            ui.checkbox(&mut s.feed.arb, format!("{} ({})", t!("conn.tip.arb"), note));
        },
    );
}

fn groups_panel(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    icons: &mut IconSet,
    picking: &mut Option<String>,
    actions: &mut super::SettingsActions,
) {
    ui.label(egui::RichText::new(t!("conn.groups_heading")).strong());
    ui.add_space(6.0);

    let mut names: Vec<String> = cfg.servers.iter().map(|s| s.group.clone()).collect();
    names.sort();
    names.dedup();

    // Группа существует только пока на неё ссылается хоть один сервер. Иначе при
    // наборе имени по буквам («4»→«44»→«444») в конфиге копились бы группы-сироты
    // от промежуточных значений. Чистим их сразу, до автосоздания актуальных.
    cfg.groups.retain(|g| names.contains(&g.name));

    if names.is_empty() {
        ui.label(egui::RichText::new(t!("conn.no_groups")).weak());
        return;
    }

    let w_act = 20.0;
    let w_ico = 22.0;
    let w_eye = 22.0;
    let w_pick = 46.0;
    let gap = 4.0; // плотные отступы между колонками (5 колонок → 4 зазора)
    // фикс. колонки + зазоры + полоса прокрутки (~16) + слак → остаток имени.
    let w_name =
        (ui.available_width() - w_act - w_ico - w_eye - w_pick - gap * 4.0 - 20.0).max(30.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        head(ui, w_act, t!("conn.col.act"));
        head(ui, w_ico, t!("conn.gcol.ico"));
        head(ui, w_name, t!("conn.col.name"));
        head(ui, w_eye, "");
        head(ui, w_pick, t!("conn.pick"));
    });

    for name in &names {
        if !cfg.groups.iter().any(|g| &g.name == name) {
            cfg.groups.push(GroupConfig::new(name.clone()));
        }
        let g = cfg.groups.iter_mut().find(|g| &g.name == name).unwrap();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.add_sized([w_act, H], egui::Checkbox::without_text(&mut g.active));
            match icons.texture(ui.ctx(), g.icon) {
                Some(tex) => {
                    ui.add_sized(
                        [w_ico, H],
                        egui::Image::new(&tex).fit_to_exact_size(egui::vec2(20.0, 20.0)),
                    );
                }
                None => {
                    ui.add_sized([w_ico, H], egui::Label::new(""));
                }
            }
            ui.add_sized(
                [w_name, H],
                egui::Label::new(egui::RichText::new(&g.name).strong()).truncate(),
            );
            // Кнопка-«глаз»: показать окно группы (создать, если закрыто). Иконку
            // рисуем painter'ом (глифы/эмодзи в Geist Mono дают «тофу»-квадрат).
            if eye_button(ui, w_eye, H)
                .on_hover_text(t!("conn.show_group"))
                .clicked()
            {
                actions.show_group.push(name.clone());
            }
            let pick = t!("conn.pick").to_string();
            if theme::seg_btn_h(ui, &pick, false, Some(w_pick), false, H).clicked() {
                *picking = Some(name.clone());
            }
        });
    }

    if let Some(pick) = picking.clone() {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(t!("conn.icon_for", name = pick)).strong());
            if ui.button("×").clicked() {
                *picking = None;
            }
        });
        let count = icons.count;
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt("icon_picker")
            .max_height(220.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for id in 0..count {
                        if let Some(tex) = icons.texture(ui.ctx(), id) {
                            let btn = egui::ImageButton::new(
                                egui::Image::new(&tex).fit_to_exact_size(egui::vec2(22.0, 22.0)),
                            );
                            if ui.add(btn).on_hover_text(format!("#{id}")).clicked() {
                                chosen = Some(id);
                            }
                        }
                    }
                });
            });
        if let Some(id) = chosen {
            if let Some(g) = cfg.groups.iter_mut().find(|g| g.name == pick) {
                g.icon = id;
            }
            *picking = None;
        }
    }
}

fn head(ui: &mut egui::Ui, w: f32, text: impl Into<String>) {
    ui.add_sized([w, H], egui::Label::new(egui::RichText::new(text.into()).weak()));
}

/// Кнопка-«глаз» (показать окно группы) — иконка нарисована painter'ом: контур
/// глаза (две дуги) + зрачок. Так читается на любом шрифте (без «тофу»).
fn eye_button(ui: &mut egui::Ui, w: f32, h: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let round = egui::Rounding::same(4.0);
        let (fill, border) = if hovered {
            (theme::LIFT_HOVER, theme::ACCENT.gamma_multiply(0.6))
        } else {
            (theme::LIFT, theme::BORDER)
        };
        let p = ui.painter();
        p.rect_filled(rect, round, fill);
        p.rect_stroke(rect, round, egui::Stroke::new(1.0, border));
        let c = rect.center();
        let col = if hovered { theme::TEXT } else { theme::TEXT_2 };
        let stroke = egui::Stroke::new(1.3, col);
        // Контур глаза — две дуги (верх/низ) как ломаные; зрачок — кружок в центре.
        let rw = 6.0; // полуширина глаза
        let rh = 3.2; // полувысота дуги
        let arc = |up: bool| {
            let s = if up { -1.0 } else { 1.0 };
            (0..=8)
                .map(|i| {
                    let t = i as f32 / 8.0;
                    let x = c.x - rw + 2.0 * rw * t;
                    let y = c.y + s * rh * (std::f32::consts::PI * t).sin();
                    egui::pos2(x, y)
                })
                .collect::<Vec<_>>()
        };
        p.add(egui::Shape::line(arc(true), stroke));
        p.add(egui::Shape::line(arc(false), stroke));
        p.circle_filled(c, 1.6, col);
    }
    resp
}

/// Заголовок колонки с всплывающей подсказкой (для сокращённых имён галок).
fn head_tip(ui: &mut egui::Ui, w: f32, text: impl Into<String>, tip: impl Into<String>) {
    ui.add_sized([w, H], egui::Label::new(egui::RichText::new(text.into()).weak()))
        .on_hover_text(tip.into());
}

/// Компактная галка с подсказкой.
fn flag(ui: &mut egui::Ui, w: f32, value: &mut bool, tip: impl Into<String>) {
    ui.add_sized([w, H], egui::Checkbox::without_text(value))
        .on_hover_text(tip.into());
}

/// Кружок статуса подключения в конце строки сервера. Цвет = состояние, подсказка
/// поясняет (для Failed — текст ошибки). `active=false` или нет записи в сессии →
/// серый «не подключается». Статус берётся из ЖИВОЙ сессии (по сохранённому
/// конфигу), поэтому несохранённые правила галки «Акт» он ещё не отражает.
fn status_dot(ui: &mut egui::Ui, w: f32, active: bool, status: Option<&crate::feed::ConnStatus>) {
    use crate::feed::ConnStatus;
    use crate::shell::theme::{ACCENT, GREEN, MUTED, RED};

    let (color, tip) = match status {
        _ if !active => (MUTED, t!("conn.status.inactive").to_string()),
        Some(ConnStatus::Ready) => (GREEN, t!("conn.status.ready").to_string()),
        Some(ConnStatus::Connecting) => (ACCENT, t!("conn.status.connecting").to_string()),
        Some(ConnStatus::Stage(s)) => (ACCENT, t!("conn.status.stage", stage = s).to_string()),
        Some(ConnStatus::Failed(e)) => (RED, t!("conn.status.failed", err = e).to_string()),
        Some(ConnStatus::Disconnected) => (MUTED, t!("conn.status.disconnected").to_string()),
        None => (MUTED, t!("conn.status.none").to_string()),
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
    resp.on_hover_text(tip);
}
