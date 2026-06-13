//! Панель «Лог» — порт egui `src/dock/log_panel.rs`. Просмотр лога с выбором
//! источника, файла, поиском и фильтром «только ошибки».
//!
//! Источники: «Лог группы» (агрегат живых логов ядер группы), «Локальный» (лог
//! приложения, `applog`-кольцо) и каждое ядро (его серверный лог, кольцо в `CoreData.log`).
//! Для одного ядра/локального можно смотреть Live (текущий) ИЛИ файл с диска
//! (`logs/<дата>_<источник>.log`); агрегат — только Live. Список виртуализирован
//! (`ListState` с выравниванием к низу — как chat-лог, новые строки видны снизу).

use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dock::{Panel, PanelEvent, PanelState, PanelView, TabPanel},
    h_flex,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    v_flex, Sizable, StyledExt,
};

use crate::detached::DetachedSpec;
use crate::{hex, Backend};
use moon_core::applog::{self, LogLine};
use moon_core::palette;
use moon_core::session::{CoreId, CoreStore};

/// Сколько последних строк держим в поле зрения.
const VIEW_LIMIT: usize = 5000;
/// Сколько строк берём с каждого ядра при сборке агрегата.
const AGG_PER_CORE: usize = 2000;

/// Источник лога.
#[derive(Clone, PartialEq)]
enum LogSource {
    Aggregate,
    Local,
    Core(CoreId),
}

/// Что показываем: живой лог из памяти или файл с диска.
#[derive(Clone, PartialEq)]
enum LogFile {
    Live,
    Named(String),
}

/// Один пункт селектора источника.
struct LogSourceItem {
    source: LogSource,
    display: String,
    file_label: String,
}

pub struct LogPanel {
    backend: Entity<Backend>,
    group: String,
    source: LogSource,
    file: LogFile,
    errors_only: bool,
    query: Entity<InputState>,
    /// Кэш загруженного файла — чтобы не читать диск каждый кадр.
    loaded_name: Option<String>,
    loaded_lines: Vec<LogLine>,
    /// Отфильтрованные строки текущего кадра (читает рендер списка по индексу).
    lines: Vec<LogLine>,
    list: ListState,
    /// Сигнатура лога прошлого кадра — чтобы НЕ пересобирать лог каждые 100мс
    /// (gather клонирует до 5000 строк; на холостом ходу это лишняя нагрузка).
    last_sig: u64,
    tab: Option<WeakEntity<TabPanel>>,
    focus: FocusHandle,
}

impl LogPanel {
    pub fn new(backend: Entity<Backend>, group: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск…"));
        cx.subscribe(&query, |_t, _e, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        // Перерисовка — ТОЛЬКО когда реально появились новые строки лога.
        cx.observe(&backend, |this, backend, cx| {
            let sig = log_sig(backend.read(cx), &this.group);
            if sig != this.last_sig {
                this.last_sig = sig;
                cx.notify();
            }
        })
        .detach();
        Self {
            backend,
            group,
            source: LogSource::Aggregate,
            file: LogFile::Live,
            errors_only: true,
            query,
            loaded_name: None,
            loaded_lines: Vec::new(),
            lines: Vec::new(),
            list: ListState::new(0, ListAlignment::Bottom, px(200.0)),
            last_sig: 0,
            tab: None,
            focus: cx.focus_handle(),
        }
    }

    /// Список источников в области видимости (порт `App::build_log_sources`).
    /// Группа непуста → только её ядра (агрегат = «Лог группы»); пусто → все (детач).
    fn sources(&self, b: &Backend) -> Vec<LogSourceItem> {
        let scoped = !self.group.is_empty();
        let mut v = vec![
            LogSourceItem {
                source: LogSource::Aggregate,
                display: if scoped { "Лог группы".into() } else { "Все ядра".into() },
                file_label: String::new(),
            },
            LogSourceItem { source: LogSource::Local, display: "Локальный".into(), file_label: "app".into() },
        ];
        for s in &b.config.servers {
            if scoped && s.group != self.group {
                continue;
            }
            v.push(LogSourceItem {
                source: LogSource::Core(s.id),
                display: s.name.clone(),
                file_label: applog::sanitize_label(&s.name),
            });
        }
        v
    }

    fn file_label(&self, sources: &[LogSourceItem]) -> String {
        sources.iter().find(|s| s.source == self.source).map(|s| s.file_label.clone()).unwrap_or_else(|| "app".into())
    }

    /// Строки для текущего выбора (Live — из памяти/агрегат слиянием; Named — из файла).
    fn gather(&mut self, store: &CoreStore, sources: &[LogSourceItem]) -> Vec<LogLine> {
        match &self.file {
            LogFile::Live => {
                self.loaded_name = None;
                match &self.source {
                    LogSource::Local => applog::snapshot(VIEW_LIMIT),
                    LogSource::Core(id) => store.core(*id).map(|c| c.log_snapshot(VIEW_LIMIT)).unwrap_or_default(),
                    LogSource::Aggregate => aggregate(store, sources),
                }
            }
            LogFile::Named(name) => {
                if self.loaded_name.as_deref() != Some(name.as_str()) {
                    self.loaded_lines = applog::read_file(name, VIEW_LIMIT);
                    self.loaded_name = Some(name.clone());
                }
                self.loaded_lines.clone()
            }
        }
    }

    fn set_source(&mut self, s: LogSource, cx: &mut Context<Self>) {
        if self.source != s {
            self.source = s;
            // Смена источника → к Live, сброс кэша файла.
            self.file = LogFile::Live;
            self.loaded_name = None;
            cx.notify();
        }
    }
    fn set_file(&mut self, f: LogFile, cx: &mut Context<Self>) {
        self.file = f;
        cx.notify();
    }

    /// Комбобокс источника.
    fn source_combo(&self, sources: &[LogSourceItem], cx: &Context<Self>) -> impl IntoElement {
        let cur = sources.iter().find(|s| s.source == self.source).map(|s| s.display.clone()).unwrap_or_else(|| "Локальный".into());
        let view = cx.entity();
        let items: Vec<(LogSource, String)> = sources.iter().map(|s| (s.source.clone(), s.display.clone())).collect();
        Popover::new("log-source")
            .trigger(Button::new("log-source-btn").outline().xsmall().label(format!("{cur} ▾")))
            .content(move |_s, _w, _cx| {
                let mut col = v_flex().gap_0p5().p_1().min_w(px(160.0));
                for (i, (src, disp)) in items.iter().enumerate() {
                    let src = src.clone();
                    let view = view.clone();
                    col = col.child(combo_item(format!("ls-{i}"), disp.clone(), move |app| {
                        view.update(app, |t, c| t.set_source(src.clone(), c))
                    }));
                }
                col
            })
    }

    /// Комбобокс файла (Live + прошлые файлы) — только для одиночного источника.
    fn file_combo(&self, sources: &[LogSourceItem], cx: &Context<Self>) -> impl IntoElement {
        let cur = match &self.file {
            LogFile::Live => "Live (текущий)".to_string(),
            LogFile::Named(n) => n.clone(),
        };
        let label = self.file_label(sources);
        let view = cx.entity();
        Popover::new("log-file")
            .trigger(Button::new("log-file-btn").outline().xsmall().label(format!("{cur} ▾")))
            .content(move |_s, _w, _cx| {
                let files = applog::list_files(&label);
                let mut col = v_flex().id("log-file-list").gap_0p5().p_1().min_w(px(180.0)).max_h(px(360.0)).overflow_y_scroll();
                col = col.child(combo_item("lf-live", "Live (текущий)", {
                    let view = view.clone();
                    move |app| view.update(app, |t, c| t.set_file(LogFile::Live, c))
                }));
                for f in files {
                    let view = view.clone();
                    let f2 = f.clone();
                    col = col.child(combo_item(SharedString::from(format!("lf-{f}")), f.clone(), move |app| {
                        let f3 = f2.clone();
                        view.update(app, |t, c| t.set_file(LogFile::Named(f3.clone()), c))
                    }));
                }
                col
            })
    }
}

/// Сигнатура лога: ревизия кольца applog + сумма log_rev ядер группы. Растёт при
/// любой новой строке (локальной или ядра). Не сменилась → пересобирать не нужно.
fn log_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    let scoped = !group.is_empty();
    let cores: u64 = b
        .session
        .sessions()
        .iter()
        .filter(|s| !scoped || s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.log_rev));
    applog::revision().wrapping_add(cores)
}

/// Слияние живых логов всех ядер области по времени (ts лексикографичен = хронологичен).
fn aggregate(store: &CoreStore, sources: &[LogSourceItem]) -> Vec<LogLine> {
    let mut merged: Vec<LogLine> = Vec::new();
    for item in sources {
        if let LogSource::Core(id) = item.source {
            if let Some(c) = store.core(id) {
                for mut l in c.log_snapshot(AGG_PER_CORE) {
                    l.target = item.display.clone();
                    merged.push(l);
                }
            }
        }
    }
    merged.sort_by(|a, b| a.ts.cmp(&b.ts));
    if merged.len() > VIEW_LIMIT {
        let drop = merged.len() - VIEW_LIMIT;
        merged.drain(0..drop);
    }
    merged
}

/// Бейдж уровня + цвет (палитра).
fn level_tag(level: log::Level) -> Option<(&'static str, u32)> {
    match level {
        log::Level::Error => Some(("ERR", hex(palette::RED))),
        log::Level::Warn => Some(("WARN", hex(palette::ACCENT))),
        _ => None,
    }
}

/// Рендер одной строки лога (время · [уровень] · источник · сообщение).
fn log_row(line: &LogLine) -> AnyElement {
    let time = line.ts.rsplit(' ').next().unwrap_or(line.ts.as_str()).to_string();
    let flat = line.msg.replace('\n', " ⏎ ");
    let mut row = h_flex().w_full().gap_1().items_baseline().text_xs().px_1();
    row = row.child(div().flex_none().text_color(rgb(hex(palette::TEXT_2))).child(time));
    if let Some((tag, col)) = level_tag(line.level) {
        row = row.child(div().flex_none().font_bold().text_color(rgb(col)).child(tag));
    }
    if !line.target.is_empty() {
        row = row.child(div().flex_none().text_color(rgb(hex(palette::TEXT_2))).child(line.target.clone()));
    }
    row.child(div().flex_1().min_w_0().text_color(rgb(hex(palette::TEXT_2))).child(flat)).into_any_element()
}

impl EventEmitter<PanelEvent> for LogPanel {}
impl Focusable for LogPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for LogPanel {
    fn panel_name(&self) -> &'static str {
        "Log"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Лог")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Log", &self.group)
    }
    fn on_added_to(&mut self, tab_panel: WeakEntity<TabPanel>, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab = Some(tab_panel);
    }
    fn toolbar_buttons(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let backend = self.backend.clone();
        let group = self.group.clone();
        let tab = self.tab.clone();
        let me = cx.entity().downgrade();
        Some(vec![Button::new("detach-log")
            .ghost()
            .label("⧉")
            .tooltip("В отдельное окно")
            .on_click(move |_, window, app| {
                if let (Some(tab), Some(me)) = (tab.as_ref().and_then(|t| t.upgrade()), me.upgrade()) {
                    let arc: Arc<dyn PanelView> = Arc::new(me);
                    tab.update(app, |tp, cx| tp.remove_panel(arc, window, cx));
                }
                let spec = DetachedSpec::new(group.clone(), "Log".to_string());
                crate::detached::spawn(app, &backend, &spec);
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            })])
    }
}

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let errors_only = self.errors_only;

        let sources = self.sources(self.backend.read(cx));
        // gather читает store (живой) — store берём из backend (tied to cx).
        let gathered = {
            let store = self.backend.read(cx).session.store();
            // store-borrow tied to cx; gather мутирует self (кэш) — disjoint поля.
            self.gather(store, &sources)
        };
        let total = gathered.len();
        let filtered: Vec<LogLine> = gathered
            .into_iter()
            .filter(|l| !errors_only || l.is_errorish())
            .filter(|l| query.is_empty() || l.msg.to_lowercase().contains(&query))
            .collect();

        // Обновить виртуализированный список (сброс счётчика при изменении длины).
        self.lines = filtered;
        if self.list.item_count() != self.lines.len() {
            self.list.reset(self.lines.len());
        }

        let is_agg = matches!(self.source, LogSource::Aggregate);

        // ── Панель управления ──
        let mut controls = h_flex().w_full().flex_wrap().gap_2().items_center().px_2().py_1();
        controls = controls.child(self.source_combo(&sources, cx));
        if !is_agg {
            controls = controls
                .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child("Файл"))
                .child(self.file_combo(&sources, cx));
        }
        controls = controls
            .child(div().w(px(180.0)).child(Input::new(&self.query).small().cleanable(true)))
            .child(
                Checkbox::new("log-errors-only")
                    .label("Только ошибки")
                    .checked(self.errors_only)
                    .on_click(cx.listener(|t, ch: &bool, _, cx| {
                        t.errors_only = *ch;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(hex(palette::TEXT_3)))
                    .child(format!("{} из {}", self.lines.len(), total)),
            );

        // ── Список (виртуализирован, к низу) ──
        let weak = cx.entity().downgrade();
        let body: AnyElement = if self.lines.is_empty() {
            let msg = if total == 0 { "Лог пуст" } else { "Нет строк по фильтру" };
            div().flex_1().w_full().flex().items_center().justify_center().text_color(rgb(hex(palette::TEXT_2))).child(msg).into_any_element()
        } else {
            let list_el = list(self.list.clone(), move |ix, _w, app| {
                weak.upgrade()
                    .and_then(|e| e.read(app).lines.get(ix).map(log_row))
                    .unwrap_or_else(|| div().into_any_element())
            })
            .size_full();
            div().flex_1().w_full().min_h_0().child(list_el).into_any_element()
        };

        v_flex()
            .id("log-panel")
            .size_full()
            .track_focus(&self.focus)
            .child(controls)
            .child(div().w_full().h(px(1.0)).bg(rgb(hex(palette::LIFT_HOVER))))
            .child(body)
    }
}

/// Кликабельный пункт попап-комбобокса.
fn combo_item(id: impl Into<SharedString>, label: impl Into<SharedString>, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    div()
        .id(id.into())
        .w_full()
        .px_2()
        .py_1()
        .cursor_pointer()
        .rounded(px(3.0))
        .text_color(rgb(hex(palette::TEXT)))
        .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
        .child(label.into())
        .on_click(move |_, _w, app| on_click(app))
}
