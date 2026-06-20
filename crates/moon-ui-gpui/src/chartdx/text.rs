//! Text emitted by chart `gpu_canvas.prepare_text`: axis labels and cursor readout.
//! This keeps chart-zone text on the retained GPU path instead of repainting the
//! GPUI view tree on every mouse move.

use gpui::{Hsla, point, px};
use moon_chart::axes::{fmt_clock, nice_interval, price_decimals};

use super::*;

const FONT_SIZE: f32 = 11.5;
const LINE_H: f32 = FONT_SIZE + 4.0;

fn color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

fn cross_color(rgba: [f32; 4]) -> Hsla {
    let r = (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u32;
    let mut out: Hsla = gpui::rgb((r << 16) | (g << 8) | b).into();
    out.a = rgba[3].clamp(0.0, 1.0);
    out
}

fn local_offset_sec() -> i64 {
    crate::axes::local_offset_sec()
}

impl RenderState {
    fn draw_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        text: &str,
        x: f32,
        y: f32,
        ax: f32,
        ay: f32,
        color: Hsla,
    ) -> anyhow::Result<()> {
        if self.text_run_cursor >= self.text_runs.len() {
            self.text_runs.push(GpuCanvasTextRun::default());
        }
        let run = &mut self.text_runs[self.text_run_cursor];
        self.text_run_cursor += 1;
        run.draw_aligned(
            ctx,
            point(px(x), px(y)),
            text,
            gpui::font(crate::design::mono()),
            px(FONT_SIZE),
            px(LINE_H),
            color,
            ax,
            ay,
        )?;
        Ok(())
    }

    pub(super) fn prepare_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
    ) -> anyhow::Result<()> {
        self.text_run_cursor = 0;
        let sf = ctx.scale_factor().max(0.1);
        let palette = self.ui_palette;
        let ink = color(palette.text_soft);
        let readout = cross_color(self.cursor_color);
        let font_w_guess = 7.0;
        let tz_offset_sec = local_offset_sec();

        for idx in 0..self.panes.len() {
            let (active, pane_bounds, view, orderbook_view, epoch_ms) = {
                let pr = &self.panes[idx];
                (
                    pr.active,
                    pr.pane_bounds,
                    pr.view,
                    pr.orderbook_view,
                    pr.epoch_ms,
                )
            };
            if !active {
                continue;
            }
            let pane_left = pane_bounds[0] / sf;
            let pane_bottom = (pane_bounds[1] + pane_bounds[3]) / sf;
            let plot_left = view.bounds[0] / sf;
            let plot_top = view.bounds[1] / sf;
            let plot_w = view.bounds[2] / sf;
            let plot_h = view.bounds[3] / sf;
            let plot_bottom = plot_top + plot_h;
            let plot_right = plot_left + plot_w;
            let full_right = ((orderbook_view.bounds[0] + orderbook_view.bounds[2])
                .max(view.bounds[0] + view.bounds[2]))
                / sf;

            if plot_w < 60.0 || plot_h < 60.0 || view.price_to_px <= 0.0 {
                continue;
            }

            let price_to_px = view.price_to_px / sf;
            let price_range = plot_h / price_to_px.max(1e-6);
            let y_min = view.view_price0;
            let top_price = y_min + price_range;
            let interval = nice_interval(price_range.max(1e-9), 8.0);
            let dec = price_decimals(y_min + price_range * 0.5);
            let mut p = (y_min / interval).ceil() * interval;
            let mut guard = 0;
            while p <= top_price && guard < 256 {
                let y = plot_bottom - (p - y_min) * price_to_px;
                if y >= plot_top - 1.0 && y <= plot_bottom + 1.0 {
                    let label = format!("{p:.dec$}");
                    self.draw_text(ctx, &label, plot_left - 4.0, y, 1.0, 0.5, ink)?;
                }
                p += interval;
                guard += 1;
            }

            let time_to_px = (view.time_to_px / sf).max(1e-6);
            let window_ms = plot_w as f64 / time_to_px as f64;
            let left_unix = epoch_ms + view.view_time0 as f64;
            let div_sec = window_ms / 1000.0 / 6.0;
            let with_sec = div_sec < 60.0;
            for k in 0..=6 {
                let frac = k as f64 / 6.0;
                let x = plot_left + (frac as f32) * plot_w;
                let unix = left_unix + frac * window_ms;
                let label = fmt_clock(unix, tz_offset_sec, with_sec);
                self.draw_text(ctx, &label, x, pane_bottom - 2.0, 0.5, 1.0, ink)?;
            }

            let Some(cursor) = self.cursor.filter(|cursor| cursor.pane == idx) else {
                continue;
            };
            let cx_dev = self.slot_origin[0] + cursor.local[0];
            let cy_dev = self.slot_origin[1] + cursor.local[1];
            let cx_log = cx_dev / sf;
            let cy_log = cy_dev / sf;

            if cx_log >= plot_left && cx_log <= plot_right {
                let unix = left_unix + (cx_log - plot_left) as f64 / time_to_px as f64;
                let label = fmt_clock(unix, tz_offset_sec, true);
                self.draw_text(ctx, &label, cx_log, pane_bottom - 1.0, 0.5, 1.0, readout)?;
            }
            if cy_log >= plot_top && cy_log <= plot_bottom {
                let price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                let label = format!("{price:.dec$}");
                let text_w = label.chars().count() as f32 * font_w_guess;
                let x = (plot_left - 3.0).max(pane_left + text_w + 6.0);
                self.draw_text(ctx, &label, x, cy_log, 1.0, 0.5, readout)?;
            }

            let _ = full_right;
        }

        if self.text_run_cursor < self.text_runs.len() {
            for run in &mut self.text_runs[self.text_run_cursor..] {
                run.clear();
            }
        }
        Ok(())
    }
}
