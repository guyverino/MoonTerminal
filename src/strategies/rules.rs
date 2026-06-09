//! Правила зависимостей полей стратегий: какое поле редактируемо/раздел активен в
//! зависимости от значений ДРУГИХ полей. Источник — `assets/param_deps.toml`
//! (`"Поле" = "A=VAL;B<>VAL"`). Файл читается на лету из внешнего пути (dev:
//! правишь — видишь сразу), а как фолбэк вшит в exe. Парсим имя→условия один раз.
//!
//! Логика разделов выводится отсюда же + соглашение имён `Ignore*` (см. mod.rs):
//! отдельного конфига разделов нет.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::Deserialize;

/// Внешний путь (относительно cwd) для hot-reload в dev (`cargo run` → корень крейта).
const EXTERNAL: &str = "assets/param_deps.toml";
/// Фолбэк, вшитый в бинарь (release-запуск без assets рядом).
const BUNDLED: &str = include_str!("../../assets/param_deps.toml");

/// Значения полей выбранной стратегии: имя(lowercase) → значение(как есть).
pub type Values = HashMap<String, String>;

/// Одно условие: `field` (op) `value`. `ne=true` → `<>`, иначе `=`.
#[derive(Clone)]
struct Cond {
    field: String,
    ne: bool,
    value: String,
}

#[derive(Default, Deserialize)]
struct DepsFile {
    #[serde(default)]
    deps: HashMap<String, String>,
}

pub struct Rules {
    /// Имя поля(lowercase) → список условий (через ; — И).
    deps: HashMap<String, Vec<Cond>>,
    /// mtime внешнего файла — для hot-reload.
    mtime: Option<SystemTime>,
}

impl Rules {
    /// Грузит правила: внешний файл, если есть, иначе вшитый фолбэк.
    pub fn load() -> Self {
        let mut r = Rules {
            deps: HashMap::new(),
            mtime: None,
        };
        match std::fs::read_to_string(EXTERNAL) {
            Ok(content) => {
                r.mtime = file_mtime();
                r.parse_into(&content);
            }
            Err(_) => r.parse_into(BUNDLED),
        }
        r
    }

    /// Перечитывает внешний файл, если он изменился. true — перечитали (нужен кадр).
    pub fn reload_if_changed(&mut self) -> bool {
        let m = file_mtime();
        if m.is_some() && m != self.mtime {
            if let Ok(content) = std::fs::read_to_string(EXTERNAL) {
                self.mtime = m;
                self.deps.clear();
                self.parse_into(&content);
                return true;
            }
        }
        false
    }

    fn parse_into(&mut self, content: &str) {
        let file: DepsFile = toml::from_str(content).unwrap_or_default();
        for (name, expr) in file.deps {
            self.deps.insert(name.to_lowercase(), parse_conds(&expr));
        }
    }

    /// Поле активно (редактируемо), если все его условия истинны на текущих
    /// значениях. Нет правила — активно. Условие на поле, которого НЕТ в values
    /// (т.е. нет у этого вида стратегии вовсе), неприменимо — не блокирует. Поля
    /// схемы кладутся в values с дефолтом/пустым (см. `selected_values`), так что
    /// «нет в values» = «нет у вида», а несохранённое поле сравнивается по дефолту.
    pub fn field_active(&self, name: &str, values: &Values) -> bool {
        match self.deps.get(&name.to_lowercase()) {
            None => true,
            Some(conds) => conds.iter().all(|c| match values.get(&c.field) {
                None => true,
                Some(v) => {
                    let eq = v.eq_ignore_ascii_case(&c.value);
                    if c.ne {
                        !eq
                    } else {
                        eq
                    }
                }
            }),
        }
    }
}

/// mtime внешнего файла правил (None — файла нет).
fn file_mtime() -> Option<SystemTime> {
    std::fs::metadata(EXTERNAL).ok().and_then(|m| m.modified().ok())
}

/// Разбирает `A=VAL;B<>VAL` в условия. Поля/значения — в lowercase для сравнения.
fn parse_conds(expr: &str) -> Vec<Cond> {
    expr.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            if let Some(i) = part.find("<>") {
                Some(Cond {
                    field: part[..i].trim().to_lowercase(),
                    ne: true,
                    value: part[i + 2..].trim().to_lowercase(),
                })
            } else {
                part.find('=').map(|i| Cond {
                    field: part[..i].trim().to_lowercase(),
                    ne: false,
                    value: part[i + 1..].trim().to_lowercase(),
                })
            }
        })
        .collect()
}
