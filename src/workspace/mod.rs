//! Workspace = группа ядер с собственной раскладкой (dock). Группа = ОС-окно
//! (WindowHost держит по одному Workspace).

use crate::config::AppConfig;
use crate::dock::Dock;
use crate::session::CoreId;

pub struct CoreInfo {
    pub id: CoreId,
    pub name: String,
    /// Цвет ядра из конфига — цвет кнопки детекта в ленте.
    pub color: [u8; 3],
    /// Quote подключения ядра (`USDT`/…), который режем из символов в UI.
    pub quote: String,
}

pub struct Workspace {
    pub group: String,
    pub icon: u32,
    pub cores: Vec<CoreInfo>,
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
                    dock: Dock::new(),
                });
            }
        }
        out
    }

    /// Пустой workspace, чтобы было одно окно (открыть Настройки), когда серверов нет.
    pub fn placeholder() -> Self {
        Self {
            group: "—".to_string(),
            icon: 0,
            cores: Vec::new(),
            dock: Dock::new(),
        }
    }
}
