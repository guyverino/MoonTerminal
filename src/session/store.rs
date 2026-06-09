//! Per-core АККАУНТНЫЕ данные: статус, ордера, детекты, стратегии. Свои у каждого
//! ядра. Рыночные данные (крестики/стакан) — общие для биржи и живут отдельно в
//! `crate::market::MarketStore` (дедуп по ядру-провайдеру), сюда не попадают.
//!
//! Версии (revision) заменяют dirty-флаги: каждая панель сама решает, когда
//! перезаливать данные (важно, когда одно ядро показано в нескольких панелях).

use std::collections::HashMap;

use crate::feed::{ConnStatus, DetectRow, FeedMsg, OrderRow, StrategyRow, StrategySchemaModel};

/// Сколько последних детектов держим в памяти на ядро.
const MAX_DETECTS: usize = 2000;

pub type CoreId = u64;

pub struct CoreData {
    pub status: ConnStatus,
    /// Открытые ордера ядра (все рынки).
    pub orders: Vec<OrderRow>,
    /// Последние детекты ядра (кольцо, обрезается до MAX_DETECTS).
    pub detects: Vec<DetectRow>,
    /// Стратегии ядра (последний снимок; для окна стратегий).
    pub strategies: Vec<StrategyRow>,
    /// Схема стратегий ядра (секции/поля по видам). None пока не пришла.
    pub schema: Option<StrategySchemaModel>,
    /// Растёт при изменении ордеров / детектов / стратегий / схемы.
    pub orders_rev: u64,
    pub detects_rev: u64,
    pub strategies_rev: u64,
    pub schema_rev: u64,
}

impl CoreData {
    pub fn new() -> Self {
        Self {
            status: ConnStatus::Connecting,
            orders: Vec::new(),
            detects: Vec::new(),
            strategies: Vec::new(),
            schema: None,
            orders_rev: 0,
            detects_rev: 0,
            strategies_rev: 0,
            schema_rev: 0,
        }
    }

    /// Применяет только АККАУНТНЫЕ сообщения. Identity/Ticks/OrderBook координатор
    /// маршрутизирует мимо CoreData (в core_key / MarketStore), сюда не доходят.
    pub fn apply(&mut self, msg: FeedMsg) {
        match msg {
            FeedMsg::Status(s) => self.status = s,
            FeedMsg::Orders(orders) => {
                self.orders = orders;
                self.orders_rev = self.orders_rev.wrapping_add(1);
            }
            FeedMsg::Detects(detects) => {
                if !detects.is_empty() {
                    self.detects.extend(detects);
                    // Кольцо: держим только последние MAX_DETECTS.
                    if self.detects.len() > MAX_DETECTS {
                        let drop = self.detects.len() - MAX_DETECTS;
                        self.detects.drain(0..drop);
                    }
                    self.detects_rev = self.detects_rev.wrapping_add(1);
                }
            }
            FeedMsg::Strategies(strategies) => {
                self.strategies = strategies;
                self.strategies_rev = self.strategies_rev.wrapping_add(1);
            }
            FeedMsg::StrategySchema(schema) => {
                self.schema = Some(schema);
                self.schema_rev = self.schema_rev.wrapping_add(1);
            }
            // Рыночные/идентификационные сообщения сюда не маршрутизируются.
            FeedMsg::Identity(_) | FeedMsg::Ticks { .. } | FeedMsg::OrderBook { .. } => {}
        }
    }
}

impl Default for CoreData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct CoreStore {
    cores: HashMap<CoreId, CoreData>,
}

impl CoreStore {
    pub fn ensure(&mut self, id: CoreId) {
        self.cores.entry(id).or_default();
    }

    pub fn core(&self, id: CoreId) -> Option<&CoreData> {
        self.cores.get(&id)
    }

    pub fn core_mut(&mut self, id: CoreId) -> Option<&mut CoreData> {
        self.cores.get_mut(&id)
    }

    /// Снимок статусов всех ядер (id → клон статуса) — для бейджей в Настройках.
    pub fn statuses(&self) -> impl Iterator<Item = (CoreId, ConnStatus)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d.status.clone()))
    }
}
