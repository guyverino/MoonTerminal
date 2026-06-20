//! Конфиг приложения в ДВУХ файлах рядом с exe:
//! - `servers.enc` (зашифрован): uid/name/key — переносимый секрет (скопировал
//!   файл — и ключи на месте). host/port/transport зашиты в самом ключе MoonBot.
//! - `settings.toml` (открытый): версия схемы + группы + по-серверная мета
//!   (галки active/show_window/feed, группа, рынок, цвет). Привязка к серверу — по uid.
//!
//! Обновление версии программы: старый settings.toml без новых полей читается
//! без потерь (serde-дефолты), а `version` < `SCHEMA_VERSION` запускает один
//! досейв — новые галки дописываются в файл с дефолтами, старые сохраняются.
//!
//! Раскладка по модулям (не валим всё в один файл):
//! - `schema`    — структуры файлов на диске (serde) + версия схемы;
//! - `store`     — чтение/запись файлов (шифрование, бэкап битого settings.toml);
//! - `reconcile` — слияние файлов ↔ рантайм + стабильные uid;
//! - `migrate`   — одноразовые миграции со старых форматов.

pub mod crypto;
pub mod groups;
pub mod lang;
pub mod layout;
pub mod orders;
pub mod paths;
pub mod secrets;
pub mod servers;
pub mod theme;

mod migrate;
mod reconcile;
mod schema;
mod store;
mod toml_io;

pub use groups::GroupConfig;
pub use lang::Language;
pub use layout::{DetachedLayout, GeomRect, GroupLayout, WindowLayout};
pub use orders::{LineStyle, OrdersStyle};
pub use secrets::Secret;
pub use servers::{ChartBucket, FeedFlags, ServerConfig};
pub use theme::ChartTheme;

use std::collections::HashSet;

use crate::market::MarketDataMode;

/// Рантайм-конфиг (смерженный из двух файлов).
#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub servers: Vec<ServerConfig>,
    pub groups: Vec<GroupConfig>,
    /// Язык интерфейса (settings.toml). Дефолт — системная локаль.
    pub language: Language,
    /// Источник рыночных данных (settings.toml). Дефолт — Dedup (провайдер на биржу).
    pub market_mode: MarketDataMode,
    /// Отдельная чарт-вкладка на каждое ядро для AddToChart (settings.toml).
    pub charts_split_by_core: bool,
    /// Писать лог (приложения и ядер) в файлы logs/ (settings.toml). Дефолт on.
    pub log_to_file: bool,
    /// Срок хранения файлов лога, дней; 0 = хранить всё (settings.toml). Дефолт 14.
    pub log_retention_days: u32,
    /// Прибавка к базовым размерам UI-шрифтов в logical px. Дефолт +2.
    pub ui_font_delta: f32,
    /// Общий масштаб геометрии UI. Дефолт 1.0.
    pub ui_scale: f32,
    /// Множитель RAM-budget для retained market history. 100 = авто-база, 800 = 8x.
    pub chart_memory_percent: u16,
    /// Тема оформления чарта (отдельный переносимый theme.toml).
    pub theme: ChartTheme,
    /// Стиль линий ордеров (отдельный переносимый orders.toml).
    pub orders: OrdersStyle,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        // Тема и стиль линий ордеров — отдельные переносимые файлы, грузятся
        // независимо от серверов/групп.
        let theme = ChartTheme::load();
        let orders = OrdersStyle::load();
        if let Some(cfg) = Self::load_plaintext_env(theme.clone(), orders.clone())? {
            return Ok(cfg);
        }
        if paths::servers_path().exists() {
            let sf = store::read_servers()?;
            let meta = store::read_settings();
            let merged = reconcile::merge(sf, meta);
            let mut cfg = Self {
                servers: merged.servers,
                groups: merged.groups,
                language: merged.language,
                market_mode: merged.market_mode,
                charts_split_by_core: merged.charts_split_by_core,
                log_to_file: merged.log_to_file,
                log_retention_days: merged.log_retention_days,
                ui_font_delta: merged.ui_font_delta,
                ui_scale: merged.ui_scale,
                chart_memory_percent: merged.chart_memory_percent,
                theme,
                orders,
            };
            log::info!(
                "конфиг: {} серверов, {} групп",
                cfg.servers.len(),
                cfg.groups.len()
            );
            // Дослоить новые дефолты / зафиксировать свежие uid на диск.
            // Не фатально: при ошибке продолжаем с тем, что уже в памяти.
            if merged.dirty {
                if let Err(e) = cfg.save() {
                    log::warn!("не удалось дослоить конфиг на диск: {e}");
                }
            }
            return Ok(cfg);
        }

        // Миграции со старых форматов (один раз → save() пишет новые файлы).
        if paths::legacy_enc_path().exists() {
            let mut cfg = migrate::from_legacy_enc()?;
            cfg.theme = theme;
            cfg.orders = orders;
            cfg.charts_split_by_core = true;
            cfg.log_to_file = true;
            cfg.log_retention_days = 14;
            cfg.ui_font_delta = schema::default_ui_font_delta();
            cfg.ui_scale = schema::default_ui_scale();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.save()?;
            log::info!("мигрировано из config.enc → servers.enc + settings.toml");
            return Ok(cfg);
        }
        if paths::legacy_toml_path().exists() {
            let mut cfg = migrate::from_legacy_toml()?;
            cfg.theme = theme;
            cfg.orders = orders;
            cfg.charts_split_by_core = true;
            cfg.log_to_file = true;
            cfg.log_retention_days = 14;
            cfg.ui_font_delta = schema::default_ui_font_delta();
            cfg.ui_scale = schema::default_ui_scale();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.save()?;
            log::info!("мигрировано из config.toml → servers.enc + settings.toml");
            return Ok(cfg);
        }

        log::warn!("конфиг не найден — добавь сервера в Настройках");
        Ok(Self {
            theme,
            orders,
            charts_split_by_core: true, // дефолт — отдельная вкладка на ядро
            log_to_file: true,
            log_retention_days: 14,
            ui_font_delta: schema::default_ui_font_delta(),
            ui_scale: schema::default_ui_scale(),
            chart_memory_percent: schema::default_chart_memory_percent(),
            ..Self::default()
        })
    }

    fn load_plaintext_env(theme: ChartTheme, orders: OrdersStyle) -> anyhow::Result<Option<Self>> {
        if std::env::var_os("MOON_CONFIG_PLAINTEXT").is_none() {
            return Ok(None);
        }

        let key = match std::env::var("MOON_CONFIG_PLAINTEXT_KEY") {
            Ok(key) if !key.trim().is_empty() => key,
            _ => {
                let path = std::env::var("MOON_CONFIG_PLAINTEXT_KEY_FILE").map_err(|_| {
                    anyhow::anyhow!(
                        "MOON_CONFIG_PLAINTEXT=1 задан, но нет MOON_CONFIG_PLAINTEXT_KEY \
                         или MOON_CONFIG_PLAINTEXT_KEY_FILE"
                    )
                })?;
                std::fs::read_to_string(&path).map_err(|e| {
                    anyhow::anyhow!("не прочитал MOON_CONFIG_PLAINTEXT_KEY_FILE {path}: {e}")
                })?
            }
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            anyhow::bail!("MOON_CONFIG_PLAINTEXT key пустой");
        }

        let name = std::env::var("MOON_CONFIG_PLAINTEXT_NAME").unwrap_or_else(|_| "default".into());
        let group = std::env::var("MOON_CONFIG_PLAINTEXT_GROUP")
            .unwrap_or_else(|_| servers::default_group());
        let market = std::env::var("MOON_CONFIG_PLAINTEXT_MARKET")
            .unwrap_or_else(|_| servers::default_market());

        log::warn!(
            "MOON_CONFIG_PLAINTEXT=1: тестовый plaintext-конфиг, servers.enc/keyring пропущены"
        );
        Ok(Some(Self {
            servers: vec![ServerConfig {
                id: 1,
                uid: 1,
                name,
                active: true,
                show_window: true,
                feed: FeedFlags::default(),
                key: Secret::new(key),
                group,
                market,
                color: servers::default_color(),
                synthetic: false,
                chart_bundle: String::new(),
            }],
            groups: Vec::new(),
            language: Language::default(),
            market_mode: MarketDataMode::default(),
            charts_split_by_core: true,
            log_to_file: true,
            log_retention_days: servers::default_log_retention_days(),
            ui_font_delta: schema::default_ui_font_delta(),
            ui_scale: schema::default_ui_scale(),
            chart_memory_percent: schema::default_chart_memory_percent(),
            theme,
            orders,
        }))
    }

    /// Сохраняет в два файла. Проставляет стабильные uid, валидирует уникальность
    /// имени и host:port. `&mut self` — т.к. может присвоить uid новым ядрам.
    pub fn save(&mut self) -> anyhow::Result<()> {
        reconcile::ensure_uids(&mut self.servers);
        self.prune_orphan_groups();
        self.validate()?;
        let (sf, meta) = reconcile::split(
            &self.servers,
            &self.groups,
            self.language,
            self.market_mode,
            self.charts_split_by_core,
            self.log_to_file,
            self.log_retention_days,
            self.ui_font_delta,
            self.ui_scale,
            self.chart_memory_percent,
        );
        store::write_servers(&sf)?;
        store::write_settings(&meta)?;
        // Тема и стиль линий — в свои переносимые файлы, независимо от settings.toml.
        self.theme.save()?;
        self.orders.save()?;
        Ok(())
    }

    /// Группа имеет смысл только пока на неё ссылается хоть одно ядро. Сироты
    /// (например, от промежуточных значений при наборе имени) не сохраняем.
    fn prune_orphan_groups(&mut self) {
        let used: HashSet<&str> = self.servers.iter().map(|s| s.group.as_str()).collect();
        self.groups.retain(|g| used.contains(g.name.as_str()));
    }

    /// Проверка уникальности имени сервера и ключа (endpoint теперь внутри ключа,
    /// поэтому одинаковый ключ = одно и то же ядро дважды). Пустые ключи не сравниваем
    /// — это недозаполненные строки в процессе редактирования.
    fn validate(&self) -> anyhow::Result<()> {
        let mut names = HashSet::new();
        let mut keys = HashSet::new();
        for s in &self.servers {
            // core i18n-агностичен: сообщения валидации — простым текстом. Раньше
            // было t!("err.dup_name"/"err.dup_key"); при желании UI перелокализует.
            if !names.insert(s.name.to_lowercase()) {
                anyhow::bail!("duplicate server name: {}", s.name);
            }
            if !s.key.is_empty() && !keys.insert(s.key.expose().to_owned()) {
                anyhow::bail!("duplicate API key (server: {})", s.name);
            }
        }
        Ok(())
    }

    /// Сигнатура «структурной» части конфига: серверы + группы, БЕЗ темы/языка/режима
    /// рынка. По ней App решает, нужен ли при сохранении настроек реконнект к ядрам и
    /// пересоздание окон. Тема меняется живо, язык и режим рынка — без реконнекта,
    /// поэтому их исключаем (нейтрализуем дефолтом).
    pub fn structural_sig(&self) -> String {
        // Связка чарт-вкладок (`chart_bundle`) — чисто UI-группировка вкладок AddToChart:
        // её смена НЕ требует реконнекта ядер/ребилда сессий, только пересборки окон групп
        // (см. apply_settings). Нейтрализуем, чтобы не считать структурной.
        let servers: Vec<ServerConfig> = self
            .servers
            .iter()
            .map(|s| ServerConfig {
                chart_bundle: String::new(),
                ..s.clone()
            })
            .collect();
        let (sf, meta) = reconcile::split(
            &servers,
            &self.groups,
            Language::default(),
            MarketDataMode::default(),
            true, // нейтрализуем: тумблер чартов не влияет на структуру (без ребилда)
            true, // лог-настройки тоже не структурные (без реконнекта/ребилда)
            14,
            schema::default_ui_font_delta(),
            schema::default_ui_scale(),
            schema::default_chart_memory_percent(),
        );
        let a = toml::to_string(&sf).unwrap_or_default();
        let b = toml::to_string(&meta).unwrap_or_default();
        format!("{a}\n{b}")
    }

    /// Свойства группы по имени (существующие или дефолт).
    pub fn group(&self, name: &str) -> GroupConfig {
        self.groups
            .iter()
            .find(|g| g.name == name)
            .cloned()
            .unwrap_or_else(|| GroupConfig::new(name))
    }

    /// Настраивали ли уже конфиг: есть ли хоть один сервер с ключом. False = первый
    /// запуск (ещё ничего не вводили) — показываем только окно Настроек.
    pub fn has_keyed_server(&self) -> bool {
        self.servers.iter().any(|s| !s.key.is_empty())
    }
}
