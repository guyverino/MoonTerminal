//! Состояние и раскладка окна «Стратегии». Рисуется в отдельном нативном окне
//! (см. window/strategies_window.rs). 4 панели: дерево (ядра→папки→стратегии),
//! секции выбранной стратегии, плашки параметров (read-only) и описание/хэлп.

pub mod params;
pub mod rules;
pub mod sections;
pub mod tree;

use std::collections::{HashMap, HashSet};

use crate::feed::{SchemaSection, StrategyRow};
use crate::session::{CoreId, CoreStore};
use rules::{Rules, Values};

/// Действие со стратегиями одного ядра: синхронизировать галки (`checks`) и,
/// если задано, стартовать/остановить отмеченные (`start_stop`).
pub struct StratAction {
    pub core: CoreId,
    pub checks: Vec<(u64, bool)>,
    pub start_stop: Option<bool>,
}

/// Итог кадра окна: действия к отправке ядрам (по нажатию старт/стоп отмеченных).
#[derive(Default)]
pub struct StrategiesOut {
    pub actions: Vec<StratAction>,
}

pub type Key = (CoreId, u64);

pub struct StrategiesState {
    /// Фильтр дерева по названию стратегии.
    pub search: String,
    /// Текущая (первичная) стратегия — источник схемы/секций (ядро, id).
    pub selected: Option<Key>,
    /// Множественный выбор (ядро, id) — подсветка + объединённый показ параметров.
    pub sel: HashSet<Key>,
    /// Якорь для range-выбора по Shift.
    pub anchor: Option<Key>,
    /// Плоский порядок видимых стратегий прошлого кадра — для Shift-диапазона.
    pub flat_order: Vec<Key>,
    /// Индекс выбранной секции в схеме её вида. НЕ сбрасывается при смене стратегии
    /// (остаёмся на том же разделе), только клампится при выходе за диапазон.
    pub selected_section: usize,
    /// Стейджинг чекбоксов: (ядро, id) → желаемый checked. Уходит на сервер по
    /// старт/стоп отмеченных, затем очищается (чекбокс снова следует серверу).
    pub staged: HashMap<(CoreId, u64), bool>,
    /// Открытое окошко просмотра длинного значения поля: (имя поля, значение).
    pub popup: Option<(String, String)>,
    /// Раскрытые ядра в дереве (своё состояние, не egui-персист — чтобы поиск мог
    /// временно раскрыть всё, а после очистки вернуть прежнюю свёрнутость).
    pub expanded_cores: HashSet<CoreId>,
    /// Раскрытые папки в дереве: (ядро, путь).
    pub expanded_folders: HashSet<(CoreId, String)>,
    /// Правила зависимостей полей (param_deps.toml; hot-reload).
    pub rules: Rules,
    /// Показывать только активные разделы (галка над списком разделов).
    pub only_active_sections: bool,
    /// Показывать только активные параметры (галка над параметрами).
    pub only_active_params: bool,
}

impl Default for StrategiesState {
    fn default() -> Self {
        Self {
            search: String::new(),
            selected: None,
            sel: HashSet::new(),
            anchor: None,
            flat_order: Vec::new(),
            selected_section: 0,
            staged: HashMap::new(),
            popup: None,
            expanded_cores: HashSet::new(),
            expanded_folders: HashSet::new(),
            rules: Rules::load(),
            // По умолчанию неактивные скрыты (галки включены).
            only_active_sections: true,
            only_active_params: true,
        }
    }
}

impl StrategiesState {
    /// Раскладывает 4 панели окна. `cores` — (id, имя) подключённых ядер по порядку.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        cores: &[(CoreId, String)],
        store: &CoreStore,
    ) -> StrategiesOut {
        let mut out = StrategiesOut::default();

        egui::SidePanel::left("strat-tree")
            .resizable(true)
            .default_width(300.0)
            .width_range(220.0..=480.0)
            .show(ctx, |ui| {
                tree::show(ui, self, cores, store, &mut out);
            });

        egui::SidePanel::left("strat-sections")
            .resizable(true)
            .default_width(285.0)
            .width_range(180.0..=460.0)
            .show(ctx, |ui| {
                sections::show(ui, self, store);
            });

        // Верхний margin как у боковых панелей (symmetric 8×2), иначе параметры
        // съезжают вниз относительно списка разделов.
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(8.0, 2.0)),
            )
            .show(ctx, |ui| {
                params::show(ui, self, store);
            });

        out
    }
}

/// Выбранная строка стратегии (по `selected`).
pub fn selected_row<'a>(st: &StrategiesState, store: &'a CoreStore) -> Option<&'a StrategyRow> {
    let (core, id) = st.selected?;
    row(store, core, id)
}

/// Поиск строки стратегии в store.
pub fn row(store: &CoreStore, core: CoreId, id: u64) -> Option<&StrategyRow> {
    store.core(core)?.strategies.iter().find(|s| s.id == id)
}

/// Строки всех выбранных стратегий ТОГО ЖЕ вида, что первичная (для объединённого
/// показа параметров). Пустой выбор → только первичная. Иные виды отбрасываем
/// (их поля не сопоставимы со схемой первичной).
pub fn multi_rows<'a>(st: &StrategiesState, store: &'a CoreStore) -> Vec<&'a StrategyRow> {
    let Some(prim) = selected_row(st, store) else {
        return Vec::new();
    };
    let keys: Vec<Key> = if st.sel.is_empty() {
        st.selected.into_iter().collect()
    } else {
        st.sel.iter().copied().collect()
    };
    keys.iter()
        .filter_map(|(c, id)| row(store, *c, *id))
        .filter(|r| r.kind_ordinal == prim.kind_ordinal)
        .collect()
}

/// Значения полей выбранной стратегии: имя(lowercase) → значение(как есть) — для
/// вычисления зависимостей (depends_on). Несохранённые ядром поля добираем
/// дефолтами схемы (как показывает сама плашка), иначе условие на такое поле не
/// сработало бы (его «нет» в снимке).
pub fn selected_values(st: &StrategiesState, store: &CoreStore) -> Values {
    let mut v = Values::new();
    if let Some(row) = selected_row(st, store) {
        for (name, val) in &row.fields {
            v.insert(name.to_lowercase(), val.clone());
        }
        // Кладём ВСЕ поля схемы (значение, иначе дефолт, иначе пусто) — так «есть в
        // values» ⟺ «поле существует у этого вида». Условие на поле ВНЕ схемы вида
        // (напр. HODLmode там, где его нет) считается неприменимым и не блокирует.
        if let Some(sections) = selected_sections(st, store) {
            for sec in sections {
                for f in &sec.fields {
                    v.entry(f.name.to_lowercase())
                        .or_insert_with(|| f.default.clone().unwrap_or_default());
                }
            }
        }
    }
    v
}

/// Раздел «осмысленный» (активный): есть хотя бы одно активное поле, НЕ являющееся
/// тумблером `Ignore*`. Так раздел гаснет при `IgnoreX=YES` (остаётся лишь активный
/// тумблер), но сам тумблер не считаем «содержимым».
pub fn section_active(rules: &Rules, values: &Values, sec: &SchemaSection) -> bool {
    sec.fields.iter().any(|f| {
        !f.name.to_lowercase().starts_with("ignore") && rules.field_active(&f.name, values)
    })
}

/// Секции схемы для выбранной стратегии (по её виду). None — нет выбора/схемы.
pub fn selected_sections<'a>(st: &StrategiesState, store: &'a CoreStore) -> Option<&'a [SchemaSection]> {
    let (core, id) = st.selected?;
    let cd = store.core(core)?;
    let row = cd.strategies.iter().find(|s| s.id == id)?;
    let schema = cd.schema.as_ref()?;
    let kind = schema.kinds.iter().find(|k| k.ordinal == row.kind_ordinal)?;
    Some(&kind.sections)
}

/// Раскрытые узлы (ядра/папки) форсим открытыми при активном поиске.
pub fn searching(st: &StrategiesState) -> bool {
    !st.search.trim().is_empty()
}

/// Совпадает ли стратегия с фильтром поиска (по названию, без регистра).
pub fn matches(st: &StrategiesState, row: &StrategyRow) -> bool {
    let q = st.search.trim().to_lowercase();
    q.is_empty() || row.name.to_lowercase().contains(&q)
}

/// Для удобства модулей: набор папок ядра (в порядке появления).
pub fn folders_of(rows: &[StrategyRow]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        if seen.insert(r.folder_path.clone()) {
            out.push(r.folder_path.clone());
        }
    }
    out
}
