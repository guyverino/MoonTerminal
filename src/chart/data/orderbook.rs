//! Модель стакана для glass-слоя в стиле стенда: кумулятивная глубина
//! (полупрозрачный bar = накопленный объём от спреда наружу) + тонкая
//! линия индивидуального объёма на каждый уровень.
//!
//! Нормировка длины баров — НЕ по всей книге, а по максимуму среди уровней,
//! попавших в видимое ценовое окно панели (`build_instances`). Иначе при мелком
//! зуме приспредовые уровни — крошечная доля полного кумулятива, и весь стакан
//! «вытягивается в струну». По видимому окну самый крупный видимый уровень = на
//! всю ширину, и транзиентная стенка чётко выстреливает на своём уровне.

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

/// Сырой уровень книги (от окна не зависит): геометрия + объёмы. `len_norm`
/// считается позже в `build_instances` под видимое окно конкретной панели.
#[derive(Clone, Copy)]
struct RawLevel {
    price: f32,
    span: f32,
    /// Индивидуальный объём уровня (для тонкой линии).
    qty: f32,
    /// Кумулятив от спреда до этого уровня (для полосы глубины).
    cum: f32,
    is_ask: bool,
}

#[derive(Default)]
pub struct OrderBookModel {
    /// Биды (по убыванию цены), затем аски (по возрастанию) — порядок задаёт
    /// порядок отрисовки: fill-полосы под line-линиями.
    raw: Vec<RawLevel>,
}

impl OrderBookModel {
    pub fn update(&mut self, book: &OrderBook) {
        self.raw.clear();

        // Копии, отсортированные от лучшей цены наружу.
        let mut bids = book.bids.clone();
        bids.sort_by(|a, b| b.price.total_cmp(&a.price)); // убывание
        let mut asks = book.asks.clone();
        asks.sort_by(|a, b| a.price.total_cmp(&b.price)); // возрастание

        push_side(&mut self.raw, &bids, false);
        push_side(&mut self.raw, &asks, true);
    }

    /// Строит GPU-инстансы, нормируя длину баров по максимуму среди уровней
    /// внутри видимого окна `[lo, hi]` (единицы цены). Внеоконные уровни тоже
    /// эмитятся (их отсечёт viewport/scissor), но в знаменатель не входят.
    pub fn build_instances(&self, lo: f32, hi: f32, out: &mut Vec<LevelInstance>) {
        out.clear();

        // Знаменатели по видимому окну — общие для bid/ask, чтобы стенки сторон
        // были визуально сравнимы.
        let mut max_qty = 1e-6_f32;
        let mut max_cum = 1e-6_f32;
        for r in &self.raw {
            if r.price >= lo && r.price <= hi {
                max_qty = max_qty.max(r.qty);
                max_cum = max_cum.max(r.cum);
            }
        }

        // Сначала все fill (полупрозрачные кумулятив-полосы), потом все line.
        for r in &self.raw {
            out.push(LevelInstance {
                price: r.price,
                span: r.span,
                len_norm: (r.cum / max_cum).clamp(0.0, 1.0),
                kind: if r.is_ask { 1.0 } else { 0.0 },
            });
        }
        for r in &self.raw {
            out.push(LevelInstance {
                price: r.price,
                span: r.span,
                len_norm: (r.qty / max_qty).clamp(0.0, 1.0) * 0.85,
                kind: if r.is_ask { 3.0 } else { 2.0 },
            });
        }
    }

    /// Число уровней книги (для отладочного счётчика).
    pub fn len(&self) -> usize {
        self.raw.len()
    }
}

fn push_side(out: &mut Vec<RawLevel>, levels: &[crate::feed::Level], is_ask: bool) {
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

        out.push(RawLevel {
            price: l.price,
            span,
            qty: l.qty,
            cum,
            is_ask,
        });
    }
}
