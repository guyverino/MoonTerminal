//! Общий рендер панелей контейнера (chart+glass) — используется и окном группы
//! (host), и откреплённым чарт-окном. Тут только wgpu-проходы по панелям и
//! egui-оверлей шкал/перекрестия; владение surface/egui — у вызывающего.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::container::{Container, Pane};
use crate::view::Rect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::session::SessionManager;

/// Кап частоты кадров: не презентим чаще этого. Общий для окна группы и
/// откреплённого чарт-окна. 16_666 мкс ≈ 60 fps.
pub const MIN_FRAME_DT: Duration = Duration::from_micros(16_666);

/// Текущее unix-время в мс (та же шкала, что приходит в render как now_ms).
pub fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Сигнатура видимых панелей: рыночные ревизии + край времени каждой панели.
/// Меняется → нужен кадр. Общая для host (активный контейнер) и чарт-окна.
pub fn panes_visible_sig(panes: &[Pane], session: &SessionManager, now_ms: f64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in panes {
        if let Some(v) = session.market_view(p.core, &p.market) {
            v.ticks_rev.hash(&mut h);
            v.book_rev.hash(&mut h);
        }
        // Ревизия линий ордеров ядра — новый ордер/узел/закрытие → нужен кадр.
        if let Some(c) = session.store().core(p.core) {
            c.order_lines.rev.hash(&mut h);
        }
        let edge = if p.chart.view.is_live(now_ms) {
            now_ms
        } else {
            p.chart.view.right_time_ms
        };
        p.chart.view.pixel_at(edge).hash(&mut h);
    }
    h.finish()
}

// local_offset_sec (подписи времени) уехал в egui-оболочку вместе с render_overlay
// (использует крейт `windows`; movnet-chart egui-/windows-free).

/// Рисует панели контейнера в `target` (тайл/фулскрин по `container.mode`).
/// Первая панель чистит кадр (фон темы), остальные — поверх. Пустой контейнер —
/// серый closed_bg. Возвращает раскладку (физ. px) для hit-теста ввода.
#[allow(clippy::too_many_arguments)]
pub fn render_panes(
    container: &mut Container,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    area: Rect,
    resolution: [f32; 2],
    ppp: f32,
    now_ms: f64,
    theme: &ChartTheme,
    orders_style: &OrdersStyle,
    hovered: Option<usize>,
    cursor: Option<(f32, f32)>,
    session: &SessionManager,
) -> Vec<(usize, Rect)> {
    let layout = container.layout(area);
    if layout.is_empty() {
        let cb = theme.closed_bg;
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chart-empty"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: crate::srgb_to_linear(cb[0]),
                        g: crate::srgb_to_linear(cb[1]),
                        b: crate::srgb_to_linear(cb[2]),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        return layout;
    }
    for (n, (idx, rect)) in layout.iter().enumerate() {
        let clear = n == 0;
        let (core, market) = {
            let p = &container.panes[*idx];
            (p.core, p.market.clone())
        };
        let data = session.market_view(core, &market);
        // Ретейн-стор линий ордеров ядра панели (история + закрытые; чарт фильтрует
        // по market и культит по окну времени).
        let lines = session.store().core(core).map(|c| &c.order_lines);
        let pcur = if hovered == Some(*idx) { cursor } else { None };
        let pane = &mut container.panes[*idx];
        pane.chart.set_cursor(pcur);
        pane.chart.render(
            device, queue, encoder, target, *rect, resolution, ppp, now_ms, data, lines, &market,
            orders_style, true, clear, theme,
        );
    }
    layout
}
