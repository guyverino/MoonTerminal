//! Ретейн-стор линий ордеров для чарта (per-core). moonproto держит терминальные
//! ордера лишь до deferred-cleanup, поэтому отменённые/исполненные мы храним САМИ —
//! всю сессию (с safety-cap по памяти), рисуя их полупрозрачными.
//!
//! Каждая линия — «лестница» ступеней `(t_ms, price)`: с момента `t_ms` цена держится
//! `price` до следующей ступени. Перестановка цены добавляет ступень (узелок). Начало
//! линии = время создания ордера, конец = время закрытия (или живой правый край).
//! Это уникальный источник старта/узлов/конца для маркеров и отрезков (рисует чарт).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::feed::{OrderRow, OrderTrace};

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

/// Кап кольца ЗАКРЫТЫХ ордеров на ядро (= верх слайдера `max_closed_orders`): свежие
/// толкаем в хвост, старейшие выпадают из головы сами — без сорта и прун-скана.
/// Открытые НЕ капаются (живут пока активны). Единственный кап на закрытые.
const CLOSED_RING_CAP: usize = 5000;

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
    /// Точная серверная polyline-трасса для buy/sell. Когда она есть, рендерит
    /// именно её; `steps` остаётся fallback для старых/неполных снимков.
    pub server_points: Vec<(f64, f32)>,
    /// Живая temp-точка серверной трассы: рисуется пунктиром от последней точки.
    pub tmp_point: Option<(f64, f32)>,
    /// Линия выключена (цена стала недоступна), но ордер ещё жив. Конец линии.
    pub off_ms: Option<f64>,
}

impl LineTrace {
    /// Обновляет лестницу новым значением цены. `start_ms` — время первой ступени
    /// (для линии входа = создание ордера; для стопов = момент фила). Возвращает
    /// true при изменении (новая ступень / выключение) — для бампа ревизии стора.
    fn update(&mut self, price: Option<f32>, start_ms: f64, now_ms: f64) -> bool {
        let had_server = !self.server_points.is_empty() || self.tmp_point.is_some();
        if had_server {
            self.server_points.clear();
            self.tmp_point = None;
        }
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
                            was_off || had_server
                        }
                    }
                }
            }
            _ => {
                if !self.steps.is_empty() && self.off_ms.is_none() {
                    self.off_ms = Some(now_ms);
                    true
                } else {
                    had_server
                }
            }
        }
    }

    fn update_server(&mut self, trace: Option<&OrderTrace>) -> bool {
        let Some(trace) = trace else {
            return false;
        };
        let points: Vec<(f64, f32)> = trace.points.iter().map(|p| (p.time_ms, p.price)).collect();
        let tmp = trace.tmp_point.map(|p| (p.time_ms, p.price));
        let changed =
            self.server_points != points || self.tmp_point != tmp || self.off_ms.is_some();
        if changed {
            self.server_points = points;
            self.tmp_point = tmp;
            self.off_ms = None;
        }
        changed
    }

    pub fn current_price(&self) -> Option<f32> {
        self.server_points
            .last()
            .map(|(_, p)| *p)
            .or_else(|| self.steps.last().map(|(_, p)| *p))
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
    pub panic_sell: bool,
    pub is_moon_shot: bool,
    pub corridor_price_down: f32,
    pub corridor_price_up: f32,
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
            panic_sell: r.panic_sell,
            is_moon_shot: r.is_moon_shot,
            corridor_price_down: r.corridor_price_down,
            corridor_price_up: r.corridor_price_up,
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
    /// Кольцо uid ЗАКРЫТЫХ в порядке закрытия — единственный кап на закрытые,
    /// без сорта/прун-скана: пришёл новый закрытый → в хвост, переполнено → из головы.
    closed_ring: VecDeque<u64>,
    /// Кэш диапазона цен buy/sell открытых ордеров по рынку (для авто-Y). Пересобирается
    /// ТОЛЬКО при изменении ордеров (вместе с rev), а не каждый prepare — buy_sell_range
    /// раньше сканировал все ордера ядра 60 раз/сек на каждую панель.
    buy_sell_ranges: HashMap<String, (f32, f32)>,
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
            if order.panic_sell != r.panic_sell
                || order.is_moon_shot != r.is_moon_shot
                || order.corridor_price_down != r.corridor_price_down
                || order.corridor_price_up != r.corridor_price_up
            {
                order.panic_sell = r.panic_sell;
                order.is_moon_shot = r.is_moon_shot;
                order.corridor_price_down = r.corridor_price_down;
                order.corridor_price_up = r.corridor_price_up;
                changed = true;
            }
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
            changed |= order.lines[LineKind::Buy as usize].update_server(r.buy_trace.as_ref());
            if r.buy_trace.is_none() {
                changed |= order.lines[LineKind::Buy as usize].update(
                    g(true, r.buy_price),
                    order.create_ms,
                    now_ms,
                );
            }
            changed |= order.lines[LineKind::Sell as usize].update_server(r.sell_trace.as_ref());
            if r.sell_trace.is_none() {
                changed |=
                    order.lines[LineKind::Sell as usize].update(g(f, r.sell_price), now_ms, now_ms);
            }

            let vals: [(Option<f32>, f64, usize); TRACED_KINDS - 2] = [
                (go(f, r.stop_loss), now_ms, LineKind::Stop as usize),
                (go(f, r.trailing), now_ms, LineKind::Trailing as usize),
                (go(f, r.take_profit), now_ms, LineKind::TakeProfit as usize),
                (go(f, r.vstop), now_ms, LineKind::VStop as usize),
                // Pending-условие осмысленно только до фила (старт = создание).
                (
                    go(!f, r.pending_cond),
                    order.create_ms,
                    LineKind::PendingCond as usize,
                ),
            ];
            for (v, start_ms, i) in vals {
                changed |= order.lines[i].update(v, start_ms, now_ms);
            }
        }

        // Исчезли из снимка дольше грейса → закрыты (грейс гасит мигание на
        // кратком пустом/частичном снимке при реконнекте/churn подписки).
        let mut newly_closed = Vec::new();
        for (uid, ord) in self.orders.iter_mut() {
            if !seen.contains(uid)
                && ord.closed_ms.is_none()
                && now_ms - ord.last_seen_ms > CLOSE_GRACE_MS
            {
                ord.closed_ms = Some(ord.last_seen_ms);
                newly_closed.push(*uid);
                changed = true;
            }
        }

        for uid in newly_closed {
            self.remember_closed(uid);
            changed = true;
        }
        if changed {
            self.rev = self.rev.wrapping_add(1);
            self.rebuild_buy_sell_ranges();
        }
    }

    /// Пересобирает кэш buy/sell-диапазонов по рынкам из текущих открытых ордеров.
    /// Зовётся только при реальном изменении (`changed`) — цены линий мутируют лишь в
    /// `update`, поэтому кэш всегда свежий, но скан O(ордера) идёт 4 Гц, не 60.
    fn rebuild_buy_sell_ranges(&mut self) {
        self.buy_sell_ranges.clear();
        for o in self.orders.values() {
            if o.closed_ms.is_some() {
                continue;
            }
            for idx in [LineKind::Buy as usize, LineKind::Sell as usize] {
                if let Some(p) = o.lines[idx].current_price() {
                    if p.is_finite() && p > 0.0 {
                        let e = self.buy_sell_ranges.entry(o.market.clone()).or_insert((p, p));
                        e.0 = e.0.min(p);
                        e.1 = e.1.max(p);
                    }
                }
            }
        }
    }

    /// Запоминает закрытый uid и срезает старейшие закрытые сверх safety-cap.
    fn remember_closed(&mut self, uid: u64) {
        self.closed_ring.push_back(uid);
        while self.closed_ring.len() > CLOSED_RING_CAP {
            let Some(old_uid) = self.closed_ring.pop_front() else {
                break;
            };
            if self
                .orders
                .get(&old_uid)
                .is_some_and(|order| order.closed_ms.is_some())
            {
                self.orders.remove(&old_uid);
            }
        }
    }

    /// Ордера данного рынка (для рендера линий конкретной панели).
    pub fn iter_market<'a>(
        &'a self,
        market: &'a str,
    ) -> impl Iterator<Item = &'a RetainedOrder> + 'a {
        self.orders.values().filter(move |o| o.market == market)
    }

    /// Ордера рынка ДЛЯ ОТРИСОВКИ: все открытые + новейшие `max_closed` закрытых, в
    /// порядке кольца (новые-первые), БЕЗ сортировки. Кап на закрытые задаёт сам стор
    /// кольцом — отдельного сорта/отбора в рендере больше нет.
    pub fn market_draw_orders(&self, market: &str, max_closed: usize) -> Vec<&RetainedOrder> {
        let mut out: Vec<&RetainedOrder> = self
            .orders
            .values()
            .filter(|o| o.market == market && o.closed_ms.is_none())
            .collect();
        let mut taken = 0usize;
        let mut seen: HashSet<u64> = HashSet::new();
        for uid in self.closed_ring.iter().rev() {
            if taken >= max_closed {
                break;
            }
            // Воскресший→переоткрытый uid может лежать в кольце дважды — дедупим.
            if !seen.insert(*uid) {
                continue;
            }
            if let Some(o) = self.orders.get(uid) {
                if o.closed_ms.is_some() && o.market == market {
                    out.push(o);
                    taken += 1;
                }
            }
        }
        out
    }

    /// Диапазон цен (min,max) текущих линий BUY и SELL открытых (не закрытых)
    /// ордеров рынка — для авто-масштаба Y. ТОЛЬКО buy/sell (не стопы/liq/прочее).
    /// Готовый кэш (`rebuild_buy_sell_ranges` при изменении ордеров), не скан per-prepare.
    pub fn buy_sell_range(&self, market: &str) -> Option<(f32, f32)> {
        self.buy_sell_ranges.get(market).copied()
    }
}
