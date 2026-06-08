//! Семантический ring тиков: SoA-подобное хранилище инстансов для GPU.
//! Append-only по времени (late-тики тоже просто добавляются).

use crate::feed::{Side, Tick};

/// Инстанс крестика для GPU. Layout должен совпадать с crosses.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TickInstance {
    /// Время относительно epoch, мс (f32).
    pub time_rel_ms: f32,
    pub price: f32,
    /// 0.0 = buy, 1.0 = sell.
    pub side: f32,
}

pub struct TickRing {
    epoch_ms: f64,
    cap: usize,
    instances: Vec<TickInstance>,
    /// Сколько инстансов срезано с начала за всю жизнь (ring). Растёт при cap —
    /// индексы инстансов «съезжают», поэтому canvas-append по индексу должен
    /// сбрасываться в re-bake (см. ChartCanvas::need_rebake).
    dropped: u64,
}

impl TickRing {
    pub fn new(epoch_ms: f64, cap: usize) -> Self {
        Self {
            epoch_ms,
            cap,
            instances: Vec::with_capacity(cap.min(1 << 20)),
            dropped: 0,
        }
    }

    /// Сколько инстансов срезано с начала за всю жизнь ring.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn push_many(&mut self, ticks: &[Tick]) {
        for t in ticks {
            self.instances.push(TickInstance {
                time_rel_ms: (t.time_ms - self.epoch_ms) as f32,
                price: t.price,
                side: match t.side {
                    Side::Buy => 0.0,
                    Side::Sell => 1.0,
                },
            });
        }
        // примитивный ring: срезаем старое начало.
        if self.instances.len() > self.cap {
            let drop = self.instances.len() - self.cap;
            self.instances.drain(0..drop);
            self.dropped += drop as u64;
        }
    }

    pub fn instances(&self) -> &[TickInstance] {
        &self.instances
    }

    /// Диапазон видимых инстансов [start, start+count) по относительному времени.
    /// Инстансы по времени возрастающие (append-only) → бинарный поиск.
    pub fn visible_range(&self, left_rel: f32, right_rel: f32) -> (u32, u32) {
        let s = &self.instances;
        if s.is_empty() {
            return (0, 0);
        }
        let start = s.partition_point(|i| i.time_rel_ms < left_rel);
        let end = s.partition_point(|i| i.time_rel_ms <= right_rel);
        (start as u32, end.saturating_sub(start) as u32)
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn last_price(&self) -> Option<f32> {
        self.instances.last().map(|i| i.price)
    }

    /// Мин/макс цены среди среза [start, start+count) — авто-диапазон Y по
    /// ВИДИМОМУ окну (на паузе окно заморожено → вертикаль не дёргается).
    pub fn price_range_in(&self, start: u32, count: u32) -> Option<(f32, f32)> {
        if count == 0 {
            return None;
        }
        let s = start as usize;
        let e = (s + count as usize).min(self.instances.len());
        if s >= e {
            return None;
        }
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in &self.instances[s..e] {
            lo = lo.min(i.price);
            hi = hi.max(i.price);
        }
        Some((lo, hi))
    }

    /// Мин/макс цены среди последних `n` тиков — для авто-диапазона Y.
    pub fn price_range_tail(&self, n: usize) -> Option<(f32, f32)> {
        if self.instances.is_empty() {
            return None;
        }
        let start = self.instances.len().saturating_sub(n);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in &self.instances[start..] {
            lo = lo.min(i.price);
            hi = hi.max(i.price);
        }
        Some((lo, hi))
    }
}
