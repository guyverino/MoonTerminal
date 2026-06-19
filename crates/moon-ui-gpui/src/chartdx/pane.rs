//! Контейнер панелей чарта — ЛОГИКА (open/auto/prune/layout/режим), без GPU. Порт смыслов
//! `moon_chart::container`, но панель хранит только `ChartView` (математика вида), а не
//! wgpu-движок: рисуем своим own-pass (`super::combo`/…). GPU-состояние слоёв живёт отдельно
//! в `RenderState` (см. `mod.rs`), синхронизируется с этими панелями по индексу.

use moon_chart::view::{ChartView, Rect};
use moon_core::session::CoreId;

// Режимы/виды/источник панели — переиспользуем типы движка (контракт UX тот же).
pub use moon_chart::container::{ContainerKind, Mode, PaneSource};

/// Применить масштаб цены к виду: None = Авто, Some(доля) = процент от цены.
fn apply_scale(view: &mut ChartView, pct: Option<f32>) {
    match pct {
        None => view.set_auto(),
        Some(p) => view.set_scale_percent(p),
    }
}

/// Одна панель: ядро/рынок/источник + вид (координаты). GPU-слои — в `RenderState` по индексу.
#[derive(Clone)]
pub struct Pane {
    pub core: CoreId,
    pub market: String,
    pub source: PaneSource,
    pub view: ChartView,
    /// П.2: пользователь «приколол» AddToChart-панель → TTL не закрывает её. На Manual-панели
    /// не влияет (они и так живут вечно). Сессионный флаг (панели сами по себе не персистятся).
    pub pinned: bool,
}

#[derive(Clone)]
pub struct Container {
    /// Идентичность вкладки (Main / Chart{num}); используется при persist раскладки (позже).
    #[allow(dead_code)]
    pub kind: ContainerKind,
    pub panes: Vec<Pane>,
    pub mode: Mode,
    /// Текущий масштаб цены контейнера (None=Авто): новые панели создаются сразу с ним.
    scale: Option<f32>,
}

impl Container {
    pub fn new(kind: ContainerKind) -> Self {
        Self {
            kind,
            panes: Vec::new(),
            mode: Mode::Fullscreen(0),
            scale: None,
        }
    }

    fn new_view(&self, epoch_ms: f64) -> ChartView {
        let mut view = ChartView::new(epoch_ms);
        apply_scale(&mut view, self.scale);
        view
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    fn find(&self, core: CoreId, market: &str) -> Option<usize> {
        self.panes
            .iter()
            .position(|p| p.core == core && p.market == market)
    }

    /// Задать масштаб цены контейнера: применить ко ВСЕМ панелям и запомнить для будущих.
    pub fn set_scale(&mut self, pct: Option<f32>) {
        self.scale = pct;
        for p in &mut self.panes {
            apply_scale(&mut p.view, pct);
        }
    }

    /// Ручное открытие монеты: найти/добавить панель и показать фулскрином.
    pub fn open_manual(&mut self, core: CoreId, market: &str, epoch_ms: f64) {
        let idx = match self.find(core, market) {
            Some(i) => i,
            None => {
                let view = self.new_view(epoch_ms);
                self.panes.push(Pane {
                    core,
                    market: market.to_string(),
                    source: PaneSource::Manual,
                    view,
                    pinned: false,
                });
                self.panes.len() - 1
            }
        };
        self.mode = Mode::Fullscreen(idx);
    }

    /// AddToChart-детект: найти/добавить панель монеты, продлить TTL, режим тайл.
    pub fn push_auto(
        &mut self,
        core: CoreId,
        market: &str,
        now_ms: f64,
        ttl_ms: f64,
        epoch_ms: f64,
    ) {
        match self.find(core, market) {
            Some(i) => {
                self.panes[i].source = PaneSource::AddToChart {
                    born_ms: now_ms,
                    ttl_ms,
                };
            }
            None => {
                let view = self.new_view(epoch_ms);
                self.panes.push(Pane {
                    core,
                    market: market.to_string(),
                    source: PaneSource::AddToChart {
                        born_ms: now_ms,
                        ttl_ms,
                    },
                    view,
                    pinned: false,
                });
            }
        }
        self.mode = Mode::Tiled;
    }

    /// Удалить истёкшие AddToChart-панели. True — если что-то удалили (→ пере-рендер).
    pub fn prune_ttl(&mut self, now_ms: f64) -> bool {
        let before = self.panes.len();
        self.panes.retain(|p| match p.source {
            // П.2: приколотая панель не закрывается по TTL.
            PaneSource::AddToChart { born_ms, ttl_ms } => p.pinned || now_ms - born_ms < ttl_ms,
            PaneSource::Manual => true,
        });
        let removed = self.panes.len() != before;
        if removed {
            self.clamp_focus();
        }
        removed
    }

    pub fn has_ttl_panes(&self) -> bool {
        self.panes
            .iter()
            .any(|p| matches!(p.source, PaneSource::AddToChart { .. }) && !p.pinned)
    }

    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.panes
            .iter()
            .filter_map(|p| match p.source {
                // Приколотые панели дедлайна не имеют (П.2).
                PaneSource::AddToChart { born_ms, ttl_ms } if !p.pinned => Some(born_ms + ttl_ms),
                _ => None,
            })
            .min_by(|a, b| a.total_cmp(b))
    }

    /// Можно ли приколоть панель idx (только AddToChart с TTL; Manual/Main — нет смысла). П.2
    pub fn is_pinnable(&self, idx: usize) -> bool {
        self.panes
            .get(idx)
            .is_some_and(|p| matches!(p.source, PaneSource::AddToChart { .. }))
    }

    pub fn is_pinned(&self, idx: usize) -> bool {
        self.panes.get(idx).is_some_and(|p| p.pinned)
    }

    /// Переключить пин панели idx. Возвращает новое состояние (или None — индекс вне диапазона).
    pub fn toggle_pin(&mut self, idx: usize) -> Option<bool> {
        let p = self.panes.get_mut(idx)?;
        p.pinned = !p.pinned;
        Some(p.pinned)
    }

    /// Удалить панель (закрытие крестиком в UI). Возвращает её (core, market) — для решения
    /// об отписке от стакана. None — индекс вне диапазона.
    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        if idx >= self.panes.len() {
            return None;
        }
        let p = self.panes.remove(idx);
        self.clamp_focus();
        Some((p.core, p.market))
    }

    /// Использует ли ещё какая-то панель этот (core, market) — чтобы не отписаться от стакана,
    /// который нужен другой панели этого же чарта.
    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.panes
            .iter()
            .any(|p| p.core == core && p.market == market)
    }

    /// Закрыть ВСЕ панели (кнопка «закрыть все графики» в выносном окне). Возвращает их
    /// (core, market) — для отписки от стаканов.
    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        let out = self
            .panes
            .iter()
            .map(|p| (p.core, p.market.clone()))
            .collect();
        self.panes.clear();
        self.clamp_focus();
        out
    }

    fn clamp_focus(&mut self) {
        if let Mode::Fullscreen(i) = &mut self.mode {
            *i = (*i).min(self.panes.len().saturating_sub(1));
        }
    }

    /// Тоггл фулскрин ↔ тайл (ПКМ). `focus` — панель под курсором (для входа в фулскрин).
    pub fn toggle_mode(&mut self, focus: usize) {
        self.mode = match self.mode {
            Mode::Fullscreen(_) => Mode::Tiled,
            Mode::Tiled => Mode::Fullscreen(focus.min(self.panes.len().saturating_sub(1))),
        };
    }

    /// Раскладка видимых панелей: (индекс панели, прямоугольник) в координатах `content` (физ. px).
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
