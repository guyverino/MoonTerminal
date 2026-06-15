//! Семантический ring тиков: SoA-подобное хранилище инстансов для GPU.
//! Append-only по времени (late-тики тоже просто добавляются).
//!
//! Настоящее кольцо (`VecDeque`): переполнение срезает голову `pop_front` за O(1)
//! без memmove. Combo-слой адресует тики АБСОЛЮТНЫМ индексом (`total`), поэтому
//! сдвиг головы НЕ инвалидирует уже залитый хвост (см. `ChartEngine::prepare`):
//! догоняется только `[last_total, total)`, полного re-bake на drop больше нет.

use std::collections::VecDeque;

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
    /// Абсолютный объём сделки.
    pub qty: f32,
}

pub struct TickRing {
    epoch_ms: f64,
    cap: usize,
    buf: VecDeque<TickInstance>,
    /// Всего инстансов добавлено за всю жизнь кольца (монотонно). Задаёт абсолютное
    /// индексное пространство: первый ещё живой индекс = `total - buf.len()`.
    total: u64,
}

impl TickRing {
    pub fn new(epoch_ms: f64, cap: usize) -> Self {
        Self {
            epoch_ms,
            cap,
            buf: VecDeque::with_capacity(cap.min(1 << 20)),
            total: 0,
        }
    }

    /// Абсолютный индекс за концом (== сколько всего добавлено). Combo догоняет
    /// хвост `[last_total, total)`.
    pub fn total_pushed(&self) -> u64 {
        self.total
    }

    /// Абсолютный индекс старейшего ещё живого инстанса (== сколько срезано с головы).
    /// Combo, отставший дальше этого, потерял часть хвоста → нужен полный reset.
    pub fn dropped(&self) -> u64 {
        self.total - self.buf.len() as u64
    }

    pub fn push_many(&mut self, ticks: &[Tick]) {
        for t in ticks {
            self.buf.push_back(TickInstance {
                time_rel_ms: (t.time_ms - self.epoch_ms) as f32,
                price: t.price,
                side: match t.side {
                    Side::Buy => 0.0,
                    Side::Sell => 1.0,
                },
                qty: t.qty.max(0.0),
            });
            self.total += 1;
        }
        // Настоящее кольцо: срезаем голову pop_front (O(1), без сдвига массива).
        while self.buf.len() > self.cap {
            self.buf.pop_front();
        }
    }

    /// Новый хвост от абсолютного индекса `abs_from` до конца (живой край для combo
    /// append). Если `abs_from` старше головы — отдаёт всё доступное (вызывающий
    /// гейтит полный reset раньше, чтобы такого не случалось в норме).
    pub fn iter_since(&self, abs_from: u64) -> impl Iterator<Item = &TickInstance> {
        let dropped = self.dropped();
        let start = abs_from.saturating_sub(dropped).min(self.buf.len() as u64) as usize;
        self.buf.range(start..)
    }

    /// Все инстансы (полный reset кольца combo: reload истории / съезд за глубину).
    pub fn iter_all(&self) -> impl Iterator<Item = &TickInstance> {
        self.buf.iter()
    }

    /// Диапазон видимых инстансов [start, start+count) по относительному времени.
    /// Инстансы по времени возрастающие (append-only) → бинарный поиск.
    pub fn visible_range(&self, left_rel: f32, right_rel: f32) -> (u32, u32) {
        if self.buf.is_empty() {
            return (0, 0);
        }
        let start = self.partition_point(|i| i.time_rel_ms < left_rel);
        let end = self.partition_point(|i| i.time_rel_ms <= right_rel);
        (start as u32, end.saturating_sub(start) as u32)
    }

    /// `partition_point` поверх `VecDeque` (индексация O(1)); std не даёт его на деке.
    fn partition_point<F: Fn(&TickInstance) -> bool>(&self, pred: F) -> usize {
        let mut lo = 0usize;
        let mut hi = self.buf.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(&self.buf[mid]) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Мин/макс цены среди среза [start, start+count) — авто-диапазон Y по
    /// ВИДИМОМУ окну (на паузе окно заморожено → вертикаль не дёргается).
    pub fn price_range_in(&self, start: u32, count: u32) -> Option<(f32, f32)> {
        if count == 0 {
            return None;
        }
        let s = start as usize;
        let e = (s + count as usize).min(self.buf.len());
        if s >= e {
            return None;
        }
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in self.buf.range(s..e) {
            lo = lo.min(i.price);
            hi = hi.max(i.price);
        }
        Some((lo, hi))
    }
}
