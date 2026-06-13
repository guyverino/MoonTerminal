//! Chart renderer: слои (grid/crosses/glass/cursor) + вид. Рыночные данные приходят
//! извне (MarketView) — один рендерер может обслуживать любую панель/ядро/рынок.

pub mod axes;
pub mod canvas;
pub mod container;
// `data` (TickRing/OrderBookModel) переехал в moon-core (его тянет market).
// Ре-экспортим под прежним путём `crate::chart::data`, чтобы рендер не править.
pub use moon_core::data;
pub mod input;
pub mod layers;
pub mod paint;
pub mod style;
pub mod transform;
pub mod view;

use canvas::ChartCanvas;
use layers::{
    CrossesLayer, CursorLayer, GlassLayer, GridLayer, LineInstance, MarkerInstance, OrderLinesLayer,
    SegInstance,
};
use style::{StyleGlobals, StyleUniform};
use transform::ChartGlobals;
use view::{ChartView, Rect};

use crate::config::{LineStyle, OrdersStyle};
use crate::config::ChartTheme;
use crate::market::MarketView;
use crate::session::order_lines::{LineKind, OrderLineStore, RetainedOrder};

/// sRGB → linear (для clear-цвета; свопчейн sRGB сам кодирует обратно).
pub fn srgb_to_linear(c: u8) -> f64 {
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
    order_lines: OrderLinesLayer,

    cursor_pos: Option<(f32, f32)>,
    // какую ревизию данных уже залили в GPU-буферы этой панели
    last_ticks_rev: u64,
    last_book_rev: u64,
    // Скретч-буферы геометрии линий ордеров (переиспользуются). Пересобираются
    // каждый рисуемый кадр (объём мал; нужен живой правый край у активных линий).
    hlines_scratch: Vec<LineInstance>,
    segs_scratch: Vec<SegInstance>,
    markers_scratch: Vec<MarkerInstance>,
    // Скретч-буфер инстансов стакана: нормировка зависит от видимого окна ЭТОЙ
    // панели, поэтому строим локально (книга-модель шарится между панелями).
    glass_scratch: Vec<crate::chart::data::LevelInstance>,
    last_glass_lo: f32,
    last_glass_hi: f32,
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
        let order_lines = OrderLinesLayer::new(device, format, &globals.layout);

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
            order_lines,
            cursor_pos: None,
            last_ticks_rev: u64::MAX,
            last_book_rev: u64::MAX,
            hlines_scratch: Vec::new(),
            segs_scratch: Vec::new(),
            markers_scratch: Vec::new(),
            glass_scratch: Vec::new(),
            last_glass_lo: f32::NAN,
            last_glass_hi: f32::NAN,
        }
    }

    pub fn set_cursor(&mut self, pos: Option<(f32, f32)>) {
        self.cursor_pos = pos;
    }

    /// Кадр графика в `target`. `data` — рыночные данные (крестики/стакан, или None).
    /// `lines` — ретейн-стор линий ордеров ядра ЭТОЙ панели (фильтр по `market`);
    /// `style` — стиль линий (orders.toml). `open=false` — чарт закрыт: только серый
    /// clear (пустой контейнер), слои не рисуем.
    #[allow(clippy::too_many_arguments)]
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
        lines: Option<&OrderLineStore>,
        market: &str,
        style: &OrdersStyle,
        open: bool,
        clear: bool,
        theme: &ChartTheme,
    ) {
        // Чарт закрыт — заливаем кадр нейтральным серым (пустой контейнер). egui
        // поверх дорисует панели; центральная зона остаётся серой. `clear=false`
        // (не первая панель тайла) — ничего не делаем, фон уже залит соседом.
        if !open {
            if clear {
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
            }
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
        // Авто-масштаб должен захватывать линии открытых ордеров — ТОЛЬКО buy/sell
        // (не стоп/liq/прочее). Расширяем видимый ценовой диапазон их ценами.
        let order_pr = lines.and_then(|s| s.buy_sell_range(market));
        let auto_price = match (visible_price, order_pr) {
            (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
            (Some(v), None) => Some(v),
            (None, Some(o)) => Some(o),
            (None, None) => None,
        };
        let last_price = data.and_then(|d| d.last_price);
        self.view.update_y(now_ms, content.h, auto_price, last_price);

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
            // Нормировка стакана зависит от видимого ценового окна → пере-
            // строить инстансы при смене книги ИЛИ при смене окна (зум/пан).
            // render_* кусочно-постоянны, так что окно меняется редко.
            let half = self.view.render_range * 0.5;
            let lo = self.view.render_center - half;
            let hi = self.view.render_center + half;
            let eps = (hi - lo).abs() * 1e-3;
            let window_changed = !((lo - self.last_glass_lo).abs() <= eps
                && (hi - self.last_glass_hi).abs() <= eps);
            if d.book_rev != self.last_book_rev || window_changed {
                d.book.build_instances(lo, hi, &mut self.glass_scratch);
                self.glass.upload(device, queue, &self.glass_scratch);
                self.last_book_rev = d.book_rev;
                self.last_glass_lo = lo;
                self.last_glass_hi = hi;
            }
        }
        // Линии ордеров (слой 5): отрезки лестницы + кресты начала/конца + узелки.
        // Геометрия логическая (time_rel/price) → пан/зум/Y-scale делает шейдер;
        // пересобираем каждый кадр (объём мал, нужен живой правый край активных).
        // Куллинг по видимому окну времени держит объём малым на длинной сессии.
        if let Some(store) = lines {
            let left_rel = view_time0;
            let right_rel = view_time0 + window_ms;
            // Правый край plot-области (чарт + стакан) во времени: активные линии
            // тянутся сюда (через стакан), без конца. window_ms = cw/px_per_ms.
            let edge_rel =
                view_time0 + (chart_area.w + glass_w) / self.view.px_per_ms.max(1e-6);
            build_order_geometry(
                store,
                market,
                style,
                self.view.epoch_ms,
                now_ms,
                left_rel,
                right_rel,
                edge_rel,
                &mut self.hlines_scratch,
                &mut self.segs_scratch,
                &mut self.markers_scratch,
            );
        } else {
            self.hlines_scratch.clear();
            self.segs_scratch.clear();
            self.markers_scratch.clear();
        }
        self.order_lines.upload(
            device,
            queue,
            &self.hlines_scratch,
            &self.segs_scratch,
            &self.markers_scratch,
        );

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
        // Запас канваса справа = 20% ширины (см. canvas::MARGIN_FRAC): маленький,
        // т.к. ручной скролл и так инвалидирует канвас (отзыв ядра-разработчика).
        self.canvas
            .ensure(device, cw.ceil() as u32 + canvas::margin_px(cw), ch.ceil() as u32);

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
                    // в linear, иначе фон осветляется в серый. `clear=false` (не
                    // первая панель тайла) → Load, чтобы не стереть соседей.
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear(theme.bg[0]),
                            g: srgb_to_linear(theme.bg[1]),
                            b: srgb_to_linear(theme.bg[2]),
                            a: 1.0,
                        })
                    } else {
                        wgpu::LoadOp::Load
                    },
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
        // Линия ликвидации — во всю ширину графика, но ДО стакана (не заходит в него).
        self.order_lines.render_liq(&mut rpass, cbg);

        scissor(&mut rpass, glass_area, resolution);
        self.glass
            .render(&mut rpass, &self.glass_globals.bind_group, &self.style.bind_group);

        // Линии ордеров (без конца) + маркеры — ПОВЕРХ стакана, по всей plot-области
        // (чарт + стакан): тянутся вправо через стакан. Тот же uniform (chart_area):
        // x = время от левого края, поэтому продолжаются в зону стакана.
        scissor(&mut rpass, plot_area, resolution);
        self.order_lines.render_overlay(&mut rpass, cbg);
        // Курсор — поверх всего, по plot-области (чарт + стакан, без жёлобов шкал).
        self.cursor.render(&mut rpass, &self.style.bind_group);
    }
}

/// sRGB-цвет [u8;3] + alpha → [f32;4] (шейдер переводит rgb в linear).
fn rgba(c: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        alpha,
    ]
}

/// Виды трассируемых линий: (стиль, индекс в RetainedOrder::lines).
fn traced_kinds(s: &OrdersStyle) -> [(&LineStyle, usize); 7] {
    [
        (&s.buy, LineKind::Buy as usize),
        (&s.sell, LineKind::Sell as usize),
        (&s.stop, LineKind::Stop as usize),
        (&s.trailing, LineKind::Trailing as usize),
        (&s.take_profit, LineKind::TakeProfit as usize),
        (&s.vstop, LineKind::VStop as usize),
        (&s.pending_cond, LineKind::PendingCond as usize),
    ]
}

/// Собирает геометрию линий ордеров рынка `market`: отрезки лестницы (горизонтали
/// на ступенях + вертикальные стыки), кресты начала/конца, узелки перестановок и
/// непрерывную линию ликвидации. Куллит ордера вне видимого окна по времени.
#[allow(clippy::too_many_arguments)]
fn build_order_geometry(
    store: &OrderLineStore,
    market: &str,
    style: &OrdersStyle,
    epoch_ms: f64,
    now_ms: f64,
    left_rel: f32,
    right_rel: f32,
    edge_rel: f32,
    hlines: &mut Vec<LineInstance>,
    segs: &mut Vec<SegInstance>,
    markers: &mut Vec<MarkerInstance>,
) {
    hlines.clear();
    segs.clear();
    markers.clear();
    let to_rel = |t_ms: f64| (t_ms - epoch_ms) as f32;
    let kinds = traced_kinds(style);

    // Отбор видимых: по cap закрытых (новые-первые) и окну времени. Активные всегда
    // тянутся к правому краю → видимы; закрытые культим по [create, closed].
    let mut visible: Vec<&RetainedOrder> = store.iter_market(market).collect();
    // Новые-первые для cap по закрытым.
    visible.sort_unstable_by(|a, b| b.seq.cmp(&a.seq));
    let mut closed_drawn = 0u32;
    for ord in visible {
        let closed = ord.closed_ms.is_some();
        if closed {
            if closed_drawn >= style.max_closed_orders {
                continue;
            }
            closed_drawn += 1;
        }
        let order_end = ord.closed_ms.unwrap_or(now_ms);
        // Куллинг по окну времени (rel ms).
        let start_rel = to_rel(ord.create_ms);
        let end_rel = to_rel(order_end);
        if end_rel < left_rel || start_rel > right_rel {
            continue;
        }
        let alpha = if closed {
            style.closed_alpha
        } else {
            style.active_alpha
        };

        // Ликвидация — непрерывная горизонталь без маркеров.
        if let Some(p) = ord.liq {
            let s = &style.liq;
            hlines.push(LineInstance {
                price: p,
                color: rgba(s.color, alpha),
                style: if s.dashed { 1.0 } else { 0.0 },
                thickness: s.thickness,
            });
        }

        let path = &style.path;
        let path_col = rgba(path.color, alpha);
        let path_dash = if path.dashed { 1.0 } else { 0.0 };

        for (st, idx) in kinds {
            let line = &ord.lines[idx];
            let n = line.steps.len();
            if n == 0 {
                continue;
            }
            // Линия завершена, если выключена сама или закрыт ордер. У активной
            // (незавершённой) линии КОНЦА НЕТ — она тянется до правого края plot
            // (через стакан), без креста конца. У завершённой конец = off/close время.
            let ended = line.off_ms.is_some() || closed;
            let line_end = line.off_ms.unwrap_or(order_end);
            let dashed = st.dashed
                || (idx == LineKind::Buy as usize && ord.pending && style.pending_dashed);
            let col = rgba(st.color, alpha);
            let dash = if dashed { 1.0 } else { 0.0 };

            let start_t = line.steps[0].0;
            // Текущая цена — последняя ступень. Основная линия ПРЯМАЯ на текущей цене
            // от начала до конца (вся переезжает при перестановке).
            let cur_p = line.steps[n - 1].1;
            let t0_rel = to_rel(start_t);
            // Активная линия — до правого края (edge_rel, через стакан); завершённая —
            // до своего времени конца.
            let t1_rel = if ended { to_rel(line_end) } else { edge_rel };

            // Опциональный «путь» (trail): змейка реальных позиций по истории —
            // рисуем ПОД основной линией, своим стилем.
            if path.show && n > 1 {
                for i in 0..n {
                    let (t, p) = line.steps[i];
                    let seg_end_t = if i + 1 < n {
                        line.steps[i + 1].0
                    } else {
                        line_end
                    };
                    if seg_end_t > t {
                        segs.push(SegInstance {
                            t0_rel: to_rel(t),
                            p0: p,
                            t1_rel: to_rel(seg_end_t),
                            p1: p,
                            thickness: path.thickness,
                            dashed: path_dash,
                            color: path_col,
                        });
                    }
                    if i + 1 < n {
                        let p2 = line.steps[i + 1].1;
                        segs.push(SegInstance {
                            t0_rel: to_rel(seg_end_t),
                            p0: p,
                            t1_rel: to_rel(seg_end_t),
                            p1: p2,
                            thickness: path.thickness,
                            dashed: path_dash,
                            color: path_col,
                        });
                    }
                }
            }

            // Основная прямая линия на текущей цене.
            segs.push(SegInstance {
                t0_rel,
                p0: cur_p,
                t1_rel,
                p1: cur_p,
                thickness: st.thickness,
                dashed: dash,
                color: col,
            });

            // Узелки — точки на прямой линии в моменты перестановок (steps[1..]).
            if st.knots {
                for i in 1..n {
                    markers.push(MarkerInstance {
                        t_rel: to_rel(line.steps[i].0),
                        price: cur_p,
                        size: st.knot_size,
                        thickness: st.marker_thickness,
                        shape: 1.0,
                        color: col,
                    });
                }
            }

            // Крест начала и конца — на концах прямой линии (на текущей цене).
            if st.start_marker {
                markers.push(MarkerInstance {
                    t_rel: t0_rel,
                    price: cur_p,
                    size: st.marker_size,
                    thickness: st.marker_thickness,
                    shape: 0.0,
                    color: col,
                });
            }
            if st.end_marker && ended {
                markers.push(MarkerInstance {
                    t_rel: t1_rel,
                    price: cur_p,
                    size: st.marker_size,
                    thickness: st.marker_thickness,
                    shape: 0.0,
                    color: col,
                });
            }
        }
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
