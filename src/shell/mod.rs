//! egui-оболочка терминала: верхняя панель (рынок/статус/цена) и статус-бар.
//! Высоты панелей экспонируются, чтобы app мог посчитать область графика.

mod brand;
pub mod theme;
pub mod widgets;

use crate::feed::ConnStatus;
use crate::icons::IconSet;

pub const HEADER_H: f32 = 46.0;
pub const STATUS_H: f32 = 24.0;

/// Высота логотипа в шапке (точки egui). Под неё растрится текстура (brand).
const LOGO_H: f32 = 22.0;

/// Данные для отрисовки, которые app обновляет каждый кадр.
pub struct ShellInfo<'a> {
    pub group: &'a str,
    /// Сколько ядер подключено (Ready) из общего числа — для счётчика «N/M».
    pub conn_ready: usize,
    pub conn_total: usize,
    /// Не-Ready ядра (имя, статус) — для всплывающей подсказки «кто и почему».
    pub conn_down: &'a [(String, ConnStatus)],
    pub tick_count: usize,
    pub book_levels: usize,
    pub fps: f32,
    /// Реальные present() в секунду (скользящее окно) — диагностика нагрузки.
    pub present_hz: f32,
    /// CPU процесса, % всей машины.
    pub cpu_process: f32,
    /// CPU всей системы, %.
    pub cpu_system: f32,
    /// RAM процесса, МБ.
    pub mem_mb: f32,
    /// Прирост RAM за окно, МБ (>0 — растёт).
    pub mem_delta_mb: f32,
}

pub struct Shell {
    /// Логотип MoonBot (растрённый SVG) — None, если рендер не удался.
    logo: Option<egui::TextureHandle>,
}

impl Shell {
    pub fn new(ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        Self {
            logo: brand::load_logo(ctx, LOGO_H),
        }
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        info: &ShellInfo,
        open_settings: &mut bool,
        reports_clicked: &mut bool,
        strategies_clicked: &mut bool,
        _icons: &mut IconSet,
    ) {
        egui::TopBottomPanel::top("header")
            .exact_height(HEADER_H)
            .show(ctx, |ui| {
                // horizontal_centered — содержимое по центру высоты панели (без
                // большого пустого зазора снизу до разделителя с тулбаром).
                ui.horizontal_centered(|ui| {
                    ui.add_space(10.0);
                    // Логотип MoonBot (как на стенде) — высота LOGO_H, ширина по аспекту.
                    if let Some(logo) = &self.logo {
                        let [tw, th] = logo.size();
                        let w = LOGO_H * tw as f32 / th.max(1) as f32;
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(w, LOGO_H)));
                        ui.add_space(12.0);
                    }
                    ui.heading(egui::RichText::new(info.group).color(theme::ACCENT));
                    ui.add_space(14.0);
                    // Цена ВСЕГДА BTC (не текущий рынок). Источник-тикер пока не
                    // подключён → «—»; правило выбора биржи см. память проекта.
                    ui.label(egui::RichText::new("1 BTC =").strong().size(16.0));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("—").size(16.0).color(theme::MUTED));
                    ui.add_space(14.0);
                    // Пиллы TP / SL / Lev (порт со стенда; действия позже). Зазор
                    // между ними — явным add_space (кастомные кнопки идут впритык).
                    if theme::pill(ui, "TP", "+3.0%", theme::TP).clicked() {
                        log::info!("[ui] TP (todo)");
                    }
                    ui.add_space(theme::BTN_GAP);
                    if theme::pill(ui, "SL", "-2.0%", theme::RED).clicked() {
                        log::info!("[ui] SL (todo)");
                    }
                    ui.add_space(theme::BTN_GAP);
                    if theme::pill(ui, "Lev", "x1", theme::TEXT).clicked() {
                        log::info!("[ui] Lev (todo)");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0; // зазор ставим явно
                        ui.add_space(10.0);
                        // Кнопки шапки — тем же общим стилем seg_btn, что и size/масштаб.
                        // (right_to_left: добавляются справа налево.)
                        if theme::seg_btn(ui, &t!("shell.settings_btn"), false, None, false).clicked() {
                            *open_settings = true;
                        }
                        ui.add_space(theme::BTN_GAP);
                        if theme::seg_btn(ui, &t!("toolbar.reports"), false, None, false).clicked() {
                            *reports_clicked = true;
                        }
                        ui.add_space(theme::BTN_GAP);
                        if theme::seg_btn(ui, &t!("toolbar.help"), false, None, false).clicked() {
                            log::info!("[ui] Справка (todo)");
                        }
                        ui.add_space(theme::BTN_GAP);
                        if theme::seg_btn(ui, &t!("toolbar.strategies"), false, None, false).clicked() {
                            *strategies_clicked = true;
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("status")
            .exact_height(STATUS_H)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    // Индикатор соединения — слева внизу (как на стенде): счётчик
                    // «подключено N/M» + тултип со списком не-подключённых ядер.
                    status_badge(ui, info);
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "ticks {}  ·  book {}  ·  {:.0} fps  ·  present {:.0}/s  ·  \
                             CPU {:.0}% proc / {:.0}% sys  ·  RAM {:.0} MB ({:+.1})",
                            info.tick_count,
                            info.book_levels,
                            info.fps,
                            info.present_hz,
                            info.cpu_process,
                            info.cpu_system,
                            info.mem_mb,
                            info.mem_delta_mb,
                        ))
                        .color(theme::MUTED)
                        .small(),
                    );
                });
            });

        // Центральную область (и тулбар/панель ордера) рисует Dock.
    }
}

fn status_badge(ui: &mut egui::Ui, info: &ShellInfo) {
    let all_ok = info.conn_total > 0 && info.conn_ready == info.conn_total;
    let any_failed = info
        .conn_down
        .iter()
        .any(|(_, s)| matches!(s, ConnStatus::Failed(_) | ConnStatus::Disconnected));
    // Зелёный — все на связи; красный — есть упавшие; иначе янтарный (идёт подключение).
    let color = if all_ok {
        theme::GREEN
    } else if any_failed {
        theme::RED
    } else {
        theme::ACCENT
    };
    let text = format!(
        "● {}/{} {}",
        info.conn_ready,
        info.conn_total,
        t!("status.connected_count")
    );
    let resp = ui.label(egui::RichText::new(text).color(color));
    // Тултип — только про тех, кто НЕ подключён (с причиной).
    if !info.conn_down.is_empty() {
        resp.on_hover_ui(|ui| {
            for (name, st) in info.conn_down {
                let reason = match st {
                    ConnStatus::Connecting => t!("status.connecting").to_string(),
                    ConnStatus::Stage(s) => s.clone(),
                    ConnStatus::Failed(e) => e.clone(),
                    ConnStatus::Disconnected => t!("status.disconnected").to_string(),
                    ConnStatus::Ready => continue,
                };
                ui.label(egui::RichText::new(format!("{name}: {reason}")).small());
            }
        });
    }
}
