//! Общий рендер панелей контейнера (chart+glass) — используется и окном группы
//! (host), и откреплённым чарт-окном. Тут только wgpu-проходы по панелям и
//! egui-оверлей шкал/перекрестия; владение surface/egui — у вызывающего.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::chart::container::{Container, Pane};
use crate::chart::view::Rect;
use crate::config::ChartTheme;
use crate::session::SessionManager;

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
        let edge = if p.chart.view.is_live(now_ms) {
            now_ms
        } else {
            p.chart.view.right_time_ms
        };
        p.chart.view.pixel_at(edge).hash(&mut h);
    }
    h.finish()
}

/// Смещение локального времени от UTC, сек (подписи часов на шкале). На не-Windows
/// — 0 (UTC). Вынесено сюда, чтобы и host, и чарт-окно считали одинаково.
#[cfg(windows)]
pub fn local_offset_sec() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let lsec = l.wHour as i64 * 3600 + l.wMinute as i64 * 60 + l.wSecond as i64;
    let usec = u.wHour as i64 * 3600 + u.wMinute as i64 * 60 + u.wSecond as i64;
    let mut d = lsec - usec;
    if d > 43_200 {
        d -= 86_400;
    } else if d < -43_200 {
        d += 86_400;
    }
    d
}
#[cfg(not(windows))]
pub fn local_offset_sec() -> i64 {
    0
}

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
                        r: crate::chart::srgb_to_linear(cb[0]),
                        g: crate::chart::srgb_to_linear(cb[1]),
                        b: crate::chart::srgb_to_linear(cb[2]),
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
        let pcur = if hovered == Some(*idx) { cursor } else { None };
        let pane = &mut container.panes[*idx];
        pane.chart.set_cursor(pcur);
        pane.chart.render(
            device, queue, encoder, target, *rect, resolution, ppp, now_ms, data, true, clear,
            theme,
        );
    }
    layout
}

/// Рисует egui-оверлей шкал/перекрестия по одному на видимую панель. Курсор —
/// только у панели под мышью. Гоняется каждым кадром (не кэшируется).
#[allow(clippy::too_many_arguments)]
pub fn render_overlay(
    container: &Container,
    layout: &[(usize, Rect)],
    overlay_ctx: &egui::Context,
    overlay_renderer: &mut egui_wgpu::Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    gpu_size: (u32, u32),
    resolution: [f32; 2],
    ppp: f32,
    hovered: Option<usize>,
    cursor: Option<(f32, f32)>,
) {
    if layout.is_empty() {
        return;
    }
    let tz = local_offset_sec();
    let mut overlays: Vec<(crate::chart::axes::AxisSnapshot, egui::Rect, Option<egui::Pos2>)> =
        Vec::new();
    for (idx, rect) in layout {
        let Some(pane) = container.panes.get(*idx) else {
            continue;
        };
        let v = &pane.chart.view;
        let snap = crate::chart::axes::AxisSnapshot {
            px_per_ms: v.px_per_ms,
            right_margin_frac: v.right_margin_frac,
            render_center: v.render_center,
            render_range: v.render_range,
            epoch_ms: v.epoch_ms,
            right_time_ms: v.right_time_ms,
            tz_offset_sec: tz,
        };
        let prect = egui::Rect::from_min_size(
            egui::pos2(rect.x / ppp, rect.y / ppp),
            egui::vec2(rect.w / ppp, rect.h / ppp),
        );
        let pcur = if hovered == Some(*idx) {
            cursor.map(|(x, y)| egui::pos2(x / ppp, y / ppp))
        } else {
            None
        };
        overlays.push((snap, prect, pcur));
    }

    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(resolution[0] / ppp, resolution[1] / ppp),
        )),
        ..Default::default()
    };
    let vid = raw.viewport_id;
    raw.viewports.entry(vid).or_default().native_pixels_per_point = Some(ppp);

    let out = overlay_ctx.run(raw, |ctx| {
        let p = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("axes-overlay"),
        ));
        for (snap, prect, pcur) in &overlays {
            crate::chart::axes::draw(&p, *prect, ppp, snap, *pcur);
        }
    });
    let otris = overlay_ctx.tessellate(out.shapes, out.pixels_per_point);
    for (id, delta) in &out.textures_delta.set {
        overlay_renderer.update_texture(device, queue, *id, delta);
    }
    let oscreen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [gpu_size.0, gpu_size.1],
        pixels_per_point: ppp,
    };
    overlay_renderer.update_buffers(device, queue, encoder, &otris, &oscreen);
    {
        let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("axes-overlay-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let mut rpass = rpass.forget_lifetime();
        overlay_renderer.render(&mut rpass, &otris, &oscreen);
    }
    for id in &out.textures_delta.free {
        overlay_renderer.free_texture(id);
    }
}
