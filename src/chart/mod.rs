//! Chart renderer: слои (grid/crosses/glass/cursor) + вид. Рыночные данные приходят
//! извне (MarketView) — один рендерер может обслуживать любую панель/ядро/рынок.

pub mod axes;
pub mod canvas;
pub mod data;
pub mod layers;
pub mod style;
pub mod transform;
pub mod view;

use canvas::ChartCanvas;
use layers::{CrossesLayer, CursorLayer, GlassLayer, GridLayer};
use style::{StyleGlobals, StyleUniform};
use transform::ChartGlobals;
use view::{ChartView, Rect};

use crate::config::ChartTheme;
use crate::market::MarketView;

/// sRGB → linear (для clear-цвета; свопчейн sRGB сам кодирует обратно).
fn srgb_to_linear(c: u8) -> f64 {
    let s = c as f64 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

pub struct Chart {
    pub view: ChartView,

    globals: ChartGlobals,
    glass_globals: ChartGlobals,
    style: StyleGlobals,
    grid: GridLayer,
    crosses: CrossesLayer,
    glass: GlassLayer,
    cursor: CursorLayer,
    canvas: ChartCanvas,

    cursor_pos: Option<(f32, f32)>,
    // какую ревизию данных уже залили в GPU-буферы этой панели
    last_ticks_rev: u64,
    last_book_rev: u64,
}

/// Ширина зоны стакана справа (как BOOK_WIDTH_CSS стенда = 220), физ. пиксели.
pub const GLASS_ZONE_PX: f32 = 220.0;

impl Chart {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, epoch_ms: f64) -> Self {
        let globals = ChartGlobals::new(device);
        let glass_globals = ChartGlobals::new(device);
        let style = StyleGlobals::new(device);
        let grid = GridLayer::new(device, format, &globals.layout, &style.layout);
        let crosses = CrossesLayer::new(device, format, &globals.layout);
        let glass = GlassLayer::new(device, format, &globals.layout, &style.layout);
        let cursor = CursorLayer::new(device, format, &style.layout);
        let canvas = ChartCanvas::new(device, format);

        Self {
            view: ChartView::new(epoch_ms),
            globals,
            glass_globals,
            style,
            grid,
            crosses,
            glass,
            cursor,
            canvas,
            cursor_pos: None,
            last_ticks_rev: u64::MAX,
            last_book_rev: u64::MAX,
        }
    }

    pub fn set_cursor(&mut self, pos: Option<(f32, f32)>) {
        self.cursor_pos = pos;
    }

    /// Кадр графика в `target`. `data` — данные активного ядра (или None).
    /// `open=false` — чарт закрыт: только серый clear (пустой контейнер), слои не
    /// рисуем.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        content: Rect,
        resolution: [f32; 2],
        ppp: f32,
        now_ms: f64,
        data: Option<&MarketView>,
        open: bool,
        theme: &ChartTheme,
    ) {
        // Чарт закрыт — заливаем кадр нейтральным серым (пустой контейнер). egui
        // поверх дорисует панели; центральная зона остаётся серой.
        if !open {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chart-empty"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Цвет пустого контейнера — из темы (sRGB→linear).
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear(theme.closed_bg[0]),
                            g: srgb_to_linear(theme.closed_bg[1]),
                            b: srgb_to_linear(theme.closed_bg[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            return;
        }
        // Тема → style-uniform (group 1) для grid/cursor.
        self.style
            .update(queue, &StyleUniform::from_theme(theme));
        // Жёлоба шкал (физ. пиксели = логич. константы × ppp): слева — цена,
        // снизу — время. Подписи в них рисует egui (chart::axes); тут лишь сжимаем
        // зоны рисования, чтобы grid/тики/стакан/курсор не залезали под шкалы.
        let price_axis_w = axes::PRICE_AXIS_W * ppp;
        let time_axis_h = axes::TIME_AXIS_H * ppp;
        let plot_h = (content.h - time_axis_h).max(1.0);
        // Три зоны над нижней шкалой: жёлоб цены | график | стакан.
        let glass_w = GLASS_ZONE_PX.min(content.w * 0.5);
        let chart_area = Rect {
            x: content.x + price_axis_w,
            y: content.y,
            w: (content.w - price_axis_w - glass_w).max(1.0),
            h: plot_h,
        };
        let glass_area = Rect {
            x: content.x + (content.w - glass_w).max(1.0),
            y: content.y,
            w: glass_w,
            h: plot_h,
        };
        // Зона перекрестия: вся plot-область (график + стакан, общая шкала цены),
        // но БЕЗ жёлобов шкал — крест не лезет в подписи.
        let plot_area = Rect {
            x: content.x + price_axis_w,
            y: content.y,
            w: (content.w - price_axis_w).max(1.0),
            h: plot_h,
        };

        // 1. Вид. Сначала X-окно (не зависит от Y), затем видимый срез тиков,
        //    затем авто-масштаб Y по ЭТОМУ срезу (на паузе окно заморожено →
        //    вертикаль стоит). Шкала цены — единая для чарта и стакана.
        // Smooth wall-clock follow (CHART_RENDERING_TZ): правый край = «сейчас»,
        // гладкий скролл по времени. Кадр дешёвый за счёт canvas UV-scroll +
        // egui-mesh cache (Stage 2b/2c), а не за счёт пропуска кадров.
        // Первый кадр: подгоняем зум так, чтобы дефолтное окно было ~1 минута.
        self.view.ensure_default_window(chart_area.w);
        self.view.follow_edge(now_ms, now_ms);
        let (view_time0, window_ms) = self.view.visible_x(chart_area.w);

        // Видимый срез крестиков (culling) — рисуем только то, что в окне по X.
        let (cross_start, cross_count) = match data {
            Some(d) => {
                let margin = self.view.marker_half_px / self.view.px_per_ms.max(1e-6);
                let left = view_time0 - margin;
                let right = view_time0 + window_ms + margin;
                d.ring.visible_range(left, right)
            }
            None => (0, 0),
        };

        let visible_price = data.and_then(|d| d.ring.price_range_in(cross_start, cross_count));
        let last_price = data.and_then(|d| d.last_price);
        self.view.update_y(now_ms, content.h, visible_price, last_price);

        let uniform = self.view.uniform(chart_area, resolution);
        self.globals.update(queue, &uniform);
        self.glass_globals
            .update(queue, &self.view.uniform(glass_area, resolution));

        // 2. Залить данные в GPU только если ревизия изменилась.
        if let Some(d) = data {
            if d.ticks_rev != self.last_ticks_rev {
                self.crosses.upload(device, queue, d.ring.instances());
                self.last_ticks_rev = d.ticks_rev;
            }
            if d.book_rev != self.last_book_rev {
                self.glass.upload(device, queue, d.book.instances());
                self.last_book_rev = d.book_rev;
            }
        }
        // Перекрестие живёт над всей plot-областью (чарт + стакан): шкала цены
        // общая, поэтому горизонталь/вертикаль проходят и через стакан; в жёлоба
        // шкал (слева/снизу) крест не заходит.
        self.cursor.update(queue, self.cursor_pos, resolution, plot_area);

        // 2c-2: крестики живут в offscreen-канвасе шире экрана (запас MARGIN_PX
        // справа «в будущее»). Полный re-bake — только при смене Y/зума/размера
        // или когда экран ушёл за запас; иначе дорисовываем лишь НОВЫЕ тики
        // (append), а движение времени — целочисленным UV-сдвигом при композите.
        // Всё это ДО основного прохода, чтобы канвас был готов к блиту.
        let cw = chart_area.w;
        let ch = chart_area.h;
        self.canvas
            .ensure(device, cw.ceil() as u32 + canvas::MARGIN_PX, ch.ceil() as u32);

        let total = data.map(|d| d.ring.len() as u32).unwrap_or(0);
        let dropped = data.map(|d| d.ring.dropped()).unwrap_or(0);
        let ppm = self.view.px_per_ms;
        let center = self.view.render_center;
        let range = self.view.render_range;

        if self.canvas.need_rebake(view_time0, ppm, center, range, cw, dropped) {
            // bake_time0 = левый край экрана → экран у левого края канваса.
            let canvas_w = self.canvas.width() as f32;
            let bake_u = self.view.bake_uniform(view_time0, canvas_w, ch);
            let marker_margin = self.view.marker_half_px / ppm.max(1e-6);
            let bstart = match data {
                Some(d) => d.ring.visible_range(view_time0 - marker_margin, view_time0).0,
                None => 0,
            };
            self.canvas.rebake(
                queue, encoder, &self.crosses, &bake_u, bstart, total, view_time0, ppm, center,
                range, dropped,
            );
        } else if total > self.canvas.baked_count() {
            let start = self.canvas.baked_count();
            self.canvas.append(encoder, &self.crosses, start, total);
        }
        let scroll = self.canvas.scroll_px(view_time0, ppm);

        // 3. Композит.
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chart-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Фон стакана/незакрытых зон — из темы. Свопчейн sRGB → задаём
                    // в linear, иначе фон осветляется в серый.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: srgb_to_linear(theme.bg[0]),
                        g: srgb_to_linear(theme.bg[1]),
                        b: srgb_to_linear(theme.bg[2]),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        scissor(&mut rpass, chart_area, resolution);
        let cbg = &self.globals.bind_group;
        self.grid.render(&mut rpass, cbg, &self.style.bind_group);
        // Крестики — блитом из канваса поверх grid с целочисленным UV-сдвигом.
        self.canvas
            .composite(queue, &mut rpass, chart_area, resolution, scroll);

        scissor(&mut rpass, glass_area, resolution);
        self.glass
            .render(&mut rpass, &self.glass_globals.bind_group, &self.style.bind_group);

        // Курсор — поверх всего, по plot-области (чарт + стакан, без жёлобов шкал).
        scissor(&mut rpass, plot_area, resolution);
        self.cursor.render(&mut rpass, &self.style.bind_group);
    }
}

/// Ставит scissor-прямоугольник, клампя к фреймбуферу.
fn scissor(rpass: &mut wgpu::RenderPass, r: Rect, res: [f32; 2]) {
    let x = r.x.clamp(0.0, res[0]);
    let y = r.y.clamp(0.0, res[1]);
    let w = (r.w.min(res[0] - x)).max(0.0);
    let h = (r.h.min(res[1] - y)).max(0.0);
    if w >= 1.0 && h >= 1.0 {
        rpass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
    }
}
