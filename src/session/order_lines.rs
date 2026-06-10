//! Ретейн-стор линий ордеров для чарта (per-core). moonproto держит терминальные
//! ордера лишь до deferred-cleanup, поэтому отменённые/исполненные мы храним САМИ —
//! всю сессию (с safety-cap по памяти), рисуя их полупрозрачными.
//!
//! Каждая линия — «лестница» ступеней `(t_ms, price)`: с момента `t_ms` цена держится
//! `price` до следующей ступени. Перестановка цены добавляет ступень (узелок). Начало
//! линии = время создания ордера, конец = время закрытия (или живой правый край).
//! Это уникальный источник старта/узлов/конца для маркеров и отрезков (рисует чарт).

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::feed::OrderRow;

/// Виды трассируемых линий (у каждой свой старт/узлы/конец). Ликвидация — отдельно
/// (непрерывная линия без маркеров), хранится как `RetainedOrder::liq`.
pub const TRACED_KINDS: usize = 7;

/// Индексы видов в `RetainedOrder::lines` (совпадают с порядком стилей).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Buy = 0,
    Sell = 1,
    Stop = 2,
    Trailing = 3,
    TakeProfit = 4,
    VStop = 5,
    PendingCond = 6,
}

/// Safety-cap хранения ордеров на ядро (бережём память при «всю сессию»).
const STORE_CAP: usize = 4000;

/// Грейс перед пометкой ордера закрытым после исчезновения из снимка, мс. Снимок
/// ордеров может кратко прийти пустым/частичным (реконнект, churn подписки) — без
/// грейса линии мигали бы active↔closed. Закрываем, только если ордер не виделся
/// дольше этого срока.
const CLOSE_GRACE_MS: f64 = 2500.0;

/// Текущее unix-время, мс (та же шкала, что time_ms тиков).
fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Относительный порог «цена изменилась» (защита от float-дрожания → ложных узлов).
fn price_eps(p: f32) -> f32 {
    p.abs() * 1e-5 + 1e-9
}

/// Одна линия ордера как лестница ступеней.
#[derive(Clone, Default)]
pub struct LineTrace {
    /// Ступени `(t_ms, price)`: с `t_ms` цена = `price` до следующей ступени.
    pub steps: Vec<(f64, f32)>,
    /// Линия выключена (цена стала недоступна), но ордер ещё жив. Конец линии.
    pub off_ms: Option<f64>,
}

impl LineTrace {
    /// Обновляет лестницу новым значением цены. `start_ms` — время первой ступени
    /// (для линии входа = создание ордера; для стопов = момент фила). Возвращает
    /// true при изменении (новая ступень / выключение) — для бампа ревизии стора.
    fn update(&mut self, price: Option<f32>, start_ms: f64, now_ms: f64) -> bool {
        match price {
            Some(p) if p.is_finite() && p > 0.0 => {
                let was_off = self.off_ms.take().is_some();
                match self.steps.last().copied() {
                    None => {
                        let t0 = if start_ms > 1.0 { start_ms } else { now_ms };
                        self.steps.push((t0, p));
                        true
                    }
                    Some((_, last_p)) => {
                        if (last_p - p).abs() > price_eps(p) {
                            self.steps.push((now_ms, p));
                            true
                        } else {
                            was_off
                        }
                    }
                }
            }
            _ => {
                if !self.steps.is_empty() && self.off_ms.is_none() {
                    self.off_ms = Some(now_ms);
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// Один удержанный ордер с трассами линий.
pub struct RetainedOrder {
    /// uid ордера — ключ стора; понадобится для hit-test/drag линий (этап 5).
    #[allow(dead_code)]
    pub uid: u64,
    pub market: String,
    pub is_short: bool,
    pub pending: bool,
    /// Время создания (начало линий), unix мс.
    pub create_ms: f64,
    /// Время закрытия (отмена/исполнение); None = ордер активен.
    pub closed_ms: Option<f64>,
    /// Когда ордер в последний раз был в снимке (для грейса закрытия).
    last_seen_ms: f64,
    /// Порядок появления (для cap-обрезки старых закрытых).
    pub seq: u64,
    /// Трассы по видам (индекс = LineKind as usize).
    pub lines: [LineTrace; TRACED_KINDS],
    /// Текущая цена ликвидации (непрерывная линия без маркеров).
    pub liq: Option<f32>,
}

impl RetainedOrder {
    fn new(r: &OrderRow, now_ms: f64, seq: u64) -> Self {
        // Старт не может быть в будущем (часы ядра могут опережать локальные) —
        // иначе сегмент линии вырождается/уходит за правый край.
        let create_ms = if r.create_time_ms > 1.0 {
            r.create_time_ms.min(now_ms)
        } else {
            now_ms
        };
        Self {
            uid: r.uid,
            market: r.market.clone(),
            is_short: r.is_short,
            pending: r.pending,
            create_ms,
            closed_ms: None,
            last_seen_ms: now_ms,
            seq,
            lines: Default::default(),
            liq: None,
        }
    }
}

/// Стор линий ордеров одного ядра (все рынки).
#[derive(Default)]
pub struct OrderLineStore {
    orders: HashMap<u64, RetainedOrder>,
    /// Растёт при реальном изменении геометрии (новый ордер/узел/закрытие/liq).
    pub rev: u64,
    seq_counter: u64,
}

impl OrderLineStore {
    /// Применяет свежий снимок ордеров: обновляет активные, фиксирует узлы при
    /// перестановках, помечает исчезнувшие закрытыми. Бампит rev при изменениях.
    pub fn update(&mut self, rows: &[OrderRow]) {
        let now_ms = now_unix_ms();
        let mut changed = false;
        let mut seen: HashSet<u64> = HashSet::with_capacity(rows.len());

        for r in rows {
            seen.insert(r.uid);
            let seq = self.seq_counter;
            let order = self.orders.entry(r.uid).or_insert_with(|| {
                changed = true;
                RetainedOrder::new(r, now_ms, seq)
            });
            if order.seq == seq {
                self.seq_counter += 1;
            }
            order.last_seen_ms = now_ms;
            // Воскрешение закрытого uid (редко) → снова активен.
            if order.closed_ms.take().is_some() {
                changed = true;
            }
            order.is_short = r.is_short;
            order.pending = r.pending;
            let f = r.filled;
            // Вход (для long и short) — всегда BUY pending-ордер: видна сразу, старт =
            // создание. SELL (закрытие, в противоположную сторону) появляется только
            // после исполнения входа, старт = момент фила. Стопы/TP/vstop/liq — тоже
            // только после фила.
            let new_liq = if f { r.liq.map(|v| v as f32) } else { None };
            if order.liq != new_liq {
                order.liq = new_liq;
                changed = true;
            }
            let g = |show: bool, v: f64| (show && v.is_finite() && v > 0.0).then_some(v as f32);
            let go = |show: bool, v: Option<f64>| if show { v.map(|x| x as f32) } else { None };
            // (значение, время первой ступени) по видам.
            let vals: [(Option<f32>, f64); TRACED_KINDS] = [
                (g(true, r.buy_price), order.create_ms), // вход (buy) — всегда
                (g(f, r.sell_price), now_ms),            // закрытие (sell) — после фила
                (go(f, r.stop_loss), now_ms),
                (go(f, r.trailing), now_ms),
                (go(f, r.take_profit), now_ms),
                (go(f, r.vstop), now_ms),
                // Pending-условие осмысленно только до фила (старт = создание).
                (go(!f, r.pending_cond), order.create_ms),
            ];
            for (i, (v, start_ms)) in vals.into_iter().enumerate() {
                changed |= order.lines[i].update(v, start_ms, now_ms);
            }
        }

        // Исчезли из снимка дольше грейса → закрыты (грейс гасит мигание на
        // кратком пустом/частичном снимке при реконнекте/churn подписки).
        for (uid, ord) in self.orders.iter_mut() {
            if !seen.contains(uid)
                && ord.closed_ms.is_none()
                && now_ms - ord.last_seen_ms > CLOSE_GRACE_MS
            {
                ord.closed_ms = Some(ord.last_seen_ms);
                changed = true;
            }
        }

        if self.orders.len() > STORE_CAP {
            self.prune();
            changed = true;
        }
        if changed {
            self.rev = self.rev.wrapping_add(1);
        }
    }

    /// Срезает старейшие ЗАКРЫТЫЕ ордера сверх safety-cap.
    fn prune(&mut self) {
        let mut closed: Vec<(u64, u64)> = self
            .orders
            .iter()
            .filter(|(_, o)| o.closed_ms.is_some())
            .map(|(uid, o)| (o.seq, *uid))
            .collect();
        let over = self.orders.len().saturating_sub(STORE_CAP);
        if over == 0 || closed.is_empty() {
            return;
        }
        closed.sort_unstable();
        for (_, uid) in closed.into_iter().take(over) {
            self.orders.remove(&uid);
        }
    }

    /// Ордера данного рынка (для рендера линий конкретной панели).
    pub fn iter_market<'a>(
        &'a self,
        market: &'a str,
    ) -> impl Iterator<Item = &'a RetainedOrder> + 'a {
        self.orders.values().filter(move |o| o.market == market)
    }

    /// Диапазон цен (min,max) текущих линий BUY и SELL открытых (не закрытых)
    /// ордеров рынка — для авто-масштаба Y. ТОЛЬКО buy/sell (не стопы/liq/прочее).
    pub fn buy_sell_range(&self, market: &str) -> Option<(f32, f32)> {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut any = false;
        for o in self.iter_market(market) {
            if o.closed_ms.is_some() {
                continue;
            }
            for idx in [LineKind::Buy as usize, LineKind::Sell as usize] {
                if let Some(&(_, p)) = o.lines[idx].steps.last() {
                    if p.is_finite() && p > 0.0 {
                        lo = lo.min(p);
                        hi = hi.max(p);
                        any = true;
                    }
                }
            }
        }
        any.then_some((lo, hi))
    }
}
