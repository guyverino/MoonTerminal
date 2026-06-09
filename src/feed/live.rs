//! Live-backend: подключение к ядру MoonBot через MoonProtoBeta.
//! Единственный модуль, знающий про moonproto.
//!
//! Поток: snapshot-driven. Трейды читаем retained-курсором `SeqRingReader`,
//! стакан — из `snapshot().order_book(...)`. Доменные события только дренируем,
//! чтобы очередь не росла; данные берём из read-model snapshot.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moonproto::state::OrderBookKind;
use moonproto::{
    ClientConfig, ConnectConfig, Event, FieldValue, InitConfig, InitialStrategies, LifecycleEvent,
    MoonClient, StrategyFieldType, StrategyFieldUiKind, StrategySchema, StrategySnapshot,
    TradesStreamMode, TransportMode,
};

use super::{
    ConnStatus, CoreCmd, DetectRow, ExchangeId, FeedMsg, FeedTx, Level, OrderBook, OrderRow,
    SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, Side, StrategyRow, StrategySchemaModel,
    Tick,
};
use crate::config::ServerConfig;
use crate::db::{ReportRow, ReportTx};

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Полный снимок полей ордера из живой модели, запоминаемый по серверному db_id.
/// Источник всех данных, которых нет в close-SQL (монета/открытие/цены/статы).
struct OrderMeta {
    coin: String,
    isshort: bool,
    buyprice: f64,
    sellprice: f64,
    quantity: f64,
    spentbtc: f64,
    gainedbtc: f64,
    lev: i64,
    strategyid: i64,
    taskid: i64,
    exorderid: Option<String>,
    emulator: bool,
    buydate: Option<i64>,
    sellsetdate: Option<i64>,
    closedate: Option<i64>,
}

/// Delphi `TDateTime` (дней с 1899-12-30) → unix-секунды. 0/пусто → None.
fn delphi_to_unix(d: f64) -> Option<i64> {
    if d > 1.0 {
        Some(((d - 25569.0) * 86400.0).round() as i64)
    } else {
        None
    }
}

pub fn run(
    server: &ServerConfig,
    tx: &FeedTx,
    cmd_rx: &Receiver<CoreCmd>,
    reports: Option<&ReportTx>,
) -> anyhow::Result<()> {
    let _ = tx.send(FeedMsg::Status(ConnStatus::Connecting));

    // 1. Ключ -> мастер/мак ключи + предложенная сеть.
    let info = moonproto::parse_key_info(server.key.expose())
        .ok_or_else(|| anyhow::anyhow!("не удалось разобрать ключ MoonBot (server.key)"))?;

    // 2. Endpoint берётся из ключа (host/port/transport зашиты в нём; отдельных
    //    полей в конфиге больше нет).
    let net = info.network.as_ref();
    let host: String = net
        .and_then(|n| n.address)
        .map(|a| a.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port: u16 = net.map(|n| n.port).filter(|p| *p != 0).unwrap_or(3000);
    let transport = net.map(|n| n.transport_mode).unwrap_or(TransportMode::V0);
    log::info!("live connect {host}:{port} market={}", server.market);

    let client_cfg = ClientConfig::new(host, port, info.keys.master_key, info.keys.mac_key)
        .with_transport_mode(transport);

    // 3. Init БЕЗ рыночных подписок. Рыночную роль ядра задаёт координатор командой
    //    SetMarket после того, как узнает биржу ядра (Identity) и изберёт провайдера:
    //    только ОДНО ядро на биржу делает subscribe_all_trades, остальные шлют лишь
    //    аккаунт. Так трейды биржи тянутся 1 раз, а не с каждого из 200 ядер.
    //    initial_strategies ОБЯЗАТЕЛЬНО — иначе init зависает после Connected.
    let init = InitConfig {
        initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
        ..Default::default()
    };

    // connect (не blocking) + connect_timeout, чтобы зависший шаг init пришёл
    // как ConnectFailed с причиной, а не молчал.
    let client = MoonClient::connect(
        client_cfg,
        ConnectConfig::new(init).with_connect_timeout(Duration::from_secs(15)),
    )?;

    // Рыночная роль ядра (задаётся координатором командой SetMarket).
    // is_provider — ретейним ли ВСЕ трейды биржи (subscribe_all_trades).
    // wanted — рынки, которые активно обслуживаем (стакан + чтение крестиков).
    // cursors — свой курсор ленты на каждый обслуживаемый рынок.
    let mut is_provider = false;
    let mut wanted: Vec<String> = Vec::new();
    let mut cursors = HashMap::new(); // market -> SeqRingCursor (тип выводится)
    let mut identity_sent = false;
    let mut rows = Vec::new(); // тип Vec<TradeHistoryRow> выводится
    let mut last_book = Instant::now();
    let mut last_orders = Instant::now();
    let mut last_strats = Instant::now();
    // Курсоры выгрузки стратегий: revision схемы и сигнатура состава/checked —
    // шлём только при изменениях (поля стратегий тяжёлые, гонять каждую секунду незачем).
    let mut last_schema_rev: u64 = u64::MAX;
    let mut last_strat_sig: u64 = u64::MAX;
    // Монотонный per-core номер детекта — курсор ингеста в ленту детектов UI.
    let mut detect_seq: u64 = 0;
    // Полные данные ордера копим по СТАБИЛЬНОМУ uid (есть с открытия). А db_id у
    // открытого ордера почти всегда 0 — он присваивается лишь перед закрытием,
    // когда строка пишется в Orders DB ядра. Поэтому держим ещё карту db_id→uid
    // (заполняется в тот момент, когда db_id появился). На close-report (там
    // только db_id) идём db_id → uid → полные данные.
    let mut order_by_uid: HashMap<u64, OrderMeta> = HashMap::new();
    let mut dbid_to_uid: HashMap<i32, u64> = HashMap::new();

    loop {
        // Команды роли от координатора (полное желаемое состояние, не дельта).
        // Закрытие канала = координатор ушёл → отключаемся.
        loop {
            match cmd_rx.try_recv() {
                Ok(CoreCmd::SetMarket { provider, markets }) => {
                    // Переход провайдерства: вкл → ретейним все трейды биржи; выкл →
                    // снимаем подписку и забываем курсоры.
                    if provider != is_provider {
                        if provider {
                            let _ = client
                                .streams()
                                .subscribe_all_trades(TradesStreamMode::TradesOnly);
                            log::info!("core {} → market provider (all-trades)", server.id);
                        } else {
                            let _ = client.streams().unsubscribe_all_trades();
                            cursors.clear();
                            log::info!("core {} → account-only", server.id);
                        }
                        is_provider = provider;
                    }
                    // Не провайдер не обслуживает рынки (стакан/чтение) вообще.
                    let markets = if provider { markets } else { Vec::new() };
                    // Диф обслуживаемых рынков: новым подписываем стакан и сбрасываем
                    // курсор (перечитать историю с начала), убранным — отписываем.
                    for m in &markets {
                        if !wanted.iter().any(|w| w == m) {
                            let _ = client.streams().subscribe_orderbook(m.clone());
                            cursors.remove(m);
                        }
                    }
                    for m in &wanted {
                        if !markets.iter().any(|x| x == m) {
                            let _ = client.streams().unsubscribe_orderbook(m.clone());
                            cursors.remove(m);
                        }
                    }
                    wanted = markets;
                }
                Ok(CoreCmd::StrategiesAction { checks, start_stop }) => {
                    // 1. Синхронизация галок: правим локальный checked у изменённых и
                    //    шлём серверу дельту (CheckedSync).
                    for (id, checked) in &checks {
                        let _ = client.strategies().set_checked(*id, *checked);
                    }
                    if !checks.is_empty() {
                        let _ = client.strategies().send_checked_delta();
                    }
                    // 2. Старт/стоп отмеченных (отдельная команда движка).
                    match start_stop {
                        Some(true) => {
                            let _ = client.strategies().start();
                        }
                        Some(false) => {
                            let _ = client.strategies().stop();
                        }
                        None => {}
                    }
                    log::info!(
                        "core {} strategies action: checks={} start_stop={:?}",
                        server.id,
                        checks.len(),
                        start_stop
                    );
                }
                Ok(CoreCmd::EditStrategyFields { ids, changes }) => {
                    // Берём полный снимок каждой стратегии, правим поля по типу и
                    // отправляем sync_local_strategies (moonproto правит только целиком).
                    if let Some(snap) = client.snapshot() {
                        let strats = snap.strats();
                        let schema = strats.strategy_schema();
                        let mut modified = Vec::new();
                        for id in &ids {
                            let Some(s) = strats.snapshot(*id) else { continue };
                            let mut sc = s.clone();
                            for (name, val) in &changes {
                                let existing = sc.fields.get(name).cloned();
                                let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                                sc.fields
                                    .insert(name.as_str(), fv_from_str(existing.as_ref(), stype, val));
                            }
                            modified.push(sc);
                        }
                        let n = modified.len();
                        if n > 0 {
                            let _ = client.strategies().sync_local_strategies(modified);
                            log::info!(
                                "core {} edit {} strategies, {} changes",
                                server.id,
                                n,
                                changes.len()
                            );
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = client.disconnect();
                    return Ok(());
                }
            }
        }

        // Биржа ядра (из server_info после BaseCheck) — координатору для группировки
        // и выбора провайдера. Шлём один раз, как только идентичность известна.
        if !identity_sent {
            if let Some(code) = client.server_info().and_then(|i| i.exchange_code) {
                let _ = tx.send(FeedMsg::Identity(ExchangeId(code.to_byte())));
                identity_sent = true;
            }
        }

        // Lifecycle -> статус (стадии и ошибки видны прямо в бейдже).
        for ev in client.drain_lifecycle_events() {
            log::info!("lifecycle: {ev:?}");
            let st = match ev {
                LifecycleEvent::Connecting => ConnStatus::Stage("connecting…".into()),
                LifecycleEvent::Connected { fresh } => {
                    ConnStatus::Stage(if fresh { "connected, init…".into() } else { "reconnected".into() })
                }
                LifecycleEvent::InitStepCompleted { step, .. } => {
                    ConnStatus::Stage(format!("init: {step}"))
                }
                LifecycleEvent::Ready => ConnStatus::Ready,
                LifecycleEvent::Reconnecting => ConnStatus::Stage("reconnecting…".into()),
                LifecycleEvent::ServerRestart => ConnStatus::Stage("server restart…".into()),
                LifecycleEvent::ConnectFailed { error } => ConnStatus::Failed(error.to_string()),
                LifecycleEvent::BindFailed { consecutive_failures } => ConnStatus::Failed(format!(
                    "UDP bind failed x{consecutive_failures} (VPN/firewall/порты?)"
                )),
                LifecycleEvent::Disconnected => ConnStatus::Disconnected,
            };
            let _ = tx.send(FeedMsg::Status(st));
        }

        // Дренируем доменные события. Тики/стакан/ордера берём из snapshot;
        // детекты и отчёты — только из потока событий, по флагам сервера.
        let events = client.drain_events();
        if server.feed.detects || (server.feed.reports && reports.is_some()) {
            let mut detects: Vec<DetectRow> = Vec::new();
            // Снимок для полей стратегии-источника детекта (SoundAlert/KeepAlert).
            let detect_snap = server.feed.detects.then(|| client.snapshot()).flatten();
            for ev in events {
                match ev {
                    Event::Detect(d) if server.feed.detects => {
                        let (sound_alert, keep_alert_secs) = detect_snap
                            .as_ref()
                            .and_then(|s| s.strats().snapshot(d.strategy_id))
                            .map(alert_params)
                            .unwrap_or((false, 60));
                        detect_seq += 1;
                        detects.push(DetectRow {
                            seq: detect_seq,
                            market: d.market_name,
                            strategy_id: d.strategy_id,
                            is_short: d.is_short,
                            kind_bits: d.kind_bits,
                            msg: d.msg,
                            time_ms: now_ms(),
                            sound_alert,
                            keep_alert_secs,
                        });
                    }
                    Event::ClosedSellOrderReport(r) if server.feed.reports => {
                        if let Some(tx_db) = reports {
                            // Разбираем SQL (insert ИЛИ update). Поля берём из SQL;
                            // чего там нет (open-side у update-формы) — из снапшота
                            // ордеров по db_id.
                            // Поля из close-SQL (финальные/авторитетные); чего там
                            // нет — из снимка модели ордера (m) по db_id.
                            let p = crate::db::parse_report_sql(&r.sql);
                            // db_id → uid → полные данные (uid стабилен с открытия).
                            // Если db_id ещё не успели замапить — сканируем ТЕКУЩИЙ
                            // снапшот: ордер часто ещё в модели с присвоенным db_id,
                            // а его полные данные уже есть в order_by_uid по uid.
                            let m = match dbid_to_uid.get(&(r.db_id as i32)) {
                                Some(uid) => order_by_uid.get(uid),
                                None => client
                                    .snapshot()
                                    .and_then(|snap| {
                                        snap.orders()
                                            .iter()
                                            .find(|o| o.db_id as i64 == r.db_id)
                                            .map(|o| o.uid)
                                    })
                                    .and_then(|uid| order_by_uid.get(&uid)),
                            };
                            // Логируем СЫРОЙ report-SQL в logs/commands.log — чтобы
                            // видеть, INSERT или UPDATE шлёт ядро и что внутри.
                            let form = if r.sql.trim_start().get(..6).map(|s| s.eq_ignore_ascii_case("insert")).unwrap_or(false) {
                                "INSERT"
                            } else {
                                "UPDATE"
                            };
                            crate::applog::command(&format!(
                                "core={} ({}) db_id={} form={} link={} coin={:?} buydate={:?}\n    SQL: {}",
                                server.uid, server.name, r.db_id, form, m.is_some(),
                                m.map(|x| x.coin.clone()).or_else(|| p.coin.clone()),
                                m.and_then(|x| x.buydate).or(p.buydate),
                                r.sql,
                            ));
                            let _ = tx_db.send(ReportRow {
                                core_uid: server.uid, // СТАБИЛЬНЫЙ uid, не рантайм-id
                                core_name: server.name.clone(),
                                db_id: r.db_id,
                                taskid: p.taskid.or_else(|| m.map(|m| m.taskid)),
                                exorderid: m.and_then(|m| m.exorderid.clone()),
                                coin: p.coin.or_else(|| m.map(|m| m.coin.clone())),
                                isshort: p.isshort.or_else(|| m.map(|m| m.isshort)),
                                buydate: p.buydate.or_else(|| m.and_then(|m| m.buydate)),
                                sellsetdate: p.sellsetdate.or_else(|| m.and_then(|m| m.sellsetdate)),
                                closedate: p.close_date.or_else(|| m.and_then(|m| m.closedate)),
                                quantity: p.quantity.or_else(|| m.map(|m| m.quantity)),
                                buyprice: p.buyprice.or_else(|| m.map(|m| m.buyprice)),
                                sellprice: p.sellprice.or_else(|| m.map(|m| m.sellprice)),
                                spentbtc: p.spent_btc.or_else(|| m.map(|m| m.spentbtc)),
                                gainedbtc: p.gained_btc.or_else(|| m.map(|m| m.gainedbtc)),
                                profitbtc: p.profit_btc.or_else(|| m.map(|m| m.gainedbtc - m.spentbtc)),
                                lev: p.lev.or_else(|| m.map(|m| m.lev)),
                                strategyid: p.strategyid.or_else(|| m.map(|m| m.strategyid)),
                                emulator: m.map(|m| m.emulator),
                                status: p.status,
                                sellreason: p.sell_reason,
                                comment: p.comment,
                                sql: r.sql,
                            });
                        }
                    }
                    _ => {}
                }
            }
            if !detects.is_empty() && tx.send(FeedMsg::Detects(detects)).is_err() {
                break;
            }
        }

        // Снимок дёшев (Arc-clone): карту ордеров обновляем КАЖДУЮ итерацию (~8мс)
        // — ловим короткоживущие ордера для close-report. Список ордеров в UI —
        // троттлим до ~4 Гц.
        if server.feed.orders || server.feed.reports {
            let orders_due =
                server.feed.orders && last_orders.elapsed() >= Duration::from_millis(250);
            if let Some(snap) = client.snapshot() {
                let mut order_rows: Vec<OrderRow> = Vec::new();
                for o in snap.orders().iter() {
                    // Полные данные ордера по uid (есть с открытия); db_id→uid —
                    // когда db_id появился (перед закрытием). close-SQL этих полей
                    // не несёт.
                    if server.feed.reports {
                        order_by_uid.insert(
                            o.uid,
                            OrderMeta {
                                coin: o.market_name.clone(),
                                isshort: o.is_short,
                                buyprice: o.buy_price,
                                sellprice: o.sell_price,
                                quantity: o.buy_order.quantity,
                                spentbtc: o.buy_order.spent_btc,
                                gainedbtc: o.sell_order.total_btc,
                                lev: o.buy_order.leverage as i64,
                                strategyid: o.strat_id as i64,
                                taskid: o.uid as i64,
                                exorderid: (o.buy_order.int_id != 0)
                                    .then(|| o.buy_order.int_id.to_string()),
                                emulator: o.emulator_mode,
                                buydate: delphi_to_unix(o.buy_order.open_time),
                                sellsetdate: delphi_to_unix(o.sell_order.create_time),
                                closedate: delphi_to_unix(o.sell_order.close_time),
                            },
                        );
                        if o.db_id != 0 {
                            dbid_to_uid.insert(o.db_id, o.uid);
                        }
                    }
                    if !orders_due {
                        continue; // не время обновлять UI-список (или отчёты-онли)
                    }
                    // Тип стратегии (вид) по strat_id.
                    let strat = match snap.strats().snapshot(o.strat_id) {
                        Some(s) => strat_kind_name(s.kind().ordinal()).to_string(),
                        None => o.strat_id.to_string(),
                    };
                    // Входная нога: buy для long, sell для short.
                    let leg = if o.is_short { &o.sell_order } else { &o.buy_order };
                    let fill_pct = if leg.quantity > 0.0 {
                        ((leg.quantity - leg.quantity_remaining) / leg.quantity * 100.0) as f32
                    } else {
                        0.0
                    };
                    order_rows.push(OrderRow {
                        market: o.market_name.clone(),
                        is_short: o.is_short,
                        size: leg.quantity,
                        sl_on: o.stops.stop_loss_enabled(),
                        ts_on: o.stops.trailing_enabled(),
                        vstop_on: o.vstop_on,
                        buy_price: o.buy_price,
                        price: snap
                            .markets()
                            .price(&o.market_name)
                            .map(|p| p.p_last as f32)
                            .unwrap_or(0.0),
                        fill_pct,
                        strat,
                    });
                }
                if orders_due {
                    last_orders = Instant::now();
                    if tx.send(FeedMsg::Orders(order_rows)).is_err() {
                        break;
                    }
                }
            }
        }

        // Стратегии ядра (для окна стратегий) — проверяем ~1 Гц, но шлём только при
        // изменениях: схему по revision, состав/значения — по сигнатуре.
        if server.feed.strategies && last_strats.elapsed() >= Duration::from_secs(1) {
            last_strats = Instant::now();
            if let Some(snap) = client.snapshot() {
                let strats = snap.strats();

                // Схема (секции/поля по видам) — при смене revision.
                let sr = strats.strategy_schema_revision();
                if sr != last_schema_rev {
                    last_schema_rev = sr;
                    if let Some(schema) = strats.strategy_schema() {
                        if tx
                            .send(FeedMsg::StrategySchema(build_schema_model(schema)))
                            .is_err()
                        {
                            break;
                        }
                    }
                }

                // Состав/значения — при смене сигнатуры (id/ver/last_date/checked).
                let mut sig = 0u64;
                for s in strats.snapshots() {
                    sig = sig
                        .wrapping_mul(1099511628211)
                        .wrapping_add(s.strategy_id)
                        .wrapping_add((s.strategy_ver as u32 as u64).wrapping_shl(1))
                        .wrapping_add(s.last_date)
                        .wrapping_add(s.checked as u64);
                }
                if sig != last_strat_sig {
                    last_strat_sig = sig;
                    let strategies: Vec<StrategyRow> = strats
                        .snapshots()
                        .map(|s| {
                            let (sound_alert, keep_alert_secs) = alert_params(s);
                            let name = s
                                .strategy_name()
                                .filter(|n| !n.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("strat {}", s.strategy_id));
                            let fields = s
                                .fields
                                .iter()
                                .map(|(n, v)| (n.to_string(), fmt_field(v)))
                                .collect();
                            StrategyRow {
                                id: s.strategy_id,
                                name,
                                kind: strat_kind_name(s.kind().ordinal()).to_string(),
                                kind_ordinal: s.kind().ordinal(),
                                folder_path: s.path.to_string(),
                                checked: s.checked,
                                is_short: s.is_short(),
                                sound_alert,
                                keep_alert_secs,
                                fields,
                            }
                        })
                        .collect();
                    if tx.send(FeedMsg::Strategies(strategies)).is_err() {
                        break;
                    }
                }
            }
        }

        // Рыночные данные обслуживаем, только если мы провайдер и есть wanted-рынки.
        // Крестики читаем по каждому рынку своим курсором; стакан троттлим ~20 Гц.
        if is_provider && !wanted.is_empty() {
          if let Some(snap) = client.snapshot() {
            // Трейды -> крестики (append-only через курсор на рынок).
            for market in &wanted {
                let Some(reader) = snap
                    .market_history_readers(market)
                    .and_then(|r| r.futures_trades)
                else {
                    continue;
                };
                let cur = cursors
                    .entry(market.clone())
                    .or_insert_with(|| reader.cursor_from_oldest());
                rows.clear();
                reader.copy_new_since(cur, 8192, &mut rows);
                if !rows.is_empty() {
                    let ticks: Vec<Tick> = rows
                        .iter()
                        .map(|r| Tick {
                            time_ms: r.unix_millis() as f64,
                            price: r.price,
                            qty: r.quantity(),
                            side: if r.is_buy() { Side::Buy } else { Side::Sell },
                        })
                        .collect();
                    if tx
                        .send(FeedMsg::Ticks {
                            market: market.clone(),
                            ticks,
                        })
                        .is_err()
                    {
                        let _ = client.disconnect();
                        return Ok(()); // координатор ушёл.
                    }
                }
            }

            // Стакан по каждому рынку — троттлим ~20 Гц.
            if last_book.elapsed() >= Duration::from_millis(50) {
                last_book = Instant::now();
                for market in &wanted {
                    if let Some(book) = snap.order_book(market, OrderBookKind::Futures) {
                        let ob = OrderBook {
                            bids: book
                                .buys
                                .iter()
                                .map(|l| Level {
                                    price: l.rate as f32,
                                    qty: l.quantity as f32,
                                })
                                .collect(),
                            asks: book
                                .sells
                                .iter()
                                .map(|l| Level {
                                    price: l.rate as f32,
                                    qty: l.quantity as f32,
                                })
                                .collect(),
                        };
                        if tx
                            .send(FeedMsg::OrderBook {
                                market: market.clone(),
                                book: ob,
                            })
                            .is_err()
                        {
                            let _ = client.disconnect();
                            return Ok(());
                        }
                    }
                }
            }
          }
        }

        std::thread::sleep(Duration::from_millis(8));
    }

    let _ = client.disconnect();
    Ok(())
}

/// (SoundAlert, KeepAlert сек) из полей стратегии. Дефолт — (false, 60):
/// кнопку-детект показываем только при SoundAlert=Yes, держим KeepAlert секунд.
fn alert_params(s: &StrategySnapshot) -> (bool, u32) {
    let sound = s.field_bool_or_false("SoundAlert");
    let keep = match s.fields.get("KeepAlert") {
        Some(FieldValue::Int32(v)) => (*v).max(0) as u32,
        Some(FieldValue::UInt32(v)) => *v,
        _ => 60,
    };
    (sound, keep)
}

/// Форматирует значение поля стратегии в строку (read-only показ в плашках).
fn fmt_field(v: &FieldValue) -> String {
    match v {
        FieldValue::Bool(b) => if *b { "Yes" } else { "No" }.to_string(),
        FieldValue::Int32(n) => n.to_string(),
        FieldValue::Int64(n) => n.to_string(),
        FieldValue::UInt32(n) => n.to_string(),
        FieldValue::UInt64(n) => n.to_string(),
        FieldValue::Byte(n) => n.to_string(),
        FieldValue::Word(n) => n.to_string(),
        FieldValue::Double(d) => fmt_num(*d),
        FieldValue::Single(f) => fmt_num(*f as f64),
        FieldValue::String(s) => s.clone(),
    }
}

/// Собирает `FieldValue` из строки UI по ТИПУ поля: приоритет — тип существующего
/// значения снимка, иначе тип из схемы, иначе строка. Кривое число → 0.
fn fv_from_str(existing: Option<&FieldValue>, stype: Option<StrategyFieldType>, s: &str) -> FieldValue {
    let b = || matches!(s.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on");
    let i = |def: i64| s.trim().parse::<i64>().unwrap_or(def);
    let u = || s.trim().parse::<u64>().unwrap_or(0);
    let f = || s.trim().parse::<f64>().unwrap_or(0.0);
    // По существующему значению.
    if let Some(ev) = existing {
        return match ev {
            FieldValue::Bool(_) => FieldValue::Bool(b()),
            FieldValue::Int32(_) => FieldValue::Int32(i(0) as i32),
            FieldValue::Int64(_) => FieldValue::Int64(i(0)),
            FieldValue::UInt32(_) => FieldValue::UInt32(u() as u32),
            FieldValue::UInt64(_) => FieldValue::UInt64(u()),
            FieldValue::Byte(_) => FieldValue::Byte(u() as u8),
            FieldValue::Word(_) => FieldValue::Word(u() as u16),
            FieldValue::Double(_) => FieldValue::Double(f()),
            FieldValue::Single(_) => FieldValue::Single(f() as f32),
            FieldValue::String(_) => FieldValue::String(s.to_string()),
        };
    }
    // По типу схемы.
    match stype {
        Some(StrategyFieldType::Bool) => FieldValue::Bool(b()),
        Some(StrategyFieldType::Int32) => FieldValue::Int32(i(0) as i32),
        Some(StrategyFieldType::Int64) => FieldValue::Int64(i(0)),
        Some(StrategyFieldType::UInt32) => FieldValue::UInt32(u() as u32),
        Some(StrategyFieldType::UInt64) => FieldValue::UInt64(u()),
        Some(StrategyFieldType::Byte) => FieldValue::Byte(u() as u8),
        Some(StrategyFieldType::Word) => FieldValue::Word(u() as u16),
        Some(StrategyFieldType::Double) => FieldValue::Double(f()),
        Some(StrategyFieldType::Single) => FieldValue::Single(f() as f32),
        _ => FieldValue::String(s.to_string()),
    }
}

/// Компактное число без хвостовых нулей.
fn fmt_num(d: f64) -> String {
    let s = format!("{d:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Декаплированная модель схемы из moonproto `StrategySchema`: по каждому виду —
/// его секции (editor sections) с полями (имя/тип/вид виджета/пиклист/дефолт).
fn build_schema_model(schema: &StrategySchema) -> StrategySchemaModel {
    let kinds = schema
        .kinds
        .iter()
        .map(|k| {
            let kind = k.kind();
            let sections = schema
                .editor_sections_for_strategy_kind(kind)
                .into_iter()
                .map(|sec| SchemaSection {
                    title: sec.title,
                    fields: sec
                        .fields
                        .iter()
                        .map(|f| SchemaField {
                            name: f.name.clone(),
                            type_name: f.type_id.name().to_string(),
                            ui: map_ui(f.ui_kind),
                            picklist: f.static_picklist.clone(),
                            default: f.default_value.as_ref().map(fmt_field),
                        })
                        .collect(),
                })
                .collect();
            SchemaKind {
                ordinal: k.ordinal(),
                name: k.name.clone(),
                sections,
            }
        })
        .collect();
    StrategySchemaModel { kinds }
}

fn map_ui(u: StrategyFieldUiKind) -> SchemaFieldUi {
    match u {
        StrategyFieldUiKind::Checkbox => SchemaFieldUi::Checkbox,
        StrategyFieldUiKind::Combo => SchemaFieldUi::Combo,
        StrategyFieldUiKind::Color => SchemaFieldUi::Color,
        _ => SchemaFieldUi::Edit, // Edit + Unknown
    }
}

/// Тип (вид) стратегии MoonBot по ordinal `StrategyKind`.
fn strat_kind_name(ordinal: u8) -> &'static str {
    match ordinal {
        0 => "Unknown",
        1 => "Telegram",
        2 => "Drops",
        3 => "Walls",
        4 => "Volumes",
        5 => "Pump Detection",
        6 => "Moon Shot",
        7 => "V Lite",
        8 => "Delta",
        9 => "Waves",
        10 => "Combo",
        11 => "UDP",
        12 => "Manual",
        13 => "Moon Strike",
        14 => "New Listing",
        15 => "Liquidations",
        16 => "Top Market",
        17 => "EMA",
        18 => "Spread",
        19 => "Chart Wall",
        20 => "Moon Hook",
        21 => "Activity",
        22 => "Alerts",
        23 => "Watcher",
        _ => "?",
    }
}
