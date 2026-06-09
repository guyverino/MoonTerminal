//! Контейнер графиков: несколько панелей chart+glass с режимом показа
//! (фулскрин одной / тайл всех по вертикали). Каждая панель — свой рендерер
//! [`Chart`] (вид/GPU-буферы/канвас независимы). Главный контейнер наполняется
//! кликами по детектам (фулскрин-фокус), AddToChart-контейнер — авто-панелями с
//! TTL (`KeepInChart`). См. docs/CHART_CONTAINERS_PLAN.md.

use crate::chart::view::Rect;
use crate::chart::Chart;
use crate::session::CoreId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// Главная вкладка: клики по детектам, фулскрин-центричный.
    Main,
    /// Чарт-вкладка №N (AddToChart=N): авто-панели монет с TTL, всегда тайл.
    Chart(u32),
}

/// Источник панели — влияет на TTL и поведение.
#[derive(Clone, Copy)]
pub enum PaneSource {
    /// Открыта вручную (клик по детекту) — живёт до закрытия крестиком.
    Manual,
    /// Авто-добавлена по AddToChart — живёт `ttl_ms` от последнего детекта.
    AddToChart { born_ms: f64, ttl_ms: f64 },
}

/// Одна панель chart+glass.
pub struct Pane {
    pub core: CoreId,
    pub market: String,
    pub source: PaneSource,
    pub chart: Chart,
}

/// Режим показа контейнера.
#[derive(Clone, Copy)]
pub enum Mode {
    /// Видна одна панель на всю зону.
    Fullscreen(usize),
    /// Все панели стопкой по вертикали.
    Tiled,
}

pub struct Container {
    pub kind: ContainerKind,
    pub panes: Vec<Pane>,
    pub mode: Mode,
}

impl Container {
    pub fn new(kind: ContainerKind) -> Self {
        Self {
            kind,
            panes: Vec::new(),
            mode: Mode::Fullscreen(0),
        }
    }

    /// Спецификация панелей (ядро/рынок/источник) — для переноса в откреплённое
    /// окно (GPU-ресурсы Chart не переносимы между девайсами, пересоздаём).
    pub fn spec(&self) -> Vec<(CoreId, String, PaneSource)> {
        self.panes
            .iter()
            .map(|p| (p.core, p.market.clone(), p.source))
            .collect()
    }

    /// Собрать контейнер из спецификации, создав `Chart` на указанном девайсе.
    pub fn from_spec(
        kind: ContainerKind,
        mode: Mode,
        spec: Vec<(CoreId, String, PaneSource)>,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        epoch_ms: f64,
    ) -> Self {
        let panes = spec
            .into_iter()
            .map(|(core, market, source)| Pane {
                core,
                market,
                source,
                chart: Chart::new(device, format, epoch_ms),
            })
            .collect();
        Self { kind, panes, mode }
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    fn find(&self, core: CoreId, market: &str) -> Option<usize> {
        self.panes
            .iter()
            .position(|p| p.core == core && p.market == market)
    }

    /// Ручное открытие монеты: найти/добавить панель и показать её фулскрином.
    pub fn open_manual(
        &mut self,
        core: CoreId,
        market: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        epoch_ms: f64,
    ) {
        let idx = self.find(core, market).unwrap_or_else(|| {
            self.panes.push(Pane {
                core,
                market: market.to_string(),
                source: PaneSource::Manual,
                chart: Chart::new(device, format, epoch_ms),
            });
            self.panes.len() - 1
        });
        self.mode = Mode::Fullscreen(idx);
    }

    /// AddToChart-детект: найти/добавить панель монеты, продлить TTL, режим тайл.
    pub fn push_auto(
        &mut self,
        core: CoreId,
        market: &str,
        now_ms: f64,
        ttl_ms: f64,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        epoch_ms: f64,
    ) {
        match self.find(core, market) {
            Some(i) => {
                self.panes[i].source = PaneSource::AddToChart { born_ms: now_ms, ttl_ms };
            }
            None => {
                self.panes.push(Pane {
                    core,
                    market: market.to_string(),
                    source: PaneSource::AddToChart { born_ms: now_ms, ttl_ms },
                    chart: Chart::new(device, format, epoch_ms),
                });
            }
        }
        self.mode = Mode::Tiled;
    }

    /// Удалить истёкшие AddToChart-панели (сам контейнер остаётся). True — если
    /// что-то удалили.
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        let before = self.panes.len();
        self.panes.retain(|p| match p.source {
            PaneSource::AddToChart { born_ms, ttl_ms } => now_ms - born_ms < ttl_ms,
            PaneSource::Manual => true,
        });
        let removed = self.panes.len() != before;
        if removed {
            self.clamp_focus();
        }
        removed
    }

    /// Есть ли AddToChart-панели (для гонки кадров под их TTL-истечение).
    pub fn has_ttl_panes(&self) -> bool {
        self.panes
            .iter()
            .any(|p| matches!(p.source, PaneSource::AddToChart { .. }))
    }

    pub fn remove(&mut self, idx: usize) {
        if idx < self.panes.len() {
            self.panes.remove(idx);
            self.clamp_focus();
        }
    }

    fn clamp_focus(&mut self) {
        if let Mode::Fullscreen(i) = &mut self.mode {
            *i = (*i).min(self.panes.len().saturating_sub(1));
        }
    }

    /// Тоггл фулскрин ↔ тайл (ПКМ-клик). `focus` — панель под курсором (для входа
    /// в фулскрин).
    pub fn toggle_mode(&mut self, focus: usize) {
        self.mode = match self.mode {
            Mode::Fullscreen(_) => Mode::Tiled,
            Mode::Tiled => Mode::Fullscreen(focus.min(self.panes.len().saturating_sub(1))),
        };
    }

    /// Раскладка видимых панелей: список (индекс панели, прямоугольник) в
    /// координатах `content` (физ. пиксели).
    pub fn layout(&self, content: Rect) -> Vec<(usize, Rect)> {
        if self.panes.is_empty() {
            return Vec::new();
        }
        match self.mode {
            Mode::Fullscreen(i) => vec![(i.min(self.panes.len() - 1), content)],
            Mode::Tiled => {
                let n = self.panes.len();
                let h = content.h / n as f32;
                (0..n)
                    .map(|k| {
                        (
                            k,
                            Rect {
                                x: content.x,
                                y: content.y + h * k as f32,
                                w: content.w,
                                h,
                            },
                        )
                    })
                    .collect()
            }
        }
    }

}
