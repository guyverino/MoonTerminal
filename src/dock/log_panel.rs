//! Вкладка «Лог»: просмотр лога с выбором источника, файла, поиском и фильтром.
//!
//! Источники: «Лог группы» (агрегат живых логов всех ядер в области видимости — по
//! умолчанию), «Локальный» (лог приложения, in-memory кольцо `applog`) и каждое ядро
//! по отдельности (его серверный лог, кольцо в `CoreData.log`). Для одного ядра/
//! локального можно смотреть `Live` (текущий пишущийся лог) ИЛИ любой прошлый файл с
//! диска (`logs/<дата>_<источник>.log`); агрегат — только Live.
//!
//! Область видимости задаётся СПИСКОМ источников, который строит вызывающий: в доке
//! окна группы — только ядра этой группы (агрегат = «Лог группы»); в откреплённом окне
//! — все ядра (агрегат = «Все ядра»). Состояние ([`LogPanelState`]) для дока живёт в
//! `Dock` (СВОЁ у каждого окна → разные окна показывают разный лог), для откреплённого
//! окна — одно в App (сбрасывается к дефолту при закрытии окна).

use crate::applog::LogLine;
use crate::session::{CoreId, CoreStore};
use crate::shell::theme;

/// Сколько последних строк держим в поле зрения (живой буфер/файл в памяти больше).
const VIEW_LIMIT: usize = 5000;
/// Сколько строк берём с каждого ядра при сборке агрегата (чтобы не сортировать
/// десятки тысяч строк каждый кадр; итог всё равно режется до VIEW_LIMIT).
const AGG_PER_CORE: usize = 2000;

/// Размер моноширинного шрифта строк лога (фикс — для ровной высоты `show_rows`).
const FONT_PX: f32 = 11.0;

/// Источник лога.
#[derive(Clone, PartialEq)]
pub enum LogSource {
    /// Агрегат живых логов всех ядер области видимости («Лог группы» / «Все ядра»).
    Aggregate,
    /// Локальный лог приложения.
    Local,
    /// Конкретное ядро.
    Core(CoreId),
}

/// Что показываем для выбранного источника: живой лог из памяти или файл с диска.
#[derive(Clone, PartialEq)]
pub enum LogFile {
    /// Текущий пишущийся лог (in-memory кольцо).
    Live,
    /// Конкретный файл logs/<имя> (например прошлый день).
    Named(String),
}

/// Один пункт селектора источника. `file_label` — метка файла на диске (`app` для
/// локального, очищенное имя ядра — для ядра; для агрегата не используется), по ней
/// ищем прошлые файлы источника.
pub struct LogSourceItem {
    pub source: LogSource,
    pub display: String,
    pub file_label: String,
}

/// Состояние лог-панели. Кэш загруженного файла — чтобы не читать диск каждый кадр.
pub struct LogPanelState {
    pub source: LogSource,
    pub file: LogFile,
    pub query: String,
    pub errors_only: bool,
    loaded_name: Option<String>,
    loaded_lines: Vec<LogLine>,
}

impl Default for LogPanelState {
    fn default() -> Self {
        Self {
            source: LogSource::Aggregate,
            file: LogFile::Live,
            query: String::new(),
            errors_only: true, // по умолчанию показываем только ошибки/предупреждения
            loaded_name: None,
            loaded_lines: Vec::new(),
        }
    }
}

impl LogPanelState {
    /// Сброс к дефолту (агрегат · Live) — при закрытии откреплённого окна лога.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Ревизия данных текущего выбора — для форса перерисовки откреплённого окна при
    /// появлении новых строк. У файла (не Live) ревизия постоянна.
    pub fn live_revision(&self, store: &CoreStore, sources: &[LogSourceItem]) -> u64 {
        if !matches!(self.file, LogFile::Live) {
            return 0;
        }
        match self.source {
            LogSource::Local => crate::applog::revision(),
            LogSource::Core(id) => store.core(id).map(|c| c.log_rev).unwrap_or(0),
            LogSource::Aggregate => sources
                .iter()
                .filter_map(|s| match s.source {
                    LogSource::Core(id) => store.core(id).map(|c| c.log_rev),
                    _ => None,
                })
                .fold(0u64, |a, r| a.wrapping_add(r)),
        }
    }

    /// Метка файла текущего источника (для поиска прошлых файлов на диске).
    fn file_label(&self, sources: &[LogSourceItem]) -> String {
        sources
            .iter()
            .find(|s| s.source == self.source)
            .map(|s| s.file_label.clone())
            .unwrap_or_else(|| "app".to_string())
    }

    /// Строки для текущего выбора. Live → из памяти (агрегат — слиянием по времени),
    /// Named → из файла (с кэшем). Возвращает срез во внутренний буфер.
    fn gather(&mut self, store: &CoreStore, sources: &[LogSourceItem]) -> &[LogLine] {
        match &self.file {
            LogFile::Live => {
                self.loaded_name = None;
                self.loaded_lines = match &self.source {
                    LogSource::Local => crate::applog::snapshot(VIEW_LIMIT),
                    LogSource::Core(id) => store
                        .core(*id)
                        .map(|c| c.log_snapshot(VIEW_LIMIT))
                        .unwrap_or_default(),
                    LogSource::Aggregate => aggregate(store, sources),
                };
            }
            LogFile::Named(name) => {
                if self.loaded_name.as_deref() != Some(name.as_str()) {
                    self.loaded_lines = crate::applog::read_file(name, VIEW_LIMIT);
                    self.loaded_name = Some(name.clone());
                }
            }
        }
        &self.loaded_lines
    }
}

/// Слияние живых логов всех ядер области видимости по времени (ts лексикографичен =
/// хронологичен). Каждой строке проставляем `target` = имя ядра-источника.
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

/// Рендер лог-панели: панель управления (источник/файл/поиск/ошибки) + список строк.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut LogPanelState,
    sources: &[LogSourceItem],
    store: &CoreStore,
) {
    controls(ui, state, sources);
    ui.add_space(4.0);

    let query = state.query.trim().to_lowercase();
    let errors_only = state.errors_only;
    let lines = state.gather(store, sources);
    let total = lines.len();
    let filtered: Vec<&LogLine> = lines
        .iter()
        .filter(|l| !errors_only || l.is_errorish())
        .filter(|l| query.is_empty() || l.msg.to_lowercase().contains(&query))
        .collect();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(t!("log.count", shown = filtered.len(), total = total))
                .size(theme::LABEL_SIZE)
                .color(theme::TEXT_3),
        );
    });
    ui.add_space(2.0);

    if filtered.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            let msg = if total == 0 {
                t!("dock.log.empty")
            } else {
                t!("log.empty_filtered")
            };
            ui.label(egui::RichText::new(msg).weak());
        });
        super::tabs::fill_rest(ui);
        return;
    }

    list(ui, &filtered);
}

/// Панель управления: комбо источника, комбо файла (кроме агрегата), поиск, «ошибки».
fn controls(ui: &mut egui::Ui, state: &mut LogPanelState, sources: &[LogSourceItem]) {
    ui.horizontal_wrapped(|ui| {
        // Источник.
        let cur_src = sources
            .iter()
            .find(|s| s.source == state.source)
            .map(|s| s.display.clone())
            .unwrap_or_else(|| t!("log.source.local").to_string());
        let mut src_changed = false;
        egui::ComboBox::from_id_salt("log_source")
            .selected_text(cur_src)
            .show_ui(ui, |ui| {
                for item in sources {
                    if ui
                        .selectable_label(state.source == item.source, &item.display)
                        .clicked()
                        && state.source != item.source
                    {
                        state.source = item.source.clone();
                        src_changed = true;
                    }
                }
            });
        // Смена источника → возвращаемся к Live и сбрасываем кэш файла.
        if src_changed {
            state.file = LogFile::Live;
            state.loaded_name = None;
        }

        // Выбор файла — только для одиночного источника (агрегат пишется в разные
        // файлы по ядрам, единого файла истории нет → только Live).
        if !matches!(state.source, LogSource::Aggregate) {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(t!("log.file")).weak());
            let cur_file = match &state.file {
                LogFile::Live => t!("log.live").to_string(),
                LogFile::Named(n) => n.clone(),
            };
            egui::ComboBox::from_id_salt("log_file")
                .selected_text(cur_file)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(matches!(state.file, LogFile::Live), t!("log.live"))
                        .clicked()
                    {
                        state.file = LogFile::Live;
                    }
                    for f in crate::applog::list_files(&state.file_label(sources)) {
                        let sel = matches!(&state.file, LogFile::Named(n) if n == &f);
                        if ui.selectable_label(sel, &f).clicked() {
                            state.file = LogFile::Named(f);
                        }
                    }
                });
        }

        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text(t!("log.search"))
                .desired_width(160.0),
        );
        ui.add_space(6.0);
        ui.checkbox(&mut state.errors_only, t!("log.errors_only"));
    });
}

/// Виртуализированный список строк (ровная высота → без дребезга автоскролла).
fn list(ui: &mut egui::Ui, lines: &[&LogLine]) {
    let font = egui::FontId::monospace(FONT_PX);
    let row_h = ui.fonts(|f| f.row_height(&font));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show_rows(ui, row_h, lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for line in &lines[range] {
                row(ui, line, &font);
            }
        });
}

fn row(ui: &mut egui::Ui, line: &LogLine, font: &egui::FontId) {
    use egui::text::{LayoutJob, TextFormat};
    let time = line.ts.rsplit(' ').next().unwrap_or(line.ts.as_str());
    let flat = line.msg.replace('\n', " ⏎ ");

    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let fmt = |color| TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    job.append(&format!("{time} "), 0.0, fmt(theme::MUTED));
    // Уровень-бейдж показываем только для Warn/Error (INFO-шум не нужен). Источник
    // (`target`: модуль для локального лога, имя ядра в агрегате) — если задан.
    if matches!(line.level, log::Level::Error | log::Level::Warn) {
        let (tag, col) = level_tag(line.level);
        job.append(&format!("{tag} "), 0.0, fmt(col));
    }
    if !line.target.is_empty() {
        job.append(&format!("{}  ", line.target), 0.0, fmt(theme::MUTED));
    }
    job.append(&flat, 0.0, fmt(theme::TEXT_2));

    // Без on_hover_text: всплывающая подсказка на всю строку мешает (просили убрать).
    ui.label(job);
}

/// Бейдж уровня + цвет.
fn level_tag(level: log::Level) -> (&'static str, egui::Color32) {
    match level {
        log::Level::Error => ("ERR ", theme::RED),
        log::Level::Warn => ("WARN", theme::ACCENT),
        log::Level::Info => ("INFO", theme::TEXT_2),
        log::Level::Debug => ("DBG ", theme::MUTED),
        log::Level::Trace => ("TRC ", theme::MUTED),
    }
}
