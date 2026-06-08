//! Вкладка «Подключения»: слева сервера, справа группы.
//! Сервер: Акт · Имя · Host · Port · Ключ · Группа · Цвет(детекта) · Удалить.
//! Группа: Акт(вся группа) · иконка · имя · выбор иконки.

use super::SettingsTab;
use crate::config::servers::{default_color, default_group, default_market};
use crate::config::{AppConfig, GroupConfig, Secret, ServerConfig};
use crate::icons::IconSet;
use crate::market::MarketDataMode;

#[derive(Default)]
pub struct ConnectionsTab {
    /// Для какой группы открыт picker иконок.
    picking: Option<String>,
    /// Строковые буферы порта (по id сервера) — нужны для маскировки порта
    /// password-полем с раскрытием по фокусу.
    port_buf: std::collections::HashMap<u64, String>,
}

const H: f32 = 22.0;

impl SettingsTab for ConnectionsTab {
    fn title(&self) -> String {
        t!("tab.connections").to_string()
    }

    fn ui(&mut self, ui: &mut egui::Ui, cfg: &mut AppConfig, icons: &mut IconSet) {
        let picking = &mut self.picking;
        let port_buf = &mut self.port_buf;
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
                groups_panel(ui, cfg, icons, picking);
            });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            servers_panel(ui, cfg, port_buf);
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
    port_buf: &mut std::collections::HashMap<u64, String>,
) {
    ui.label(egui::RichText::new(t!("conn.servers_heading")).strong());
    ui.add_space(6.0);

    let w_act = 20.0;
    let w_win = 20.0;
    let w_flag = 18.0; // одна галка фильтра приёма
    let n_flags = 8.0;
    let w_flags = w_flag * n_flags;
    let w_port = 44.0;
    // Color-кнопка рисуется в свою НАТУРАЛЬНУЮ ширину (≈interact_size), а
    // allocate_ui её не ограничивает — поэтому берём реальную ширину из стиля,
    // иначе строка окажется шире бюджета и «Удал» уедет под правую панель.
    let w_color = ui.spacing().interact_size.x;
    let w_del = 44.0;
    let sp = ui.spacing().item_spacing.x;
    // Фиксированные (нерастяжимые) ширины + межвиджетные отступы.
    // В строке 17 виджетов → 16 промежутков. Остаток делим между 4 текстовыми
    // полями. Без горизонтального скролла: всё всегда влезает в доступную ширину.
    let fixed = w_act + w_win + w_flags + w_port + w_color + w_del;
    let gaps = sp * 16.0;
    let flex = (ui.available_width() - fixed - gaps - 4.0).max(120.0);
    let w_name = flex * 0.20;
    let w_host = flex * 0.26;
    let w_key = flex * 0.34;
    let w_group = flex * 0.20;

    egui::ScrollArea::vertical()
        .id_salt("servers_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                head_tip(ui, w_act, t!("conn.col.act"), t!("conn.tip.act"));
                head_tip(ui, w_win, t!("conn.col.win"), t!("conn.tip.win"));
                head(ui, w_name, t!("conn.col.name"));
                head(ui, w_host, "Host");
                head(ui, w_port, "Port");
                head(ui, w_key, t!("conn.col.key"));
                head(ui, w_group, t!("conn.col.group"));
                head_tip(ui, w_flag, t!("conn.col.orders"), t!("conn.tip.orders"));
                head_tip(ui, w_flag, t!("conn.col.detects"), t!("conn.tip.detects"));
                head_tip(ui, w_flag, t!("conn.col.reports"), t!("conn.tip.reports"));
                head_tip(ui, w_flag, t!("conn.col.balance"), t!("conn.tip.balance"));
                head_tip(ui, w_flag, t!("conn.col.strat"), t!("conn.tip.strat"));
                head_tip(ui, w_flag, t!("conn.col.log"), t!("conn.tip.log"));
                head_tip(ui, w_flag, t!("conn.col.alerts"), t!("conn.tip.alerts"));
                head_tip(ui, w_flag, t!("conn.col.arb"), t!("conn.tip.arb"));
                head(ui, w_color, t!("conn.col.color"));
                head(ui, w_del, "");
            });

            let mut remove = None;
            for i in 0..cfg.servers.len() {
                let s = &mut cfg.servers[i];
                ui.horizontal(|ui| {
                    flag(ui, w_act, &mut s.active, t!("conn.tip.act"));
                    flag(ui, w_win, &mut s.show_window, t!("conn.tip.win"));
                    ui.add_sized([w_name, H], egui::TextEdit::singleline(&mut s.name));
                    // Host/Port маскируем (для скриншотов), раскрываем по фокусу.
                    let host_id = egui::Id::new(("srv-host", s.id));
                    let host_focused = ui.memory(|m| m.has_focus(host_id));
                    ui.add_sized(
                        [w_host, H],
                        egui::TextEdit::singleline(&mut s.host)
                            .password(!host_focused)
                            .id(host_id),
                    );
                    let port_id = egui::Id::new(("srv-port", s.id));
                    let port_focused = ui.memory(|m| m.has_focus(port_id));
                    let buf = port_buf.entry(s.id).or_insert_with(|| s.port.to_string());
                    if !port_focused {
                        // Пока не редактируем — держим буфер в синхроне с конфигом.
                        *buf = s.port.to_string();
                    }
                    let port_resp = ui.add_sized(
                        [w_port, H],
                        egui::TextEdit::singleline(buf)
                            .password(!port_focused)
                            .id(port_id),
                    );
                    if port_resp.changed() {
                        let digits: String = buf.chars().filter(|c| c.is_ascii_digit()).collect();
                        s.port = digits.parse::<u32>().unwrap_or(0).min(65535) as u16;
                    }
                    ui.add_sized(
                        [w_key, H],
                        egui::TextEdit::singleline(s.key.buffer_mut())
                            .password(true)
                            .hint_text("key"),
                    );
                    ui.add_sized([w_group, H], egui::TextEdit::singleline(&mut s.group));
                    // Фильтры приёма. Подсказка честно поясняет: ядро всё равно
                    // шлёт — выкл лишь не читаем/не складываем/не рисуем.
                    let note = t!("conn.filter_note");
                    flag(ui, w_flag, &mut s.feed.orders, format!("{} ({})", t!("conn.tip.orders"), note));
                    flag(ui, w_flag, &mut s.feed.detects, format!("{} ({})", t!("conn.tip.detects"), note));
                    flag(ui, w_flag, &mut s.feed.reports, format!("{} ({})", t!("conn.tip.reports"), note));
                    flag(ui, w_flag, &mut s.feed.balance, format!("{} ({})", t!("conn.tip.balance"), note));
                    flag(ui, w_flag, &mut s.feed.strategies, format!("{} ({})", t!("conn.tip.strat"), note));
                    flag(ui, w_flag, &mut s.feed.log, format!("{} ({})", t!("conn.tip.log"), note));
                    flag(ui, w_flag, &mut s.feed.alerts, format!("{} ({})", t!("conn.tip.alerts"), note));
                    flag(ui, w_flag, &mut s.feed.arb, format!("{} ({})", t!("conn.tip.arb"), note));
                    ui.allocate_ui(egui::vec2(w_color, H), |ui| {
                        let mut col =
                            egui::Color32::from_rgb(s.color[0], s.color[1], s.color[2]);
                        if ui.color_edit_button_srgba(&mut col).changed() {
                            s.color = [col.r(), col.g(), col.b()];
                        }
                    });
                    if ui
                        .add_sized([w_del, H], egui::Button::new(t!("conn.delete").to_string()))
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                cfg.servers.remove(i);
            }
        });

    ui.add_space(8.0);
    if ui.button(t!("conn.add_core").to_string()).clicked() {
        let next_id = cfg.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        cfg.servers.push(ServerConfig {
            id: next_id,
            uid: 0, // стабильный uid проставит AppConfig::save (ensure_uids)
            name: format!("server {next_id}"),
            active: true,
            show_window: true,
            feed: crate::config::FeedFlags::default(),
            host: String::new(),
            port: 0,
            key: Secret::default(),
            group: default_group(),
            market: default_market(),
            color: default_color(),
        });
    }
}

fn groups_panel(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    icons: &mut IconSet,
    picking: &mut Option<String>,
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

    let w_act = 22.0;
    let w_ico = 24.0;
    let w_pick = 50.0;
    // запас под 3 межвиджетных отступа (~24) + полосу прокрутки (~16) + слак.
    let w_name = (ui.available_width() - w_act - w_ico - w_pick - 24.0).max(40.0);

    ui.horizontal(|ui| {
        head(ui, w_act, t!("conn.col.act"));
        head(ui, w_ico, t!("conn.gcol.ico"));
        head(ui, w_name, t!("conn.col.name"));
        head(ui, w_pick, t!("conn.pick"));
    });

    for name in &names {
        if !cfg.groups.iter().any(|g| &g.name == name) {
            cfg.groups.push(GroupConfig::new(name.clone()));
        }
        let g = cfg.groups.iter_mut().find(|g| &g.name == name).unwrap();
        ui.horizontal(|ui| {
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
            if ui
                .add_sized([w_pick, H], egui::Button::new(t!("conn.pick").to_string()))
                .clicked()
            {
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
