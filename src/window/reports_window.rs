//! Отдельное нативное окно «Отчёты»: таблица закрытых ордеров из локальной
//! SQLite (ПОЛНОЕ зеркало `orders`), фильтры (ядро/даты/монета/сторона), выбор
//! отображаемых колонок (все колонки БД), сортировка по клику на заголовок
//! (сохраняется), топ-N по сортировке + ИТОГО за период внизу. Автообновление по
//! приходу/изменению отчёта (счётчик-генерация writer'а). Кроссплатформенно.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusqlite::types::Value;
use rusqlite::Connection;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::db::{self, ReportFilter, ReportTable, SideFilter};
use crate::gpu::GpuContext;
use crate::shell::theme;

/// Грузим только топ-N по сортировке (строк в БД может быть очень много).
const ROW_LIMIT: usize = 100;

/// Колонки, видимые по умолчанию (имена = колонки БД).
const DEFAULT_VISIBLE: &[&str] = &[
    "buydate", "closedate", "core_name", "coin", "isshort", "quantity", "buyprice",
    "sellprice", "profitbtc", "lev", "strategyid", "sellreason", "comment",
];

pub struct ReportsWindow {
    pub window: Arc<Window>,
    gpu: GpuContext,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    dirty: bool,

    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    cores: Vec<(u64, String)>,
    table: ReportTable,
    totals: (f64, i64),

    sort_key: String,
    sort_desc: bool,

    sel_core: usize,
    coin_buf: String,
    from_buf: String,
    to_buf: String,
    side: SideFilter,
    needs_query: bool,

    /// Видимость колонок (параллельно db::DISPLAY_COLUMNS).
    visible: Vec<bool>,
}

impl ReportsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        generation: Option<Arc<AtomicU64>>,
    ) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("Отчёты — MoonTerminal")
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 660.0));
        let window = Arc::new(event_loop.create_window(attrs)?);

        let gpu = GpuContext::new(window.clone())?;
        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false);

        let conn = db::open_reader();
        let cores = conn.as_ref().map(db::distinct_cores).unwrap_or_default();
        let last_gen = generation.as_ref().map(|g| g.load(Ordering::Relaxed)).unwrap_or(0);
        let visible = db::DISPLAY_COLUMNS
            .iter()
            .map(|c| DEFAULT_VISIBLE.contains(c))
            .collect();
        let (sort_key, sort_desc) = conn
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));

        Ok(Self {
            window,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            dirty: true,
            generation,
            last_gen,
            conn,
            cores,
            table: ReportTable { cols: db::DISPLAY_COLUMNS, rows: Vec::new() },
            totals: (0.0, 0),
            sort_key,
            sort_desc,
            sel_core: 0,
            coin_buf: String::new(),
            from_buf: String::new(),
            to_buf: String::new(),
            side: SideFilter::All,
            needs_query: true,
            visible,
        })
    }

    pub fn on_egui_event(&mut self, event: &WindowEvent) -> bool {
        self.dirty = true;
        self.egui_state.on_window_event(&self.window, event).consumed
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.gpu.resize(size);
        self.dirty = true;
    }

    pub fn needs_render(&self) -> bool {
        self.dirty
    }

    /// Сверяет счётчик writer'а: новые/изменённые записи → перезапрос.
    pub fn poll(&mut self) {
        if let Some(g) = &self.generation {
            let v = g.load(Ordering::Relaxed);
            if v != self.last_gen {
                self.last_gen = v;
                self.needs_query = true;
                self.dirty = true;
            }
        }
    }

    fn filter(&self) -> ReportFilter {
        ReportFilter {
            core_uid: if self.sel_core == 0 {
                None
            } else {
                self.cores.get(self.sel_core - 1).map(|(uid, _)| *uid)
            },
            date_from: db::parse_ymd(&self.from_buf),
            date_to: db::parse_ymd(&self.to_buf).map(|d| d + 86_399),
            coin: self.coin_buf.clone(),
            side: self.side,
        }
    }

    fn requery(&mut self) {
        if self.conn.is_none() {
            self.conn = db::open_reader();
        }
        let f = self.filter();
        if let Some(conn) = &self.conn {
            self.cores = db::distinct_cores(conn);
            self.table = db::query_reports(conn, &f, &self.sort_key, self.sort_desc, ROW_LIMIT);
            self.totals = db::query_totals(conn, &f);
        }
        self.needs_query = false;
    }

    pub fn render(&mut self) {
        if self.needs_query {
            self.requery();
        }

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.gpu.surface.configure(&self.gpu.device, &self.gpu.config);
                return;
            }
            Err(e) => {
                log::warn!("reports surface error: {e:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("reports-encoder") });

        let raw_input = self.egui_state.take_egui_input(&self.window);

        let mut changed = false;
        let mut sort_changed = false;
        let sel_core = &mut self.sel_core;
        let coin_buf = &mut self.coin_buf;
        let from_buf = &mut self.from_buf;
        let to_buf = &mut self.to_buf;
        let side = &mut self.side;
        let visible = &mut self.visible;
        let sort_key = &mut self.sort_key;
        let sort_desc = &mut self.sort_desc;
        let cores = &self.cores;
        let table = &self.table;
        let totals = self.totals;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::TopBottomPanel::top("reports-filters")
                .exact_height(44.0)
                .show(ctx, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Ядро:").weak());
                        let sel_text = if *sel_core == 0 {
                            "Все".to_string()
                        } else {
                            cores.get(*sel_core - 1).map(|(_, n)| n.clone()).unwrap_or_else(|| "Все".into())
                        };
                        egui::ComboBox::from_id_salt("core_cb").selected_text(sel_text).show_ui(ui, |ui| {
                            changed |= ui.selectable_value(sel_core, 0, "Все").changed();
                            for (i, (_u, name)) in cores.iter().enumerate() {
                                changed |= ui.selectable_value(sel_core, i + 1, name).changed();
                            }
                        });

                        ui.separator();
                        ui.label(egui::RichText::new("Монета:").weak());
                        changed |= ui.add(egui::TextEdit::singleline(coin_buf).desired_width(80.0).hint_text("все")).changed();

                        ui.separator();
                        ui.label(egui::RichText::new("Сторона:").weak());
                        let side_text = match *side {
                            SideFilter::All => "Все",
                            SideFilter::Long => "Лонг",
                            SideFilter::Short => "Шорт",
                        };
                        egui::ComboBox::from_id_salt("side_cb").selected_text(side_text).show_ui(ui, |ui| {
                            changed |= ui.selectable_value(side, SideFilter::All, "Все").changed();
                            changed |= ui.selectable_value(side, SideFilter::Long, "Лонг").changed();
                            changed |= ui.selectable_value(side, SideFilter::Short, "Шорт").changed();
                        });

                        ui.separator();
                        ui.label(egui::RichText::new("С:").weak());
                        changed |= ui.add(egui::TextEdit::singleline(from_buf).desired_width(92.0).hint_text("ГГГГ-ММ-ДД")).changed();
                        ui.label(egui::RichText::new("По:").weak());
                        changed |= ui.add(egui::TextEdit::singleline(to_buf).desired_width(92.0).hint_text("ГГГГ-ММ-ДД")).changed();

                        ui.separator();
                        ui.menu_button("Колонки ▾", |ui| {
                            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                for (i, col) in db::DISPLAY_COLUMNS.iter().enumerate() {
                                    ui.checkbox(&mut visible[i], header_for(col));
                                }
                            });
                        });
                    });
                });

            egui::TopBottomPanel::bottom("reports-totals")
                .exact_height(30.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let (sum, count) = totals;
                        ui.label(egui::RichText::new("Итого за период:").weak());
                        let col = if sum > 0.0 { theme::GREEN } else if sum < 0.0 { theme::RED } else { theme::MUTED };
                        ui.label(egui::RichText::new(format!("{sum:+.6} BTC")).color(col).strong());
                        ui.separator();
                        ui.label(egui::RichText::new(format!("ордеров: {count}")).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(format!("показано (топ): {}", table.rows.len())).weak());
                        });
                    });
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                reports_table(ui, table, visible, sort_key, sort_desc, &mut sort_changed);
            });
        });

        if changed || sort_changed {
            self.needs_query = true;
            self.dirty = true;
        }
        if sort_changed {
            if let Some(conn) = &self.conn {
                db::save_sort(conn, &self.sort_key, self.sort_desc);
            }
        }

        self.egui_state.handle_platform_output(&self.window, full_output.platform_output);
        let tris = self.egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.size.width, self.gpu.size.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        for (id, delta) in &full_output.textures_delta.set {
            self.egui_renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }
        self.egui_renderer.update_buffers(&self.gpu.device, &self.gpu.queue, &mut encoder, &tris, &screen);
        {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reports-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0745, g: 0.0784, b: 0.0863, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut rpass = rpass.forget_lifetime();
            self.egui_renderer.render(&mut rpass, &tris, &screen);
        }
        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        self.dirty = self.egui_ctx.has_requested_repaint();
    }
}

/// Generic-таблица по всем DISPLAY_COLUMNS. Клик по заголовку — сортировка.
fn reports_table(
    ui: &mut egui::Ui,
    table: &ReportTable,
    visible: &[bool],
    sort_key: &mut String,
    sort_desc: &mut bool,
    sort_changed: &mut bool,
) {
    let vis: Vec<usize> = (0..table.cols.len()).filter(|i| visible.get(*i).copied().unwrap_or(false)).collect();
    if vis.is_empty() {
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Все колонки скрыты — включите в «Колонки».").weak());
        return;
    }
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;

    egui::ScrollArea::horizontal().show(ui, |ui| {
        // Заголовки — кликабельные, со стрелкой направления у активной колонки.
        ui.horizontal(|ui| {
            for &i in &vis {
                let col = table.cols[i];
                let arrow = if *sort_key == col {
                    if *sort_desc { " ▼" } else { " ▲" }
                } else {
                    ""
                };
                let w = width_for(col);
                let btn = egui::Button::new(egui::RichText::new(format!("{}{arrow}", header_for(col))).strong()).frame(false);
                if ui.add_sized([w, row_h], btn).clicked() {
                    if *sort_key == col {
                        *sort_desc = !*sort_desc;
                    } else {
                        *sort_key = col.to_string();
                        *sort_desc = true;
                    }
                    *sort_changed = true;
                }
            }
        });
        ui.separator();

        if table.rows.is_empty() {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Нет отчётов под фильтр (или БД пуста).").weak());
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, table.rows.len(), |ui, range| {
            for r in &table.rows[range] {
                ui.horizontal(|ui| {
                    for &i in &vis {
                        let col = table.cols[i];
                        let val = r.get(i).unwrap_or(&Value::Null);
                        let (text, color) = cell(col, val);
                        let mut rich = egui::RichText::new(text.clone());
                        if let Some(c) = color {
                            rich = rich.color(c);
                        }
                        let resp = ui.add_sized([width_for(col), row_h], egui::Label::new(rich).truncate());
                        if matches!(col, "comment" | "sellreason" | "channelname" | "signaltype" | "fname")
                            && !text.is_empty()
                        {
                            resp.on_hover_text(text);
                        }
                    }
                });
            }
        });
    });
}

/// Текст + цвет ячейки по имени колонки и значению.
fn cell(col: &str, v: &Value) -> (String, Option<egui::Color32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => {
            (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None)
        }
        "isshort" => match as_i64(v) {
            Some(1) => ("Шорт".into(), Some(theme::RED)),
            Some(0) => ("Лонг".into(), Some(theme::GREEN)),
            _ => (String::new(), Some(theme::MUTED)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => ("эму".into(), Some(theme::MUTED)),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(theme::GREEN),
                Some(x) if x < 0.0 => Some(theme::RED),
                _ => None,
            };
            (n.map(|x| format!("{x:+.6}")).unwrap_or_default(), color)
        }
        _ => (value_to_string(v), None),
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(r) => Some(*r),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => {
            let s = format!("{r:.8}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            if s.is_empty() { "0".into() } else { s.to_string() }
        }
        Value::Text(t) => t.clone(),
        Value::Blob(_) => "<blob>".into(),
    }
}

/// Человекочитаемый заголовок колонки (для дельт — имя как есть).
fn header_for(col: &str) -> &str {
    match col {
        "buydate" => "Открыт (UTC)",
        "closedate" => "Закрыт (UTC)",
        "sellsetdate" => "Sell set",
        "last_update_at" => "Обновлён",
        "core_name" => "Ядро",
        "db_id" => "ID",
        "taskid" => "TaskID",
        "exorderid" => "ExOrderID",
        "coin" => "Монета",
        "isshort" => "Сторона",
        "quantity" => "Кол-во",
        "boughtq" => "Куплено",
        "buyprice" => "Покупка",
        "sellprice" => "Продажа",
        "spentbtc" => "Влож.BTC",
        "gainedbtc" => "Получ.BTC",
        "profitbtc" => "Профит BTC",
        "lev" => "Плечо",
        "strategyid" => "Strat",
        "channelname" => "Канал",
        "signaltype" => "Сигнал",
        "fname" => "Файл",
        "basecurrency" => "BaseCur",
        "emulator" => "Эму",
        "status" => "Статус",
        "sellreason" => "Причина",
        "comment" => "Коммент",
        other => other,
    }
}

fn width_for(col: &str) -> f32 {
    match col {
        "buydate" | "closedate" => 120.0,
        "sellsetdate" | "last_update_at" => 116.0,
        "comment" => 280.0,
        "sellreason" => 170.0,
        "channelname" | "signaltype" | "fname" | "exorderid" => 110.0,
        "core_name" | "coin" => 88.0,
        "profitbtc" | "gainedbtc" | "spentbtc" => 96.0,
        "lev" | "isshort" | "emulator" => 52.0,
        _ => 82.0,
    }
}
