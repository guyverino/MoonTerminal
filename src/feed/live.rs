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
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    TradesStreamMode, TransportMode,
};

use super::report::{delphi_to_unix, send_close_report, OrderIndex, OrderMeta};
use super::strategies::{alert_params, build_schema_model, fmt_field, fv_from_str, strat_kind_name};
use super::{
    ConnStatus, CoreCmd, DetectRow, ExchangeId, FeedMsg, FeedTx, Level, OrderBook, OrderRow, Side,
    StrategyRow, Tick,
};
use crate::config::ServerConfig;
use crate::db::ReportTx;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
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
    // Полные данные ордеров для close-report'ов (uid/db_id) — см. feed::report.
    let mut orders_index = OrderIndex::default();

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
                        let params = detect_snap
                            .as_ref()
                            .and_then(|s| s.strats().snapshot(d.strategy_id))
                            .map(alert_params)
                            .unwrap_or_default();
                        detect_seq += 1;
                        detects.push(DetectRow {
                            seq: detect_seq,
                            market: d.market_name,
                            time_ms: now_ms(),
                            sound_alert: params.sound_alert,
                            keep_alert_secs: params.keep_alert_secs,
                            add_to_chart: params.add_to_chart,
                            keep_in_chart_secs: params.keep_in_chart_secs,
                        });
                    }
                    Event::ClosedSellOrderReport(r) if server.feed.reports => {
                        if let Some(tx_db) = reports {
                            // db_id → uid → полные данные (uid стабилен с открытия).
                            // Если db_id ещё не успели замапить — сканируем ТЕКУЩИЙ
                            // снапшот: ордер часто ещё в модели с присвоенным db_id,
                            // а его полные данные уже есть в индексе по uid.
                            let m = orders_index.by_dbid(r.db_id as i32).or_else(|| {
                                client
                                    .snapshot()
                                    .and_then(|snap| {
                                        snap.orders()
                                            .iter()
                                            .find(|o| o.db_id as i64 == r.db_id)
                                            .map(|o| o.uid)
                                    })
                                    .and_then(|uid| orders_index.by_uid(uid))
                            });
                            send_close_report(tx_db, server, r.db_id, r.sql, m);
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
                        orders_index.remember(
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
                            orders_index.map_dbid(o.db_id, o.uid);
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
