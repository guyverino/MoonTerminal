//! Маппинг `moon_chart::view::ChartView` (математика вида, общая с эталоном) → наш cbuffer
//! `ChartViewGpu` + извлечение Trades из ring в GPU-инстансы (конверт `TickInstance`12б →
//! `ChartCross`16б). Никакого рисования — только подготовка данных для own-pass слоёв.

use moon_chart::view::{ChartView, Rect};
use moon_core::data::{TickInstance, TickRing};

use super::gpu::{ChartCross, ChartViewGpu};

/// Собирает GPU-юнформ для текущего вида и чарт-области (физ. px). Поля заполняются ПО ИМЕНАМ
/// (порядок в `ChartViewGpu` отличается от `moon_chart` ChartUniform — нельзя memcpy).
pub fn view_gpu(view: &ChartView, area: Rect, resolution: [f32; 2]) -> ChartViewGpu {
    let (view_time0, _window_ms) = view.visible_x(area.w);
    let view_price0 = view.render_center - (area.h * 0.5) / view.px_per_price.max(1e-6);
    ChartViewGpu {
        bounds: [area.x, area.y, area.w, area.h],
        resolution,
        time_to_px: view.px_per_ms,
        view_time0,
        price_to_px: view.px_per_price,
        view_price0,
        marker_half: view.marker_half_px,
        pad: 0.0,
    }
}

#[inline]
fn cross_of(t: &TickInstance) -> ChartCross {
    ChartCross {
        time_rel: t.time_rel_ms,
        price: t.price,
        side: if t.side >= 0.5 { 1 } else { 0 }, // 0 buy / 1 sell (TickInstance side: 0.0/1.0)
        pad: 0,
    }
}

/// Весь набор тиков (reset кольца combo — reload истории / съезд индексов после drain).
pub fn collect_all(ring: &TickRing) -> Vec<ChartCross> {
    ring.instances().iter().map(cross_of).collect()
}

/// Новые тики [from, to) для инкрементального append в кольцо combo (живой край).
pub fn collect_range(ring: &TickRing, from: usize, to: usize) -> Vec<ChartCross> {
    let s = ring.instances();
    let to = to.min(s.len());
    if from >= to {
        return Vec::new();
    }
    s[from..to].iter().map(cross_of).collect()
}
