//! Окно «Стратегии» (порт egui `src/strategies/*` + `window/strategies_window.rs`).
//! Отдельное ОС-окно, 4 панели (дерево → секции → параметры): дерево ядро→папка→
//! стратегия с поиском/фильтрами/чекбоксами (стейджинг) и «Применить» (старт/стоп),
//! секции схемы выбранной стратегии (затемнение неактивных), плашки параметров
//! (read-only, YES/NO, «…» для длинных значений). Зависимости полей/разделов — из
//! `assets/param_deps.toml` (hot-reload, [`rules`]). Читает живой `Backend` (store
//! по ядрам), «Применить» шлёт `session.apply_strategies` (синхр. галок + старт/стоп).

mod filter;
mod rules;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_palette::{
    h_flex, v_flex, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize,
    MoonTextArea, MoonTextAreaEvent, MoonTextAreaState, MoonTone, Root, StyledExt,
};

use crate::{hex, Backend};
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::palette;
use moon_core::session::{CoreId, CoreStore};

use filter::StrategyFilter;
use rules::{Rules, Values};

pub type Key = (CoreId, u64);
type FieldEditKey = (CoreId, u64, String);

enum ParamsPanelModel {
    NoSelection,
    NoSchema,
    Content {
        section: SchemaSection,
        values: Values,
        row_pairs: Vec<(Key, StrategyRow)>,
        multi: bool,
        common: Option<HashSet<String>>,
        differ: bool,
    },
}

/// Цвет палитры с альфой → `Rgba` (0xRRGGBBAA). Для подсветки выбора/затемнения.
fn hexa(c: [u8; 3], a: u8) -> Rgba {
    rgba((c[0] as u32) << 24 | (c[1] as u32) << 16 | (c[2] as u32) << 8 | a as u32)
}

/// Состояние окна «Стратегии» (порт egui `StrategiesState` + рендер 4 панелей).
pub struct StrategiesView {
    backend: Entity<Backend>,
    /// Текстовое поле поиска — значение читаем в фильтр.
    search: Entity<MoonInputState>,
    /// Фильтры дерева (вид/направление/только активные); `search` синхр. из инпута.
    filter: StrategyFilter,
    /// Текущая (первичная) стратегия — источник схемы/секций (ядро, id).
    selected: Option<Key>,
    /// Множественный выбор (ядро, id) — подсветка + объединённый показ параметров.
    sel: HashSet<Key>,
    /// Якорь для range-выбора по Shift.
    anchor: Option<Key>,
    /// Плоский порядок видимых стратегий прошлого кадра — для Shift-диапазона.
    flat_order: Vec<Key>,
    /// Индекс выбранной секции в схеме её вида. НЕ сбрасывается при смене стратегии,
    /// только клампится при выходе за диапазон.
    selected_section: usize,
    /// Стейджинг чекбоксов: (ядро, id) → желаемый checked. Уходит на сервер по
    /// старт/стоп отмеченных, затем очищается.
    staged: HashMap<Key, bool>,
    /// Draft редактирования полей: (ядро, id, field) → новая строка UI.
    field_edits: HashMap<FieldEditKey, String>,
    /// Живые состояния single-line редакторов видимых/посещённых полей.
    field_inputs: HashMap<String, Entity<MoonInputState>>,
    /// Живые состояния memo/formula редакторов видимых/посещённых полей.
    field_memos: HashMap<String, Entity<MoonTextAreaState>>,
    /// Поле, для которого открыт контекстный helper/autocomplete.
    focused_field: Option<String>,
    /// Открытое окошко просмотра длинного значения поля: (имя поля, значение).
    popup: Option<(String, String)>,
    /// Раскрытые ядра в дереве.
    expanded_cores: HashSet<CoreId>,
    /// Раскрытые папки в дереве: (ядро, путь).
    expanded_folders: HashSet<(CoreId, String)>,
    /// Правила зависимостей полей (param_deps.toml; hot-reload).
    rules: Rules,
    /// Показывать только активные параметры (галка над параметрами).
    only_active_params: bool,
    focus: FocusHandle,
}

impl StrategiesView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| MoonInputState::new(window, cx).placeholder("поиск"));
        // Печать в поиске → перерисовать (значение читаем из инпута в render).
        cx.subscribe(&search, |_this, _e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        // Новые снимки стратегий/схемы (дренаж backend) → перерисовка; заодно
        // hot-reload правил зависимостей (param_deps.toml) — правка файла видна на лету.
        cx.observe(&backend, |this, _b, cx| {
            this.rules.reload_if_changed();
            cx.notify();
        })
        .detach();

        Self {
            backend,
            search,
            filter: StrategyFilter::default(),
            selected: None,
            sel: HashSet::new(),
            anchor: None,
            flat_order: Vec::new(),
            selected_section: 0,
            staged: HashMap::new(),
            field_edits: HashMap::new(),
            field_inputs: HashMap::new(),
            field_memos: HashMap::new(),
            focused_field: None,
            popup: None,
            expanded_cores: HashSet::new(),
            expanded_folders: HashSet::new(),
            rules: Rules::load(),
            // По умолчанию неактивные параметры скрыты (галка включена).
            only_active_params: true,
            focus: cx.focus_handle(),
        }
    }

    // ── Выбор ───────────────────────────────────────────────────────────────

    /// Клик по стратегии с учётом модификаторов: Shift — диапазон от якоря (по
    /// `order`), Ctrl/Cmd — добавить/убрать по одной, без модификатора — выбрать одну.
    fn apply_click(&mut self, key: Key, order: &[Key], shift: bool, command: bool) {
        if shift {
            if let Some(a) = self.anchor {
                let ia = order.iter().position(|k| *k == a);
                let ib = order.iter().position(|k| *k == key);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                    self.sel = order[lo..=hi].iter().copied().collect();
                } else {
                    self.sel = std::iter::once(key).collect();
                }
            } else {
                self.sel = std::iter::once(key).collect();
                self.anchor = Some(key);
            }
        } else if command {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor = Some(key);
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor = Some(key);
        }
        // Первичная (источник схемы/секций) — всегда кликнутая. Раздел не сбрасываем.
        self.selected = Some(key);
    }

    // ── Действия (старт/стоп отмеченных) ─────────────────────────────────────

    /// «Старт/стоп отмеченных»: на ядро — изменённые галки (diff стейджинга против
    /// серверного checked) + команда старт/стоп, если у ядра есть отмеченная стратегия
    /// или есть правки галок. Шлёт через `session.apply_strategies`, чистит стейджинг.
    fn apply_start_stop(
        &mut self,
        cores: &[(CoreId, String)],
        start: bool,
        cx: &mut Context<Self>,
    ) {
        // Собрать действия (читаем store), затем применить (повторный borrow backend).
        let mut actions: Vec<(CoreId, Vec<(u64, bool)>, bool)> = Vec::new();
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            for (core, _) in cores {
                let Some(cd) = store.core(*core) else {
                    continue;
                };
                let mut checks = Vec::new();
                let mut has_checked = false;
                for r in &cd.strategies {
                    let eff = self
                        .staged
                        .get(&(*core, r.id))
                        .copied()
                        .unwrap_or(r.checked);
                    if eff != r.checked {
                        checks.push((r.id, eff));
                    }
                    if eff {
                        has_checked = true;
                    }
                }
                if !checks.is_empty() || has_checked {
                    actions.push((*core, checks, start));
                }
            }
        }
        if actions.is_empty() {
            return;
        }
        let b = self.backend.read(cx);
        for (core, checks, st) in actions {
            b.session.apply_strategies(core, checks, Some(st));
        }
        self.staged.clear();
        cx.notify();
    }

    fn stage_field_value(
        &mut self,
        keys: &[Key],
        field: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        self.focused_field = Some(field.to_string());
        for (core, id) in keys {
            self.field_edits
                .insert((*core, *id, field.to_string()), value.clone());
        }
        cx.notify();
    }

    fn apply_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        let mut per_strategy: HashMap<(CoreId, u64), Vec<(String, String)>> = HashMap::new();
        for ((core, id, field), value) in &self.field_edits {
            per_strategy
                .entry((*core, *id))
                .or_default()
                .push((field.clone(), value.clone()));
        }
        let b = self.backend.read(cx);
        for ((core, id), changes) in per_strategy {
            b.session.edit_strategies(core, vec![id], changes);
        }
        self.clear_field_draft();
        cx.notify();
    }

    fn discard_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        self.clear_field_draft();
        cx.notify();
    }

    fn clear_field_draft(&mut self) {
        self.field_edits.clear();
        self.field_inputs.clear();
        self.field_memos.clear();
        self.focused_field = None;
    }

    fn field_input_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(state) = self.field_inputs.get(&id) {
            return state.clone();
        }
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_inputs.insert(id, state.clone());
        state
    }

    fn field_memo_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonTextAreaState> {
        if let Some(state) = self.field_memos.get(&id) {
            return state.clone();
        }
        let state = cx.new(|cx| MoonTextAreaState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonTextAreaEvent, cx| {
            if matches!(ev, MoonTextAreaEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_memos.insert(id, state.clone());
        state
    }

    fn append_formula_snippet(&mut self, field: &str, snippet: &str, cx: &mut Context<Self>) {
        let needle = field_id(field);
        let state = self
            .field_memos
            .iter()
            .find_map(|(id, state)| id.contains(&needle).then_some(state.clone()));
        let current = state
            .as_ref()
            .map(|state| state.read(cx).value().to_string())
            .unwrap_or_default();
        let next = append_snippet(&current, snippet);
        if let Some(state) = state {
            state.update(cx, |state, cx| state.set_value(next, cx));
        }
    }

    /// Развернуть все узлы (если `collapsed`) или свернуть все (иначе).
    fn expand_collapse_toggle(
        &mut self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        collapsed: bool,
    ) {
        if collapsed {
            for (c, _) in cores {
                self.expanded_cores.insert(*c);
                if let Some(cd) = store.core(*c) {
                    for r in &cd.strategies {
                        // Раскрываем каждый уровень пути (накопительные префиксы).
                        let mut acc = String::new();
                        for part in r.folder_path.split(['/', '\\']).filter(|s| !s.is_empty()) {
                            if !acc.is_empty() {
                                acc.push('/');
                            }
                            acc.push_str(part);
                            self.expanded_folders.insert((*c, acc.clone()));
                        }
                    }
                }
            }
        } else {
            self.expanded_cores.clear();
            self.expanded_folders.clear();
        }
    }

    // ── Панель 1: дерево ──────────────────────────────────────────────────────

    fn tree_panel(
        &self,
        store: &CoreStore,
        cores: &[(CoreId, String)],
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let accent = rgb(hex(palette::ACCENT));
        let border = rgb(hex(palette::LIFT_HOVER));

        // Поиск временно раскрывает всё; своё состояние раскрытия не трогаем.
        let force_open = self.filter.searching();

        // Узлы ядер → дерево.
        let mut list = v_flex().w_full().gap_0();
        for (core_id, core_name) in cores {
            let Some(cd) = store.core(*core_id) else {
                continue;
            };
            if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| self.filter.matches(r)) {
                continue;
            }
            let open = force_open || self.expanded_cores.contains(core_id);
            let total = cd
                .strategies
                .iter()
                .filter(|r| self.filter.counts(r))
                .count();
            let active = cd
                .strategies
                .iter()
                .filter(|r| self.filter.counts(r) && r.checked)
                .count();
            let label = format!(
                "{}  {}  {}/{}",
                if open { "▼" } else { "▶" },
                core_name,
                active,
                total
            );
            let cid = *core_id;
            list = list.child(
                div()
                    .id(SharedString::from(format!("core-{cid}")))
                    .w_full()
                    .py_0p5()
                    .cursor_pointer()
                    .font_bold()
                    .text_color(accent)
                    .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.expanded_cores, cid);
                        cx.notify();
                    })),
            );
            if !open {
                continue;
            }
            // Вложенное дерево папок (отступ слева — как egui ui.indent).
            let root = build_node(cd.strategies.iter().filter(|r| self.filter.matches(r)));
            let mut prefix: Vec<String> = Vec::new();
            let mut kids: Vec<AnyElement> = Vec::new();
            self.render_node(
                &root,
                &cd.strategies,
                *core_id,
                &mut prefix,
                force_open,
                order,
                built,
                &mut kids,
                cx,
            );
            let mut body = v_flex().w_full().pl_3().gap_0();
            for k in kids {
                body = body.child(k);
            }
            list = list.child(body);
        }

        // Поиск + фильтр вида + фильтр направления.
        let kinds = kinds_present(cores, store);
        let kind_text = self
            .filter
            .kind
            .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "все типы".to_string());
        let dir_text = match self.filter.dir {
            None => "все".to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };

        let collapsed = self.expanded_cores.is_empty() && self.expanded_folders.is_empty();
        let cores_owned: Arc<Vec<(CoreId, String)>> = Arc::new(cores.to_vec());

        v_flex()
            .w(px(220.0))
            .h_full()
            .border_r_1()
            .border_color(border)
            // ── Фильтры сверху ──
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .child(
                        div().w_full().child(
                            MoonInput::new("strat-search")
                                .state(&self.search)
                                .small()
                                .cleanable(true),
                        ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .items_center()
                            .child(self.combo_kind(kind_text, kinds, cx))
                            .child(self.combo_dir(dir_text, cx)),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                MoonCheckbox::new("flt-active")
                                    .label("только активные")
                                    .checked(self.filter.only_active)
                                    .size(MoonCheckboxSize::Compact)
                                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                                        this.filter.only_active = *ch;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                MoonButton::new("expand-all")
                                    .ghost()
                                    .size(MoonButtonSize::Micro)
                                    .label(if collapsed { "▼" } else { "▲" })
                                    .on_click({
                                        let cores = cores_owned.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            let store = this.backend.read(cx).session.store();
                                            let coll = this.expanded_cores.is_empty()
                                                && this.expanded_folders.is_empty();
                                            // store borrow tied to cx; clone cores for &-call.
                                            let cores_v = cores.as_ref().clone();
                                            this.expand_collapse_toggle(&cores_v, store, coll);
                                            cx.notify();
                                        })
                                    })
                                    .render(),
                            ),
                    ),
            )
            .child(div().w_full().h(px(1.0)).bg(border))
            // ── Прокручиваемый список ──
            .child(
                div()
                    .id("strat-tree-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .p_2()
                    .child(list),
            )
            // ── Нижняя панель действий ──
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(self.action_bar(cores_owned, store, cx))
            .into_any_element()
    }

    /// Комбобокс фильтра вида (попап-список: «все типы» + присутствующие виды).
    fn combo_kind(
        &self,
        current: String,
        kinds: Vec<(u8, String)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let selected_kind = self.filter.kind;
        let mut items = vec![MoonMenuItem::with_key("kind-all", "все типы")
            .selected(selected_kind.is_none())
            .on_click({
                let view = view.clone();
                move |_, _, app| {
                    view.update(app, |this, c| {
                        this.filter.kind = None;
                        c.notify();
                    });
                }
            })];
        for (ord, name) in kinds {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("kind-{ord}"), name.clone())
                    .selected(selected_kind == Some(ord))
                    .on_click({
                        let name_ord = ord;
                        move |_, _, app| {
                            view.update(app, |this, c| {
                                this.filter.kind = Some(name_ord);
                                c.notify();
                            });
                        }
                    }),
            );
        }
        MoonDropdown::new("strat-kind-filter")
            .label(format!("{current} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(116.0)
            .menu_width(180.0)
            .menu_size(MoonMenuSize::Compact)
            .menu_max_height(240.0)
            .items(items)
            .into_any_element()
    }

    /// Комбобокс фильтра направления (все/LONG/SHORT).
    fn combo_dir(&self, current: String, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let opts: [(&str, Option<bool>); 3] =
            [("все", None), ("LONG", Some(false)), ("SHORT", Some(true))];
        let mut items = Vec::with_capacity(opts.len());
        for (label, val) in opts {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("dir-{label}"), label)
                    .selected(self.filter.dir == val)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, c| {
                            this.filter.dir = val;
                            c.notify();
                        });
                    }),
            );
        }
        MoonDropdown::new("strat-dir-filter")
            .label(format!("{current} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(80.0)
            .menu_width(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }

    /// Нижняя панель действий: старт/стоп отмеченных + счётчик стейджинга.
    fn action_bar(
        &self,
        cores: Arc<Vec<(CoreId, String)>>,
        _store: &CoreStore,
        cx: &Context<Self>,
    ) -> AnyElement {
        // Кнопки видимы всегда (как egui); пустое действие — no-op в apply_start_stop.
        let cs = cores.clone();
        let mut row = h_flex().w_full().p_2().gap_2().items_center();
        row = row.child(
            MoonButton::new("start-checked")
                .primary()
                .size(MoonButtonSize::Micro)
                .label("▶ отмеченных")
                .on_click({
                    let cs = cs.clone();
                    cx.listener(move |this, _, _, cx| {
                        let cores_v = cs.as_ref().clone();
                        this.apply_start_stop(&cores_v, true, cx);
                    })
                })
                .render(),
        );
        row = row.child(
            MoonButton::new("stop-checked")
                .outline()
                .size(MoonButtonSize::Micro)
                .label("■ отмеченных")
                .on_click({
                    let cs = cs.clone();
                    cx.listener(move |this, _, _, cx| {
                        let cores_v = cs.as_ref().clone();
                        this.apply_start_stop(&cores_v, false, cx);
                    })
                })
                .render(),
        );
        if !self.staged.is_empty() {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(rgb(hex(palette::ACCENT)))
                    .child(format!("изменений: {}", self.staged.len())),
            );
        }
        row.into_any_element()
    }

    /// Рекурсивно собирает элементы узла: подпапки (сворачиваемые, с активн./всего),
    /// затем стратегии прямо в этой папке.
    #[allow(clippy::too_many_arguments)]
    fn render_node(
        &self,
        node: &FolderNode,
        strategies: &[StrategyRow],
        core_id: CoreId,
        prefix: &mut Vec<String>,
        force_open: bool,
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        out: &mut Vec<AnyElement>,
        cx: &Context<Self>,
    ) {
        for (name, child) in &node.children {
            prefix.push(name.clone());
            let path_key = prefix.join("/");
            let fkey = (core_id, path_key.clone());
            let fopen = force_open || self.expanded_folders.contains(&fkey);
            let (active, total) = folder_counts(strategies, &self.filter, prefix);
            let flabel = format!(
                "{}  {name}  {active}/{total}",
                if fopen { "▼" } else { "▶" }
            );
            let fkey_click = fkey.clone();
            out.push(
                div()
                    .id(SharedString::from(format!("folder-{core_id}-{path_key}")))
                    .w_full()
                    .py_0p5()
                    .cursor_pointer()
                    .text_color(rgb(hex(palette::TEXT)))
                    .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
                    .child(flabel)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.expanded_folders, fkey_click.clone());
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if fopen {
                let mut kids: Vec<AnyElement> = Vec::new();
                self.render_node(
                    child, strategies, core_id, prefix, force_open, order, built, &mut kids, cx,
                );
                let mut body = v_flex().w_full().pl_3().gap_0();
                for k in kids {
                    body = body.child(k);
                }
                out.push(body.into_any_element());
            }
            prefix.pop();
        }
        for r in &node.strategies {
            out.push(self.strategy_row(core_id, r, order, built, cx));
        }
    }

    /// Одна строка стратегии: чекбокс (стейджинг) · индикатор запуска · имя (выбор).
    fn strategy_row(
        &self,
        core: CoreId,
        r: &StrategyRow,
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let key = (core, r.id);
        built.push(key);
        let server = r.checked;
        let val = self.staged.get(&key).copied().unwrap_or(server);

        // Подсветка — для всех выбранных (мультивыбор), иначе для первичной.
        let highlighted = if self.sel.is_empty() {
            self.selected == Some(key)
        } else {
            self.sel.contains(&key)
        };
        let dot = if server {
            palette::GREEN
        } else {
            palette::TEXT_3
        };
        let type_col = if r.is_short {
            palette::RED
        } else {
            palette::TEXT_3
        };

        let order_c = order.clone();
        let mut name_row = div()
            .id(SharedString::from(format!("strat-{core}-{}", r.id)))
            .flex_1()
            .min_w_0()
            .px_1()
            .rounded(px(2.0))
            .cursor_pointer()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(hex(palette::TEXT)))
                            .child(r.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(hex(type_col)))
                            .child(r.kind.clone()),
                    ),
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                let m = e.modifiers();
                this.apply_click(key, &order_c, m.shift, m.secondary());
                cx.notify();
            }));
        if highlighted {
            name_row = name_row.bg(hexa(palette::ACCENT, 0x33));
        } else {
            name_row = name_row.hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))));
        }

        h_flex()
            .w_full()
            .items_center()
            .gap_1()
            .py_0p5()
            .child(
                MoonCheckbox::new(SharedString::from(format!("chk-{core}-{}", r.id)))
                    .checked(val)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        let v = *ch;
                        if v == server {
                            this.staged.remove(&key);
                        } else {
                            this.staged.insert(key, v);
                        }
                        cx.notify();
                    })),
            )
            .child(div().text_color(rgb(hex(dot))).child("●"))
            .child(name_row)
            .into_any_element()
    }

    // ── Панель 2: разделы (секции) ────────────────────────────────────────────

    fn sections_panel(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let border = rgb(hex(palette::LIFT_HOVER));
        let mut col = v_flex()
            .w(px(220.0))
            .h_full()
            .border_r_1()
            .border_color(border)
            .p_2()
            .gap_1()
            .child(div().font_bold().child("Разделы"))
            .child(div().w_full().h(px(1.0)).bg(border));

        let Some(sections) = selected_sections(self, store) else {
            return col
                .child(
                    div()
                        .mt_2()
                        .text_color(rgb(hex(palette::TEXT_2)))
                        .child("выберите стратегию в дереве"),
                )
                .into_any_element();
        };
        if sections.is_empty() {
            return col
                .child(
                    div()
                        .mt_2()
                        .text_color(rgb(hex(palette::TEXT_2)))
                        .child("схема не получена"),
                )
                .into_any_element();
        }
        let values = selected_values(self, store);

        // Порядок: сначала активные, потом неактивные; внутри групп — порядок схемы.
        let mut order: Vec<(usize, bool)> = sections
            .iter()
            .enumerate()
            .map(|(i, sec)| (i, section_active(&self.rules, &values, sec)))
            .collect();
        order.sort_by_key(|(_, active)| !active);

        let mut list = v_flex().w_full().gap_0();
        for (i, active) in order {
            let sec = &sections[i];
            let on = self.selected_section == i;
            let tcol = if !active {
                palette::TEXT_3
            } else {
                palette::TEXT
            };
            let mut row = div()
                .id(SharedString::from(format!("sec-{i}")))
                .w_full()
                .px_1()
                .py_0p5()
                .rounded(px(2.0))
                .cursor_pointer()
                .text_color(rgb(hex(tcol)))
                .child(sec.title.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected_section = i;
                    cx.notify();
                }));
            if on {
                row = row.bg(hexa(palette::ACCENT, 0x33));
            } else {
                row = row.hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))));
            }
            list = list.child(row);
        }
        col = col.child(
            div()
                .id("strat-sections-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        );
        col.into_any_element()
    }

    // ── Панель 3: параметры выбранной секции ────────────────────────────────

    fn params_model(&self, store: &CoreStore) -> ParamsPanelModel {
        if selected_row(self, store).is_none() {
            return ParamsPanelModel::NoSelection;
        }
        let Some(sections) = selected_sections(self, store) else {
            return ParamsPanelModel::NoSchema;
        };
        let Some(section) = sections.get(self.selected_section).cloned() else {
            return ParamsPanelModel::NoSchema;
        };
        let values = selected_values(self, store);
        let row_pairs: Vec<(Key, StrategyRow)> = multi_row_pairs(self, store)
            .into_iter()
            .map(|(key, row)| (key, row.clone()))
            .collect();
        let multi = row_pairs.len() > 1;
        let common = common_fields(self, store);
        let differ = kinds_differ(self, store);
        ParamsPanelModel::Content {
            section,
            values,
            row_pairs,
            multi,
            common,
            differ,
        }
    }

    fn params_panel(
        &mut self,
        model: ParamsPanelModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = v_flex().flex_1().h_full().p_2().gap_1();

        let ParamsPanelModel::Content {
            section,
            values,
            row_pairs,
            multi,
            common,
            differ,
        } = model
        else {
            let text = match model {
                ParamsPanelModel::NoSelection => "выберите стратегию в дереве",
                ParamsPanelModel::NoSchema => "схема не получена",
                ParamsPanelModel::Content { .. } => unreachable!(),
            };
            return col
                .child(div().mt_2().text_color(rgb(hex(palette::TEXT_2))).child(text))
                .into_any_element();
        };
        let keys: Vec<Key> = row_pairs.iter().map(|(key, _)| *key).collect();

        // Заголовок раздела + счётчик (полей / выбрано) справа.
        let count = if multi {
            format!("выбрано: {}", row_pairs.len())
        } else {
            format!("полей: {}", section.fields.len())
        };
        let dirty = self.field_edits.len();
        let mut header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(div().font_bold().child(section.title.clone()))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(hex(palette::TEXT_2)))
                            .child(count),
                    )
                    .when(dirty > 0, |row| {
                        row.child(
                            MoonButton::new("strat-fields-apply")
                                .success()
                                .size(MoonButtonSize::Micro)
                                .label(format!("apply {dirty}"))
                                .on_click(cx.listener(|this, _, _, cx| this.apply_field_edits(cx)))
                                .render(),
                        )
                        .child(
                            MoonButton::new("strat-fields-revert")
                                .ghost()
                                .size(MoonButtonSize::Micro)
                                .label("revert")
                                .on_click(cx.listener(|this, _, _, cx| this.discard_field_edits(cx)))
                                .render(),
                        )
                    }),
            );
        if dirty > 0 {
            header = header.border_l_2().border_color(hexa(palette::ORANGE, 0x99)).pl_2();
        }
        col = col
            .child(header)
            .child(
                MoonCheckbox::new("params-only-active")
                    .label("только активные")
                    .checked(self.only_active_params)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                        this.only_active_params = *ch;
                        cx.notify();
                    })),
            )
            .child(div().w_full().h(px(1.0)).bg(rgb(hex(palette::LIFT_HOVER))));

        // Порядок полей — как в схеме. Значения берём из снимка по имени.
        let mut list = v_flex().w_full().gap_0();
        for f in &section.fields {
            let lname = f.name.to_lowercase();
            if multi && lname == "strategyname" {
                continue;
            }
            if let Some(c) = &common {
                if !c.contains(&lname) {
                    continue;
                }
            }
            if differ && lname == "signaltype" {
                continue;
            }
            let active = self.rules.field_active(&f.name, &values);
            if self.only_active_params && !active {
                continue;
            }
            let merged = merged_value_for_owned(self, &row_pairs, f);
            list = list.child(self.field_row(f, &keys, merged, active, window, cx));
        }
        let scroll = div()
            .id("strat-params-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(list);
        let mut body = h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .items_start()
            .gap_2()
            .child(scroll);
        if let Some(helper) = self.formula_helper(cx) {
            body = body.child(helper);
        }
        col = col.child(body);
        col.into_any_element()
    }

    /// Строка поля: имя слева, значение справа. `active=false` — приглушаем тёмным.
    /// `merged=None` — значения у выбранных различаются (помечаем «≠», без значения).
    fn field_row(
        &mut self,
        f: &SchemaField,
        keys: &[Key],
        merged: Option<String>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name_col = if active {
            palette::TEXT_2
        } else {
            palette::TEXT_3
        };
        let val_col = if active {
            palette::TEXT
        } else {
            palette::TEXT_3
        };

        let dirty = keys
            .iter()
            .any(|(core, id)| self.field_edits.contains_key(&(*core, *id, f.name.clone())));
        let field_name = f.name.clone();
        let row_id = editor_state_id(keys, &field_name);
        let view = cx.entity();

        let value_el: AnyElement = match merged {
            None => div()
                .font_bold()
                .text_color(rgb(hex(palette::ACCENT)))
                .child("≠")
                .into_any_element(),
            Some(value) => match f.ui {
                SchemaFieldUi::Checkbox => {
                    let on = is_on(&value);
                    let keys = keys.to_vec();
                    let field = field_name.clone();
                    MoonCheckbox::new(SharedString::from(format!("field-check-{row_id}")))
                        .checked(on)
                        .disabled(!active)
                        .size(MoonCheckboxSize::Compact)
                        .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                            this.stage_field_value(
                                &keys,
                                &field,
                                if *ch { "Yes" } else { "No" }.to_string(),
                                cx,
                            );
                        }))
                        .into_any_element()
                }
                SchemaFieldUi::Combo if !f.picklist.is_empty() => {
                    let mut items = Vec::with_capacity(f.picklist.len());
                    for option in &f.picklist {
                        let option_value = option.clone();
                        let label = if option.is_empty() { "—".to_string() } else { option.clone() };
                        let keys = keys.to_vec();
                        let field = field_name.clone();
                        let view = view.clone();
                        items.push(
                            MoonMenuItem::with_key(format!("field-{row_id}-{option}"), label)
                                .selected(option_value == value)
                                .on_click(move |_, _, app| {
                                    view.update(app, |this, cx| {
                                        this.stage_field_value(&keys, &field, option_value.clone(), cx);
                                    });
                                }),
                        );
                    }
                    MoonDropdown::new(SharedString::from(format!("field-combo-{row_id}")))
                        .label(format!(
                            "{} ▾",
                            if value.is_empty() {
                                "—".to_string()
                            } else {
                                value.clone()
                            }
                        ))
                        .trigger_variant(if dirty {
                            MoonButtonVariant::Amber
                        } else {
                            MoonButtonVariant::Soft
                        })
                        .trigger_size(MoonButtonSize::Action)
                        .trigger_width(180.0)
                        .menu_width(220.0)
                        .menu_size(MoonMenuSize::Compact)
                        .menu_max_height(220.0)
                        .disabled(!active)
                        .items(items)
                        .into_any_element()
                }
                _ => {
                    let keys_arc = Arc::new(keys.to_vec());
                    if is_memo_field(f, &value) {
                        let state = self.field_memo_state(
                            row_id.clone(),
                            value,
                            keys_arc,
                            field_name.clone(),
                            window,
                            cx,
                        );
                        MoonTextArea::new(SharedString::from(format!("field-memo-{row_id}")))
                            .state(&state)
                            .formula()
                            .tone(MoonTone::Warning)
                            .selected(dirty)
                            .disabled(!active)
                            .into_any_element()
                    } else {
                        let state = self.field_input_state(
                            row_id.clone(),
                            value,
                            keys_arc,
                            field_name.clone(),
                            window,
                            cx,
                        );
                        MoonInput::new(SharedString::from(format!("field-input-{row_id}")))
                            .state(&state)
                            .small()
                            .tone(if matches!(f.ui, SchemaFieldUi::Color) {
                                MoonTone::Warning
                            } else {
                                MoonTone::Info
                            })
                            .selected(dirty)
                            .disabled(!active)
                            .into_any_element()
                    }
                }
            },
        };

        let field_for_focus = field_name.clone();
        h_flex()
            .id(SharedString::from(format!("field-row-{row_id}")))
            .w_full()
            .items_start()
            .gap_3()
            .py(px(4.0))
            .border_l(px(if dirty { 2.0 } else { 0.0 }))
            .border_color(hexa(palette::ORANGE, if dirty { 0x99 } else { 0x00 }))
            .pl(px(if dirty { 8.0 } else { 10.0 }))
            .pr_2()
            .hover(|s| s.bg(hexa(palette::LIFT_HOVER, 0x70)))
            .child(
                div()
                    .w(px(180.0))
                    .flex_none()
                    .pt(px(5.0))
                    .truncate()
                    .text_color(rgb(hex(name_col)))
                    .child(f.name.clone()),
            )
            .child(div().flex_1().min_w_0().text_color(rgb(hex(val_col))).child(value_el))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focused_field = Some(field_for_focus.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn formula_helper(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let field = self.focused_field.clone()?;
        if !is_formula_field(&field) {
            return None;
        }
        let snippets = formula_snippets();
        let mut list = v_flex().w_full().gap_1();
        for (label, detail, insert) in snippets {
            let field = field.clone();
            list = list.child(
                v_flex()
                    .id(SharedString::from(format!("helper-{label}")))
                    .w_full()
                    .rounded(px(2.0))
                    .border_1()
                    .border_color(rgb(hex(palette::LIFT_HOVER)))
                    .bg(rgb(hex(palette::LIFT)))
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.border_color(hexa(palette::ORANGE, 0xBB)))
                    .child(div().font_family("Geist Mono").text_size(px(11.0)).child(label))
                    .child(
                        div()
                            .font_family("Geist Mono")
                            .text_size(px(10.0))
                            .text_color(rgb(hex(palette::TEXT_2)))
                            .child(detail),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.append_formula_snippet(&field, insert, cx);
                    })),
            );
        }
        Some(
            v_flex()
                .w(px(260.0))
                .h_full()
                .flex_none()
                .gap_2()
                .p_3()
                .bg(rgb(hex(palette::SURFACE_1)))
                .border_l_1()
                .border_color(rgb(hex(palette::LIFT_HOVER)))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(hex(palette::TEXT_2)))
                        .child(format!("{field} · formula helper")),
                )
                .child(list)
                .into_any_element(),
        )
    }

    /// Окошко просмотра длинного значения (read-only) — оверлей поверх окна.
    fn popup_overlay(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (name, val) = self.popup.clone()?;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000073))
                .child(
                    v_flex()
                        .w(px(460.0))
                        .max_h(px(360.0))
                        .bg(rgb(hex(palette::SURFACE_1)))
                        .border_1()
                        .border_color(rgb(hex(palette::LIFT_HOVER)))
                        .rounded(px(6.0))
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(hex(palette::LIFT_HOVER)))
                                .child(div().font_bold().child(name))
                                .child(
                                    MoonButton::new("popup-close")
                                        .ghost()
                                        .size(MoonButtonSize::Micro)
                                        .label("×")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.popup = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                ),
                        )
                        .child(
                            div()
                                .id("popup-scroll")
                                .flex_1()
                                .w_full()
                                .overflow_y_scroll()
                                .p_3()
                                .text_color(rgb(hex(palette::TEXT)))
                                .child(val),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl EventEmitter<()> for StrategiesView {}
impl Focusable for StrategiesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StrategiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Поиск читаем из инпута в фильтр (единый источник).
        self.filter.search = self.search.read(cx).value().to_string();

        // Список ядер (id, имя) — все подключённые, как egui (session.sessions()).
        let cores: Vec<(CoreId, String)> = {
            let b = self.backend.read(cx);
            b.session
                .sessions()
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect()
        };

        // Клампим выбранный раздел в диапазон (как sections::show).
        {
            let store = self.backend.read(cx).session.store();
            if let Some(secs) = selected_sections(self, store) {
                if self.selected_section >= secs.len() {
                    self.selected_section = 0;
                }
            }
        }

        // Плоский порядок прошлого кадра (для Shift-диапазона) + новый накапливаем.
        let order = Arc::new(self.flat_order.clone());
        let mut built: Vec<Key> = Vec::new();

        let (tree, sections, params_model) = {
            let store = self.backend.read(cx).session.store();
            (
                self.tree_panel(store, &cores, &order, &mut built, cx),
                self.sections_panel(store, cx),
                self.params_model(store),
            )
        };
        let params = self.params_panel(params_model, window, cx);
        let overlay = self.popup_overlay(cx);

        // Сохранить порядок текущего кадра (store-borrow держит cx, не self).
        self.flat_order = built;

        let mut root = v_flex()
            .size_full()
            .relative()
            .bg(rgb(hex(palette::BG)))
            .text_color(rgb(hex(palette::TEXT)))
            .text_sm()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .child(tree)
                    .child(sections)
                    .child(params),
            );
        if let Some(overlay) = overlay {
            root = root.child(overlay);
        }
        root
    }
}

// ── Чистые помощники (порт `strategies/mod.rs`) ──────────────────────────────

/// Поиск строки стратегии в store.
fn row(store: &CoreStore, core: CoreId, id: u64) -> Option<&StrategyRow> {
    store.core(core)?.strategies.iter().find(|s| s.id == id)
}

/// Выбранная строка стратегии (по `selected`).
fn selected_row<'a>(st: &StrategiesView, store: &'a CoreStore) -> Option<&'a StrategyRow> {
    let (core, id) = st.selected?;
    row(store, core, id)
}

/// Ключи выбранных стратегий (мультивыбор) или первичная, если выбор пуст.
fn selected_keys(st: &StrategiesView) -> Vec<Key> {
    if st.sel.is_empty() {
        st.selected.into_iter().collect()
    } else {
        st.sel.iter().copied().collect()
    }
}

fn multi_row_pairs<'a>(st: &StrategiesView, store: &'a CoreStore) -> Vec<(Key, &'a StrategyRow)> {
    selected_keys(st)
        .iter()
        .filter_map(|(c, id)| row(store, *c, *id).map(|row| ((*c, *id), row)))
        .collect()
}

/// У выбранных РАЗНЫЕ виды стратегий? (тогда SignalType менять нельзя — скрываем).
fn kinds_differ(st: &StrategiesView, store: &CoreStore) -> bool {
    let mut kind: Option<u8> = None;
    for (c, id) in selected_keys(st) {
        if let Some(r) = row(store, c, id) {
            match kind {
                None => kind = Some(r.kind_ordinal),
                Some(k) if k != r.kind_ordinal => return true,
                _ => {}
            }
        }
    }
    false
}

/// Имена полей (lowercase) в схеме ядра `core` для вида `ord`.
fn kind_field_set(store: &CoreStore, core: CoreId, ord: u8) -> HashSet<String> {
    store
        .core(core)
        .and_then(|cd| cd.schema.as_ref())
        .and_then(|sch| sch.kinds.iter().find(|k| k.ordinal == ord))
        .map(|k| {
            k.sections
                .iter()
                .flat_map(|s| &s.fields)
                .map(|f| f.name.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// Поля (lowercase), которые есть у ВСЕХ выбранных стратегий (пересечение схем их
/// видов). None — выбрана одна (ограничения нет, показываем всё).
fn common_fields(st: &StrategiesView, store: &CoreStore) -> Option<HashSet<String>> {
    let keys = selected_keys(st);
    if keys.len() <= 1 {
        return None;
    }
    let mut acc: Option<HashSet<String>> = None;
    for (c, id) in keys {
        let Some(r) = row(store, c, id) else { continue };
        let set = kind_field_set(store, c, r.kind_ordinal);
        acc = Some(match acc {
            None => set,
            Some(a) => a.intersection(&set).cloned().collect(),
        });
    }
    acc
}

/// Значения полей выбранной стратегии: имя(lowercase) → значение(как есть) — для
/// вычисления зависимостей (depends_on). Несохранённые ядром поля добираем дефолтами.
fn selected_values(st: &StrategiesView, store: &CoreStore) -> Values {
    let mut v = Values::new();
    if let Some(row) = selected_row(st, store) {
        for (name, val) in &row.fields {
            v.insert(name.to_lowercase(), val.clone());
        }
        if let Some((core, id)) = st.selected {
            for ((c, sid, name), value) in &st.field_edits {
                if *c == core && *sid == id {
                    v.insert(name.to_lowercase(), value.clone());
                }
            }
        }
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

/// Раздел АКТИВЕН (не затемнён), если в нём осталось БОЛЬШЕ ОДНОГО активного поля.
fn section_active(rules: &Rules, values: &Values, sec: &SchemaSection) -> bool {
    sec.fields
        .iter()
        .filter(|f| rules.field_active(&f.name, values))
        .count()
        > 1
}

/// Секции схемы для выбранной стратегии (по её виду). None — нет выбора/схемы.
fn selected_sections<'a>(st: &StrategiesView, store: &'a CoreStore) -> Option<&'a [SchemaSection]> {
    let (core, id) = st.selected?;
    let cd = store.core(core)?;
    let row = cd.strategies.iter().find(|s| s.id == id)?;
    let schema = cd.schema.as_ref()?;
    let kind = schema
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)?;
    Some(&kind.sections)
}

fn merged_value_for_owned(
    st: &StrategiesView,
    rows: &[(Key, StrategyRow)],
    f: &SchemaField,
) -> Option<String> {
    let mut it = rows.iter().map(|(key, row)| edited_field_value(st, *key, row, f));
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

fn edited_field_value(st: &StrategiesView, key: Key, row: &StrategyRow, f: &SchemaField) -> String {
    st.field_edits
        .get(&(key.0, key.1, f.name.clone()))
        .cloned()
        .unwrap_or_else(|| field_value(row, f))
}

/// Значение поля стратегии (по имени) или дефолт схемы.
fn field_value(row: &StrategyRow, f: &SchemaField) -> String {
    row.fields
        .iter()
        .find(|(n, _)| n == &f.name)
        .map(|(_, v)| v.clone())
        .or_else(|| f.default.clone())
        .unwrap_or_default()
}

fn is_on(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
}

fn is_memo_field(f: &SchemaField, value: &str) -> bool {
    if value.contains('\n') || value.chars().count() > 44 {
        return true;
    }
    is_formula_field(&f.name)
        || matches!(f.ui, SchemaFieldUi::Edit)
            && value
                .chars()
                .any(|ch| matches!(ch, '<' | '>' | '(' | ')' | '&' | '|'))
}

fn is_formula_field(field: &str) -> bool {
    let name = field.to_ascii_lowercase();
    name.contains("custom")
        || name.contains("formula")
        || name.contains("ema")
        || name.contains("condition")
        || name.contains("filter")
}

fn formula_snippets() -> [(&'static str, &'static str, &'static str); 10] {
    [
        ("EMA(t,i)", "token EMA", "EMA(60s, 1)"),
        ("BTC(t,i)", "BTC market EMA", "BTC(60s, 1)"),
        ("MIN(t,i)", "min price change", "MIN(15m, 1)"),
        ("MAX(t,i)", "max price change", "MAX(15m, 1)"),
        ("MAvg(t,i)", "avg of all EMAs", "MAvg(5m, 1)"),
        ("Avg(t,i)", "price average", "Avg(5m, 1)"),
        ("Vol(t,i)", "volume indicator", "Vol(5m, 1)"),
        ("Arb(ex)", "arb spread", "Arb(GateS)"),
        ("EMA short", "EMA(60s,1)<{v}", "EMA(60s, 1) < "),
        ("Multi-TF", "MIN(15m,1)<{v} AND MIN(5m,1)<{v}", "MIN(15m, 1) <  AND MIN(5m, 1) < "),
    ]
}

fn field_id(field: &str) -> String {
    field
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .to_ascii_lowercase()
}

fn editor_state_id(keys: &[Key], field: &str) -> String {
    let mut key_parts: Vec<String> = keys.iter().map(|(core, id)| format!("{core}-{id}")).collect();
    key_parts.sort();
    format!("{}:{}", field_id(field), key_parts.join(","))
}

fn append_snippet(current: &str, snippet: &str) -> String {
    if current.trim().is_empty() {
        snippet.to_string()
    } else if current.ends_with(' ') || current.ends_with('\n') {
        format!("{current}{snippet}")
    } else {
        format!("{current} {snippet}")
    }
}

/// Переключает наличие ключа в множестве (раскрыт/свёрнут).
fn toggle<T: std::cmp::Eq + std::hash::Hash>(set: &mut HashSet<T>, key: T) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

/// Виды стратегий, присутствующие в дереве: (ordinal, имя), отсортировано по имени.
fn kinds_present(cores: &[(CoreId, String)], store: &CoreStore) -> Vec<(u8, String)> {
    let mut map: std::collections::BTreeMap<u8, String> = std::collections::BTreeMap::new();
    for (c, _) in cores {
        if let Some(cd) = store.core(*c) {
            for r in &cd.strategies {
                map.entry(r.kind_ordinal).or_insert_with(|| r.kind.clone());
            }
        }
    }
    let mut v: Vec<(u8, String)> = map.into_iter().collect();
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    v
}

/// Узел дерева папок: подпапки (по имени) + стратегии прямо в этой папке.
#[derive(Default)]
struct FolderNode<'a> {
    children: std::collections::BTreeMap<String, FolderNode<'a>>,
    strategies: Vec<&'a StrategyRow>,
}

/// Строит вложенное дерево из путей стратегий (`/` и `\` — разделители).
fn build_node<'a>(it: impl Iterator<Item = &'a StrategyRow>) -> FolderNode<'a> {
    let mut root = FolderNode::default();
    for r in it {
        let mut node = &mut root;
        for part in r.folder_path.split(['/', '\\']).filter(|s| !s.is_empty()) {
            node = node.children.entry(part.to_string()).or_default();
        }
        node.strategies.push(r);
    }
    root
}

/// Активных/всего (по фильтру типа/L/S) во всех стратегиях под путём `prefix`.
fn folder_counts(
    strategies: &[StrategyRow],
    filter: &StrategyFilter,
    prefix: &[String],
) -> (usize, usize) {
    let mut active = 0;
    let mut total = 0;
    for r in strategies {
        if !filter.counts(r) {
            continue;
        }
        let parts: Vec<&str> = r
            .folder_path
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= prefix.len()
            && prefix
                .iter()
                .zip(parts.iter())
                .all(|(a, b)| a.as_str() == *b)
        {
            total += 1;
            if r.checked {
                active += 1;
            }
        }
    }
    (active, total)
}

/// Открыть окно «Стратегии» (отдельное ОС-окно). Дедуп окон — в `Backend`.
pub fn open(backend: Entity<Backend>, cx: &mut App) {
    // Уже открыто → сфокусировать.
    if let Some(handle) = backend.read(cx).strategies_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1180.0), px(680.0)),
        })),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal — Стратегии".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| StrategiesView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        backend.update(cx, |bk, _| bk.strategies_window = Some(handle));
    }
}
