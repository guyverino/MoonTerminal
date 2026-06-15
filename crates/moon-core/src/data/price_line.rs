//! Retained price-line rings (LastPrice / MarkPrice) for chart rendering.

use crate::feed::PricePoint;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PriceLinePoint {
    pub time_rel_ms: f32,
    pub price: f32,
}

pub struct PriceLineRing {
    epoch_ms: f64,
    cap: usize,
    points: Vec<PriceLinePoint>,
    dropped: u64,
}

impl PriceLineRing {
    pub fn new(epoch_ms: f64, cap: usize) -> Self {
        Self {
            epoch_ms,
            cap,
            points: Vec::with_capacity(cap.min(1 << 18)),
            dropped: 0,
        }
    }

    pub fn push_many(&mut self, points: &[PricePoint]) {
        for p in points {
            if p.price.is_finite() && p.price > 0.0 {
                self.points.push(PriceLinePoint {
                    time_rel_ms: (p.time_ms - self.epoch_ms) as f32,
                    price: p.price,
                });
            }
        }
        if self.points.len() > self.cap {
            let drop = self.points.len() - self.cap;
            self.points.drain(0..drop);
            self.dropped += drop as u64;
        }
    }

    pub fn points(&self) -> &[PriceLinePoint] {
        &self.points
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}
