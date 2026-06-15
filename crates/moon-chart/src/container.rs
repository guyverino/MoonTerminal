//! Типы-описания вкладок/панелей чарта, общие для UI-оболочки. Сама логика контейнера
//! (open/auto/prune/layout/режим) живёт в own-pass-оболочке (`chartdx::pane` в
//! moon-ui-gpui), которая ре-экспортит эти типы. wgpu-движок панелей (`Pane{chart:Chart}`)
//! удалён вместе с egui-бинарём.

use moon_core::session::CoreId;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    /// Главная вкладка: клики по детектам, фулскрин-центричный.
    Main,
    /// Чарт-вкладка AddToChart=`num`. `core` = Some(ядро), когда чарты разделены по
    /// ядрам (настройка charts_split_by_core); None — все ядра в одной вкладке.
    Chart { num: u32, core: Option<CoreId> },
}

/// Источник панели — влияет на TTL и поведение.
#[derive(Clone, Copy)]
pub enum PaneSource {
    /// Открыта вручную (клик по детекту) — живёт до закрытия крестиком.
    Manual,
    /// Авто-добавлена по AddToChart — живёт `ttl_ms` от последнего детекта.
    AddToChart { born_ms: f64, ttl_ms: f64 },
}

/// Режим показа контейнера.
#[derive(Clone, Copy)]
pub enum Mode {
    /// Видна одна панель на всю зону.
    Fullscreen(usize),
    /// Все панели стопкой по вертикали.
    Tiled,
}
