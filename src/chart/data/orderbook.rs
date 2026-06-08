//! Модель стакана для glass-слоя в стиле стенда: кумулятивная глубина
//! (полупрозрачный bar = накопленный объём от спреда наружу) + тонкая
//! линия индивидуального объёма на каждый уровень.

use crate::feed::OrderBook;

/// Инстанс прямоугольника стакана. Совпадает с glass.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LevelInstance {
    /// Цена центра полосы.
    pub price: f32,
    /// Высота полосы в единицах цены (зазор до соседнего уровня).
    pub span: f32,
    /// Длина полосы 0..1 (доля ширины зоны).
    pub len_norm: f32,
    /// 0 = bid fill, 1 = ask fill, 2 = bid line, 3 = ask line.
    pub kind: f32,
}

#[derive(Default)]
pub struct OrderBookModel {
    pub instances: Vec<LevelInstance>,
}

impl OrderBookModel {
    pub fn update(&mut self, book: &OrderBook) {
        self.instances.clear();

        // Копии, отсортированные от лучшей цены наружу.
        let mut bids = book.bids.clone();
        bids.sort_by(|a, b| b.price.total_cmp(&a.price)); // убывание
        let mut asks = book.asks.clone();
        asks.sort_by(|a, b| a.price.total_cmp(&b.price)); // возрастание

        // Нормировки по кумулятиву и индивидуальному объёму (обе стороны).
        let max_qty = bids
            .iter()
            .chain(asks.iter())
            .map(|l| l.qty)
            .fold(0.0_f32, f32::max)
            .max(1e-6);
        let bid_cum: f32 = bids.iter().map(|l| l.qty).sum();
        let ask_cum: f32 = asks.iter().map(|l| l.qty).sum();
        let max_cum = bid_cum.max(ask_cum).max(1e-6);

        // Сначала все fill (полупрозрачные), потом все line (поверх).
        push_side(&mut self.instances, &bids, max_cum, max_qty, false, false);
        push_side(&mut self.instances, &asks, max_cum, max_qty, true, false);
        push_side(&mut self.instances, &bids, max_cum, max_qty, false, true);
        push_side(&mut self.instances, &asks, max_cum, max_qty, true, true);
    }

    pub fn instances(&self) -> &[LevelInstance] {
        &self.instances
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }
}

fn push_side(
    out: &mut Vec<LevelInstance>,
    levels: &[crate::feed::Level],
    max_cum: f32,
    max_qty: f32,
    is_ask: bool,
    is_line: bool,
) {
    let kind = match (is_ask, is_line) {
        (false, false) => 0.0,
        (true, false) => 1.0,
        (false, true) => 2.0,
        (true, true) => 3.0,
    };
    let n = levels.len();
    let mut cum = 0.0_f32;
    for i in 0..n {
        let l = levels[i];
        cum += l.qty;
        // Зазор до соседнего уровня (для высоты непрерывной полосы).
        let span = if n > 1 {
            let j = if i > 0 { i - 1 } else { 1 };
            (levels[i].price - levels[j].price).abs()
        } else {
            l.price * 0.0005
        }
        .max(1e-6);

        let len_norm = if is_line {
            (l.qty / max_qty).clamp(0.0, 1.0) * 0.85
        } else {
            (cum / max_cum).clamp(0.0, 1.0)
        };

        out.push(LevelInstance {
            price: l.price,
            span,
            len_norm,
            kind,
        });
    }
}
