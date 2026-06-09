//! Отдельное нативное окно «Стратегии»: 4 панели (дерево/секции/параметры/хэлп).
//! Читает аккуратный план ядер из `SessionManager` (store + имена ядер); по
//! «Применить» возвращает наружу команды старт/стоп (App шлёт их через session).
//! egui-конвейер — общий [`EguiSurface`].

use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::session::{CoreId, SessionManager};
use crate::strategies::{StratAction, StrategiesState};
use crate::window::{AuxWindow, EguiSurface};

/// Итог кадра окна для App: действия со стратегиями (синхронизация галок + старт/стоп).
pub struct StrategiesWinOut {
    pub actions: Vec<StratAction>,
}

pub struct StrategiesWindow {
    pub window: Arc<Window>,
    egui: EguiSurface,
    state: StrategiesState,
    /// Сигнатура данных стратегий/схемы по ядрам — для авто-перерисовки при приходе
    /// новых снимков (как generation у окна отчётов).
    last_data_sig: u64,
}

impl StrategiesWindow {
    pub fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(t!("strat.window_title").to_string())
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1180.0, 680.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_window_icon(crate::icons::brand_winit_icon());

        let egui = EguiSurface::new(&window)?;
        Ok(Self {
            window,
            egui,
            state: StrategiesState::default(),
            last_data_sig: 0,
        })
    }

    /// Сверяет сигнатуру данных стратегий/схемы всех ядер: пришёл новый снимок →
    /// перерисовать (отразить новые checked/состав/схему).
    pub fn poll(&mut self, session: &SessionManager) {
        let mut sig = 0u64;
        for s in &session.sessions {
            if let Some(cd) = session.store.core(s.id) {
                sig = sig
                    .wrapping_add(cd.strategies_rev)
                    .wrapping_mul(31)
                    .wrapping_add(cd.schema_rev);
            }
        }
        if sig != self.last_data_sig {
            self.last_data_sig = sig;
            self.egui.mark_dirty();
        }
        // Hot-reload правил зависимостей (param_deps.toml) — правка файла видна на лету.
        if self.state.rules.reload_if_changed() {
            self.egui.mark_dirty();
        }
    }

    pub fn needs_render(&self) -> bool {
        self.egui.needs_render()
    }

    /// Кадр окна. Читает данные из `session`; возвращает команды старт/стоп.
    pub fn render(&mut self, session: &SessionManager) -> StrategiesWinOut {
        let cores: Vec<(CoreId, String)> = session
            .sessions
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();
        let state = &mut self.state;
        let store = &session.store;
        let mut actions = Vec::new();
        self.egui.render(&self.window, "strategies-pass", |ctx| {
            actions = state.ui(ctx, &cores, store).actions;
        });
        StrategiesWinOut { actions }
    }
}

impl AuxWindow for StrategiesWindow {
    fn on_event(&mut self, event: &WindowEvent) -> bool {
        self.egui.on_event(&self.window, event)
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.egui.resize(size);
    }
}
