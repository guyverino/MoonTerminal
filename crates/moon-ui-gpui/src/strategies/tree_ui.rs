//! UI операций над деревом стратегий: тулбар выделения, модалки (создать/переименовать/
//! подтвердить удаление), ПКМ-контекст-меню и диспетч в `moon-core` (группировка по ядрам).
//! Чистая логика над путями/наборами — в [`super::tree_ops`]; команды ядра — в `session`.

use super::tree_ops;
use super::*;
use moon_core::feed::NewStrategySpec;

/// Активная модалка операции (взаимоисключающая; рисуется оверлеем поверх окна).
pub(super) enum TreeOp {
    /// Создать стратегию: целевая папка + выбранный вид (kind ordinal).
    CreateStrategy { core: CoreId, target: String, kind: Option<u8> },
    /// Создать (UI-)папку: целевой родитель.
    CreateFolder { core: CoreId, target: String },
    /// Переименовать папку: ядро + путь папки (сегменты).
    RenameFolder { core: CoreId, old_path: Vec<String> },
    /// Подтверждение удаления стратегий выделения (id переderives при подтверждении).
    ConfirmDeleteStrategies { label: String },
    /// Подтверждение удаления папки: ядро + путь, подпись.
    ConfirmDeleteFolder { core: CoreId, path: Vec<String>, label: String },
}

/// Открытое контекст-меню: цель + позиция курсора.
pub(super) struct ContextMenu {
    pub(super) core: CoreId,
    pub(super) target: MenuTarget,
    pub(super) pos: Point<Pixels>,
}

pub(super) enum MenuTarget {
    Folder(Vec<String>),
    Strategy(u64),
}

/// Полезная нагрузка drag&drop: перетаскиваемые стратегии (ядро-источник + id).
#[derive(Clone)]
pub(super) struct StratDrag {
    pub(super) core: CoreId,
    pub(super) ids: Vec<u64>,
}

/// Полезная нагрузка drag&drop: перетаскиваемая папка (ядро-источник + путь).
#[derive(Clone)]
pub(super) struct FolderDrag {
    pub(super) core: CoreId,
    pub(super) path: Vec<String>,
}

/// Превью под курсором при перетаскивании.
pub(super) struct DragChip {
    pub(super) label: SharedString,
}

impl Render for DragChip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(moon(p.shell_high))
            .border_1()
            .border_color(moon(p.blue))
            .text_color(moon(p.text))
            .text_size(design::text_px(cx, 10.5))
            .font_family("Geist Mono")
            .child(self.label.clone())
    }
}

impl StrategiesView {
    // ── Утилиты ───────────────────────────────────────────────────────────────

    /// Виды (ordinal, имя) из схемы ядра — для выбора при создании стратегии.
    fn kinds_of(&self, store: &CoreStore, core: CoreId) -> Vec<(u8, String)> {
        store
            .core(core)
            .and_then(|cd| cd.schema.as_ref())
            .map(|s| s.kinds.iter().map(|k| (k.ordinal, k.name.clone())).collect())
            .unwrap_or_default()
    }

    /// Выбранные строки (мультивыбор) с их ядром — owned-копии (для буфера/проверок).
    fn selection_rows(&self, store: &CoreStore) -> Vec<(CoreId, StrategyRow)> {
        selected_keys(self)
            .into_iter()
            .filter_map(|(c, id)| row(store, c, id).map(|r| (c, r.clone())))
            .collect()
    }

    /// Целевая папка по умолчанию (ядро, путь) — папка первичной стратегии или корень
    /// первого ядра.
    pub(super) fn default_target(
        &self,
        store: &CoreStore,
        cores: &[(CoreId, String)],
    ) -> (CoreId, String) {
        if let Some((core, id)) = self.selected {
            if let Some(r) = row(store, core, id) {
                return (core, r.folder_path.clone());
            }
        }
        (cores.first().map(|(c, _)| *c).unwrap_or(0), String::new())
    }

    // ── Открытие модалок/меню ────────────────────────────────────────────────

    pub(super) fn open_create_strategy(
        &mut self,
        core: CoreId,
        target: String,
        cx: &mut Context<Self>,
    ) {
        let store = self.backend.read(cx).session.store();
        let kind = self.kinds_of(store, core).first().map(|(o, _)| *o);
        self.menu = None;
        self.op_input_init = String::new();
        self.op_input = None; // render пересоздаст свежий ввод
        self.op = Some(TreeOp::CreateStrategy { core, target, kind });
        cx.notify();
    }

    pub(super) fn open_create_folder(
        &mut self,
        core: CoreId,
        target: String,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        self.op_input_init = String::new();
        self.op_input = None;
        self.op = Some(TreeOp::CreateFolder { core, target });
        cx.notify();
    }

    pub(super) fn open_rename_folder(
        &mut self,
        core: CoreId,
        old_path: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let cur = old_path.last().cloned().unwrap_or_default();
        self.menu = None;
        self.op_input_init = cur;
        self.op_input = None;
        self.op = Some(TreeOp::RenameFolder { core, old_path });
        cx.notify();
    }

    /// Запросить удаление стратегий выделения (с проверкой правила «все выключены»).
    pub(super) fn request_delete_selection(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        // Правило: удалять можно, только если ВСЕ выбранные выключены.
        if rows.iter().any(|(_, r)| r.checked) {
            return;
        }
        // Выделение может охватывать разные ядра — подтверждение одно, диспетч группирует
        // по ядрам (см. delete_selection, переderives выделение).
        self.op = Some(TreeOp::ConfirmDeleteStrategies {
            label: format!("{} стратеги(й)", rows.len()),
        });
        cx.notify();
    }

    /// Запросить удаление папки (правило: все стратегии под ней выключены).
    pub(super) fn request_delete_folder(
        &mut self,
        core: CoreId,
        path: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let store = self.backend.read(cx).session.store();
        let Some(cd) = store.core(core) else { return };
        let under = tree_ops::rows_under(&cd.strategies, &path);
        if !tree_ops::all_off(&under) {
            return; // есть запущенные — нельзя
        }
        let label = format!("папку «{}»", path.last().cloned().unwrap_or_default());
        self.op = Some(TreeOp::ConfirmDeleteFolder { core, path, label });
        cx.notify();
    }

    // ── Буфер (копировать/вставить) ──────────────────────────────────────────

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        let refs: Vec<&StrategyRow> = rows.iter().map(|(_, r)| r).collect();
        self.clipboard = Some(tree_ops::copy_rows(&refs));
        cx.notify();
    }

    pub(super) fn copy_folder(&mut self, core: CoreId, path: Vec<String>, cx: &mut Context<Self>) {
        self.menu = None;
        let store = self.backend.read(cx).session.store();
        let Some(cd) = store.core(core) else { return };
        self.clipboard = Some(tree_ops::copy_folder(&cd.strategies, &path));
        cx.notify();
    }

    pub(super) fn paste_into(&mut self, core: CoreId, target: String, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        let specs = {
            let store = self.backend.read(cx).session.store();
            let taken: std::collections::HashSet<String> = store
                .core(core)
                .map(|cd| cd.strategies.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default();
            let plan = tree_ops::paste_plan(&clip, &tree_ops::split_path(&target), &taken);
            specs_from(plan)
        };
        // Имя первой вставленной — выберем её, как только ядро пришлёт эхо.
        let first_name = specs.first().and_then(|s| {
            s.fields
                .iter()
                .find(|(n, _)| n == tree_ops::STRATEGY_NAME_FIELD)
                .map(|(_, v)| v.clone())
        });
        self.backend.read(cx).session.create_strategies(core, specs);
        // Новые/вставленные стратегии всегда выключены — снимаем «только активные», иначе их
        // не видно. Раскрываем целевое ядро, чтобы результат был на виду.
        self.filter.only_active = false;
        self.expanded_cores.insert(core);
        self.pending_select = first_name.map(|n| (core, n));
        cx.notify();
    }

    // ── Drag & Drop ───────────────────────────────────────────────────────────

    /// Сбросить перетаскиваемые СТРАТЕГИИ в целевую папку (`target` пуст = корень ядра).
    /// В пределах ядра — перенос (`move_strategies`); между ядрами — копирование.
    pub(super) fn drop_strategies(
        &mut self,
        target_core: CoreId,
        target: Vec<String>,
        drag: &StratDrag,
        cx: &mut Context<Self>,
    ) {
        let ids = drag.ids.clone();
        if ids.is_empty() {
            return;
        }
        if drag.core == target_core {
            let moves = {
                let store = self.backend.read(cx).session.store();
                let rows: Vec<&StrategyRow> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().filter(|r| ids.contains(&r.id)).collect())
                    .unwrap_or_default();
                tree_ops::move_to(&rows, &target)
            };
            self.backend.read(cx).session.move_strategies(target_core, moves);
        } else {
            let specs = {
                let store = self.backend.read(cx).session.store();
                let rows: Vec<&StrategyRow> = store
                    .core(drag.core)
                    .map(|c| c.strategies.iter().filter(|r| ids.contains(&r.id)).collect())
                    .unwrap_or_default();
                let clip = tree_ops::copy_rows(&rows);
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                specs_from(tree_ops::paste_plan(&clip, &target, &taken))
            };
            self.backend.read(cx).session.create_strategies(target_core, specs);
            self.filter.only_active = false;
        }
        self.expanded_cores.insert(target_core);
        cx.notify();
    }

    /// Сбросить перетаскиваемую ПАПКУ в целевую папку-родитель (`target` пуст = корень).
    /// В пределах ядра — перенос поддерева; между ядрами — копирование.
    pub(super) fn drop_folder(
        &mut self,
        target_core: CoreId,
        target: Vec<String>,
        drag: &FolderDrag,
        cx: &mut Context<Self>,
    ) {
        let path = drag.path.clone();
        if drag.core == target_core {
            let moves = {
                let store = self.backend.read(cx).session.store();
                store
                    .core(target_core)
                    .map(|c| tree_ops::move_folder(&c.strategies, &path, &target))
                    .unwrap_or_default()
            };
            if moves.is_empty() {
                return; // в себя/потомка или пустая папка
            }
            self.backend.read(cx).session.move_strategies(target_core, moves);
        } else {
            let specs = {
                let store = self.backend.read(cx).session.store();
                let clip = store
                    .core(drag.core)
                    .map(|c| tree_ops::copy_folder(&c.strategies, &path))
                    .unwrap_or_default();
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                specs_from(tree_ops::paste_plan(&clip, &target, &taken))
            };
            self.backend.read(cx).session.create_strategies(target_core, specs);
            self.filter.only_active = false;
        }
        self.expanded_cores.insert(target_core);
        cx.notify();
    }

    /// Список id для перетаскивания стратегии: весь мультивыбор этого ядра, если строка в
    /// выборе; иначе только она.
    pub(super) fn drag_ids_for(&self, core: CoreId, id: u64) -> Vec<u64> {
        if self.sel.contains(&(core, id)) {
            self.sel
                .iter()
                .filter(|(c, _)| *c == core)
                .map(|(_, i)| *i)
                .collect()
        } else {
            vec![id]
        }
    }

    // ── Подтверждённый диспетч ────────────────────────────────────────────────

    fn confirm_create_strategy(
        &mut self,
        core: CoreId,
        target: String,
        kind_ord: u8,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let spec = {
            let store = self.backend.read(cx).session.store();
            let Some(kind) = store
                .core(core)
                .and_then(|cd| cd.schema.as_ref())
                .and_then(|s| s.kinds.iter().find(|k| k.ordinal == kind_ord).cloned())
            else {
                return;
            };
            let ns = tree_ops::new_strategy(&kind, &name, &target);
            NewStrategySpec {
                kind_ordinal: ns.kind_ordinal,
                folder_path: ns.folder_path,
                fields: ns.fields,
            }
        };
        self.backend
            .read(cx)
            .session
            .create_strategies(core, vec![spec]);
        // Новая стратегия выключена — снимаем «только активные» и раскрываем ядро, чтобы её видеть.
        self.filter.only_active = false;
        self.expanded_cores.insert(core);
        // Выберем её, как только ядро пришлёт эхо.
        self.pending_select = Some((core, name));
    }

    fn confirm_rename_folder(
        &mut self,
        core: CoreId,
        old_path: &[String],
        new_name: &str,
        cx: &mut Context<Self>,
    ) {
        let moves = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else { return };
            tree_ops::rename_folder(&cd.strategies, old_path, new_name)
        };
        // UI-папка (пустая) — переименовать локально.
        self.rename_ui_folder(core, old_path, new_name);
        self.backend.read(cx).session.move_strategies(core, moves);
    }

    /// Удалить выделение (группировка по ядрам; правило уже проверено в request_).
    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let rows = {
            let store = self.backend.read(cx).session.store();
            self.selection_rows(store)
        };
        {
            let b = self.backend.read(cx);
            for (core, r) in &rows {
                b.session.delete_strategy(*core, r.id);
            }
        }
        self.sel.clear();
        self.selected = None;
    }

    fn delete_folder(&mut self, core: CoreId, path: &[String], cx: &mut Context<Self>) {
        self.backend
            .read(cx)
            .session
            .delete_folder(core, tree_ops::join_path(path));
        self.remove_ui_folder(core, path);
    }

    // ── UI-папки (пустые, до наполнения) ──────────────────────────────────────

    fn add_ui_folder(&mut self, core: CoreId, parent: &str, name: &str) {
        let mut parts = tree_ops::split_path(parent);
        parts.push(name.to_string());
        self.ui_folders.insert((core, tree_ops::join_path(&parts)));
        // Раскрыть ядро и родительскую цепочку (все сегменты, кроме новой папки), чтобы она
        // была сразу видна.
        self.expanded_cores.insert(core);
        let ancestors = parts.len().saturating_sub(1);
        self.expand_path(core, parts.iter().take(ancestors).map(String::as_str));
    }

    fn remove_ui_folder(&mut self, core: CoreId, path: &[String]) {
        let key = tree_ops::join_path(path);
        self.ui_folders
            .retain(|(c, p)| !(*c == core && (p == &key || p.starts_with(&format!("{key}/")))));
    }

    fn rename_ui_folder(&mut self, core: CoreId, old_path: &[String], new_name: &str) {
        if old_path.is_empty() {
            return;
        }
        let old_key = tree_ops::join_path(old_path);
        let mut np = old_path.to_vec();
        *np.last_mut().unwrap() = new_name.to_string();
        let new_key = tree_ops::join_path(&np);
        let affected: Vec<String> = self
            .ui_folders
            .iter()
            .filter(|(c, p)| *c == core && (p == &old_key || p.starts_with(&format!("{old_key}/"))))
            .map(|(_, p)| p.clone())
            .collect();
        for p in affected {
            self.ui_folders.remove(&(core, p.clone()));
            let rebased = p.replacen(&old_key, &new_key, 1);
            self.ui_folders.insert((core, rebased));
        }
    }

    /// Пустые UI-папки данного ядра (сегменты пути) — для подмешивания в дерево.
    pub(super) fn ui_folder_paths(&self, core: CoreId) -> Vec<Vec<String>> {
        self.ui_folders
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, p)| tree_ops::split_path(p))
            .collect()
    }

    // ── Клавиатура (Ctrl+C / Ctrl+V / Delete) ────────────────────────────────

    pub(super) fn handle_tree_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if m.control && key == "c" {
            self.copy_selection(cx);
        } else if m.control && key == "v" {
            let (core, target) = {
                let store = self.backend.read(cx).session.store();
                let cores: Vec<(CoreId, String)> = self
                    .backend
                    .read(cx)
                    .session
                    .sessions()
                    .iter()
                    .map(|s| (s.id, s.name.clone()))
                    .collect();
                self.default_target(store, &cores)
            };
            self.paste_into(core, target, cx);
        } else if key == "delete" {
            self.request_delete_selection(cx);
        }
    }

    // ── Контекст-меню (ПКМ) ───────────────────────────────────────────────────

    pub(super) fn open_menu(&mut self, menu: ContextMenu, cx: &mut Context<Self>) {
        self.op = None;
        self.menu = Some(menu);
        cx.notify();
    }

    // ── Рендер: тулбар выделения ──────────────────────────────────────────────

    /// Кнопки операций над выделением/буфером (в нижней панели действий).
    pub(super) fn selection_toolbar(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let rows = self.selection_rows(store);
        let has_sel = !rows.is_empty();
        let all_off = rows.iter().all(|(_, r)| !r.checked);
        let can_paste = self.clipboard.is_some();
        // Левая группа фикс. ширины: ряд [копировать][вставить], под ними [удалить] по центру
        // во всю ширину (MoonButton не тянется — центрируем в своих слотах).
        v_flex()
            .w(px(176.0))
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div().flex_1().flex().justify_center().child(
                            MoonButton::new("sel-copy")
                                .outline()
                                .size(MoonButtonSize::Micro)
                                .label("копировать")
                                .disabled(!has_sel)
                                .on_click(cx.listener(|this, _, _, cx| this.copy_selection(cx)))
                                .render(),
                        ),
                    )
                    .child(
                        div().flex_1().flex().justify_center().child(
                            MoonButton::new("sel-paste")
                                .outline()
                                .size(MoonButtonSize::Micro)
                                .label("вставить")
                                .disabled(!can_paste)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // вставка в папку первичной стратегии (или корень).
                                    let (core, target) = {
                                        let store = this.backend.read(cx).session.store();
                                        let cores: Vec<(CoreId, String)> = this
                                            .backend
                                            .read(cx)
                                            .session
                                            .sessions()
                                            .iter()
                                            .map(|s| (s.id, s.name.clone()))
                                            .collect();
                                        this.default_target(store, &cores)
                                    };
                                    this.paste_into(core, target, cx);
                                }))
                                .render(),
                        ),
                    ),
            )
            .child(
                div().w_full().flex().justify_center().child(
                    MoonButton::new("sel-delete")
                        .danger()
                        .size(MoonButtonSize::Micro)
                        .label("удалить")
                        .disabled(!has_sel || !all_off)
                        .on_click(cx.listener(|this, _, _, cx| this.request_delete_selection(cx)))
                        .render(),
                ),
            )
            .into_any_element()
    }

    /// Кнопка «＋ Создать» (дропдаун: стратегия/папка) для шапки дерева.
    pub(super) fn create_dropdown(
        &self,
        core: CoreId,
        target: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let t1 = target.clone();
        let items = vec![
            MoonMenuItem::with_key("new-strat", "Новая стратегия…").on_click({
                let view = view.clone();
                move |_, _, app| {
                    let (core, t) = (core, t1.clone());
                    view.update(app, |this, c| this.open_create_strategy(core, t, c));
                }
            }),
            MoonMenuItem::with_key("new-folder", "Новая папка…").on_click({
                let view = view.clone();
                let t2 = target.clone();
                move |_, _, app| {
                    let (core, t) = (core, t2.clone());
                    view.update(app, |this, c| this.open_create_folder(core, t, c));
                }
            }),
        ];
        MoonDropdown::new("strat-create")
            .label("＋ Создать ▾")
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(110.0)
            .menu_width(180.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }

    // ── Рендер: оверлеи модалок ──────────────────────────────────────────────

    pub(super) fn op_overlay(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let op = self.op.as_ref()?;
        let p = MoonPalette::active(cx);
        let body = match op {
            TreeOp::CreateStrategy { core, target, kind } => {
                self.modal_create_strategy(*core, target, *kind, &p, cx)
            }
            TreeOp::CreateFolder { core, target } => {
                self.modal_create_folder(*core, target, &p, cx)
            }
            TreeOp::RenameFolder { core, old_path } => {
                self.modal_rename_folder(*core, old_path.clone(), &p, cx)
            }
            TreeOp::ConfirmDeleteStrategies { label, .. }
            | TreeOp::ConfirmDeleteFolder { label, .. } => self.modal_confirm_delete(label, &p, cx),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000073))
                .child(body)
                .into_any_element(),
        )
    }

    fn modal_shell(&self, title: &str, p: &MoonPalette, cx: &Context<Self>) -> Div {
        v_flex()
            .w(px(360.0))
            .bg(moon(p.shell_high))
            .border_1()
            .border_color(moon(p.border))
            .rounded(design::ui_px(cx, 6.0))
            .child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
    }

    fn modal_create_strategy(
        &self,
        core: CoreId,
        target: &str,
        kind: Option<u8>,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let store = self.backend.read(cx).session.store();
        let kinds = self.kinds_of(store, core);
        let kind_name = kind
            .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "выберите вид".to_string());
        let target_label = if target.is_empty() {
            "корень".to_string()
        } else {
            target.to_string()
        };
        let view = cx.entity();
        let mut kind_items = Vec::with_capacity(kinds.len());
        for (ord, name) in &kinds {
            let view = view.clone();
            let ord = *ord;
            kind_items.push(
                MoonMenuItem::with_key(format!("ck-{ord}"), name.clone())
                    .selected(kind == Some(ord))
                    .on_click(move |_, _, app| {
                        view.update(app, |this, c| {
                            if let Some(TreeOp::CreateStrategy { kind, .. }) = &mut this.op {
                                *kind = Some(ord);
                                c.notify();
                            }
                        });
                    }),
            );
        }
        self.modal_shell("Новая стратегия", p, cx)
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .child(div().text_color(moon(p.text_muted)).child(format!("папка: {target_label}")))
                    .child(
                        MoonDropdown::new("create-kind")
                            .label(format!("{kind_name} ▾"))
                            .trigger_variant(MoonButtonVariant::Soft)
                            .trigger_size(MoonButtonSize::Action)
                            .trigger_width(320.0)
                            .menu_width(320.0)
                            .menu_size(MoonMenuSize::Compact)
                            .menu_max_height(240.0)
                            .items(kind_items),
                    )
                    .children(
                        self.op_input
                            .as_ref()
                            .map(|inp| MoonInput::new("create-name").state(inp).small()),
                    ),
            )
            .child(self.modal_buttons(
                "Создать",
                move |this, _, _, cx| {
                    let name = this
                        .op_input
                        .as_ref()
                        .map(|i| i.read(cx).value().to_string())
                        .unwrap_or_default();
                    let (target, kind) = match &this.op {
                        Some(TreeOp::CreateStrategy { target, kind, .. }) => {
                            (target.clone(), *kind)
                        }
                        _ => return,
                    };
                    if name.trim().is_empty() {
                        return;
                    }
                    if let Some(k) = kind {
                        this.confirm_create_strategy(core, target, k, name, cx);
                    }
                    this.op = None;
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }

    fn modal_create_folder(
        &self,
        core: CoreId,
        target: &str,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let target_label = if target.is_empty() {
            "корень".to_string()
        } else {
            target.to_string()
        };
        let target_owned = target.to_string();
        self.modal_shell("Новая папка", p, cx)
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .child(div().text_color(moon(p.text_muted)).child(format!("в: {target_label}")))
                    .children(
                        self.op_input
                            .as_ref()
                            .map(|inp| MoonInput::new("folder-name").state(inp).small()),
                    ),
            )
            .child(self.modal_buttons(
                "Создать",
                move |this, _, _, cx| {
                    let name = this
                        .op_input
                        .as_ref()
                        .map(|i| i.read(cx).value().to_string())
                        .unwrap_or_default();
                    if name.trim().is_empty() {
                        this.op = None;
                        cx.notify();
                        return;
                    }
                    this.add_ui_folder(core, &target_owned, name.trim());
                    this.op = None;
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }

    fn modal_rename_folder(
        &self,
        core: CoreId,
        old_path: Vec<String>,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.modal_shell("Переименовать папку", p, cx)
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .children(
                        self.op_input
                            .as_ref()
                            .map(|inp| MoonInput::new("rename-name").state(inp).small()),
                    ),
            )
            .child(self.modal_buttons(
                "Переименовать",
                move |this, _, _, cx| {
                    let name = this
                        .op_input
                        .as_ref()
                        .map(|i| i.read(cx).value().to_string())
                        .unwrap_or_default();
                    if !name.trim().is_empty() {
                        this.confirm_rename_folder(core, &old_path, name.trim(), cx);
                    }
                    this.op = None;
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }

    fn modal_confirm_delete(
        &self,
        label: &str,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.modal_shell("Удалить?", p, cx)
            .child(
                div()
                    .w_full()
                    .p_3()
                    .text_color(moon(p.text))
                    .child(format!("Удалить {label}? Действие необратимо.")),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(
                        MoonButton::new("confirm-no")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .label("Нет")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.op = None;
                                cx.notify();
                            }))
                            .render(),
                    )
                    .child(
                        MoonButton::new("confirm-yes")
                            .danger()
                            .size(MoonButtonSize::Micro)
                            .label("Да")
                            .on_click(cx.listener(|this, _, _, cx| {
                                match this.op.take() {
                                    Some(TreeOp::ConfirmDeleteStrategies { .. }) => {
                                        this.delete_selection(cx)
                                    }
                                    Some(TreeOp::ConfirmDeleteFolder { core, path, .. }) => {
                                        this.delete_folder(core, &path, cx)
                                    }
                                    _ => {}
                                }
                                cx.notify();
                            }))
                            .render(),
                    ),
            )
            .into_any_element()
    }

    fn modal_buttons(
        &self,
        ok_label: &str,
        on_ok: impl Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        h_flex()
            .w_full()
            .justify_end()
            .gap_2()
            .px_3()
            .pb_3()
            .child(
                MoonButton::new("modal-cancel")
                    .ghost()
                    .size(MoonButtonSize::Micro)
                    .label("Отмена")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.op = None;
                        cx.notify();
                    }))
                    .render(),
            )
            .child(
                MoonButton::new("modal-ok")
                    .primary()
                    .size(MoonButtonSize::Micro)
                    .label(ok_label.to_string())
                    .on_click(cx.listener(on_ok))
                    .render(),
            )
            .into_any_element()
    }

    // ── Рендер: контекст-меню ────────────────────────────────────────────────

    pub(super) fn menu_overlay(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let p = MoonPalette::active(cx);
        let core = menu.core;
        let can_paste = self.clipboard.is_some();

        let mut items: Vec<(SharedString, Box<dyn Fn(&mut Self, &mut Context<Self>)>)> = Vec::new();
        match &menu.target {
            MenuTarget::Folder(path) => {
                let pp = path.clone();
                items.push((
                    "Переименовать…".into(),
                    Box::new(move |this, cx| this.open_rename_folder(core, pp.clone(), cx)),
                ));
                let pp = path.clone();
                items.push((
                    "Копировать".into(),
                    Box::new(move |this, cx| this.copy_folder(core, pp.clone(), cx)),
                ));
                if can_paste {
                    let t = tree_ops::join_path(path);
                    items.push((
                        "Вставить сюда".into(),
                        Box::new(move |this, cx| this.paste_into(core, t.clone(), cx)),
                    ));
                }
                let t = tree_ops::join_path(path);
                items.push((
                    "Новая стратегия здесь…".into(),
                    Box::new(move |this, cx| this.open_create_strategy(core, t.clone(), cx)),
                ));
                let t = tree_ops::join_path(path);
                items.push((
                    "Новая папка здесь…".into(),
                    Box::new(move |this, cx| this.open_create_folder(core, t.clone(), cx)),
                ));
                let pp = path.clone();
                items.push((
                    "Удалить папку…".into(),
                    Box::new(move |this, cx| this.request_delete_folder(core, pp.clone(), cx)),
                ));
            }
            MenuTarget::Strategy(_id) => {
                items.push((
                    "Копировать".into(),
                    Box::new(move |this, cx| this.copy_selection(cx)),
                ));
                items.push((
                    "Удалить…".into(),
                    Box::new(move |this, cx| this.request_delete_selection(cx)),
                ));
            }
        }

        let mut list = v_flex()
            .absolute()
            .left(menu.pos.x)
            .top(menu.pos.y)
            .w(px(190.0))
            .bg(moon(p.shell_high))
            .border_1()
            .border_color(moon(p.border))
            .rounded(design::ui_px(cx, 5.0))
            .py(px(3.0));
        for (i, (label, action)) in items.into_iter().enumerate() {
            list = list.child(
                div()
                    .id(SharedString::from(format!("menu-{i}")))
                    .w_full()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .text_color(moon(p.text))
                    .hover(move |s| s.bg(moon_alpha(p.panel, 0.8)))
                    .child(label)
                    // mouse_down + stop_propagation: действие срабатывает на нажатии (до
                    // закрытия меню фоном), и клик НЕ проваливается на дерево позади.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            action(this, cx);
                            this.menu = None;
                            cx.notify();
                        }),
                    ),
            );
        }

        let close = |this: &mut Self, cx: &mut Context<Self>| {
            this.menu = None;
            cx.notify();
        };
        Some(
            div()
                .absolute()
                .inset_0()
                // фон-перехватчик: клик мимо — закрыть меню; stop_propagation, чтобы клик
                // не дошёл до дерева (иначе сворачивались бы ветки за меню).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _e: &MouseDownEvent, _w, cx| {
                        cx.stop_propagation();
                        close(this, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _e: &MouseDownEvent, _w, cx| {
                        cx.stop_propagation();
                        close(this, cx);
                    }),
                )
                .child(list)
                .into_any_element(),
        )
    }
}

/// Преобразовать план вставки/создания в команды ядра.
fn specs_from(plan: Vec<tree_ops::NewStrategy>) -> Vec<NewStrategySpec> {
    plan.into_iter()
        .map(|n| NewStrategySpec {
            kind_ordinal: n.kind_ordinal,
            folder_path: n.folder_path,
            fields: n.fields,
        })
        .collect()
}
