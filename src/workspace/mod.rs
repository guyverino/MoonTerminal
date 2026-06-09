//! Workspace = группа ядер с собственной раскладкой (dock). Группа = ОС-окно
//! (WindowHost держит по одному Workspace).

use crate::config::AppConfig;
use crate::dock::Dock;
use crate::session::CoreId;

pub struct CoreInfo {
    pub id: CoreId,
    pub name: String,
    /// Рынок по умолчанию ядра (для вывода quote подключения).
    pub market: String,
    /// Цвет ядра из конфига — цвет кнопки детекта в ленте.
    pub color: [u8; 3],
    /// Quote подключения ядра (`USDT`/…), который режем из символов в UI.
    pub quote: String,
}

/// Открытый чарт окна: какое ядро и какой рынок сейчас показываем.
#[derive(Clone)]
pub struct OpenChart {
    pub core: CoreId,
    pub market: String,
}

pub struct Workspace {
    pub group: String,
    pub icon: u32,
    pub cores: Vec<CoreInfo>,
    /// Открытый чарт (по клику на детект). None = пусто (серый контейнер).
    pub open: Option<OpenChart>,
    pub dock: Dock,
}

impl Workspace {
    /// Группирует серверы конфига по полю `group` (порядок появления сохраняется).
    pub fn build_all(config: &AppConfig) -> Vec<Workspace> {
        let mut out: Vec<Workspace> = Vec::new();
        for s in config
            .servers
            .iter()
            .filter(|s| s.active && s.show_window && config.group(&s.group).active)
        {
            let info = CoreInfo {
                id: s.id,
                name: if s.name.is_empty() {
                    format!("core {}", s.id)
                } else {
                    s.name.clone()
                },
                market: s.market.clone(),
                color: s.color,
                quote: crate::symbol::resolve_quote(&s.market),
            };
            if let Some(ws) = out.iter_mut().find(|w| w.group == s.group) {
                ws.cores.push(info);
            } else {
                out.push(Workspace {
                    group: s.group.clone(),
                    icon: config.group(&s.group).icon,
                    cores: vec![info],
                    open: None,
                    dock: Dock::new(),
                });
            }
        }
        out
    }

    /// Ядро открытого чарта (0 = ничего не открыто) — для подписки/данных.
    pub fn active_core(&self) -> CoreId {
        self.open.as_ref().map(|o| o.core).unwrap_or(0)
    }

    /// Базовая монета открытого рынка (без quote подключения) или «—», когда пусто.
    pub fn active_market(&self) -> &str {
        match &self.open {
            Some(o) => {
                let quote = self
                    .cores
                    .iter()
                    .find(|c| c.id == o.core)
                    .map(|c| c.quote.as_str())
                    .unwrap_or("");
                crate::symbol::base_symbol(&o.market, quote)
            }
            None => "—",
        }
    }

    /// Пустой workspace, чтобы было одно окно (открыть Настройки), когда серверов нет.
    pub fn placeholder() -> Self {
        Self {
            group: "—".to_string(),
            icon: 0,
            cores: Vec::new(),
            open: None,
            dock: Dock::new(),
        }
    }
}
