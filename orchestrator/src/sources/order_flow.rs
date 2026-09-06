use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc::Sender, watch};
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::sources::maker_entry_research::{
    MakerBook, MakerEntryResearch, MakerFeatures, MakerSignal, MakerTrade,
};
use crate::sources::{best_level, SignalSource};
use crate::symbol_universe::{record_edge, EdgeScores};
use crate::types::{
    next_signal_id, Direction, ExecutionMode, ExecutionReport, ExecutionStatus, Market,
    Opportunity, Strategy,
};

const BYBIT_LINEAR_WS_URL: &str = "wss://stream.bybit.com/v5/public/linear";
const TAKER_FEE_PER_SIDE: f64 = 0.00055;
const UNKNOWN_TAKER_FEE_PER_SIDE: f64 = 0.00110;
const STANDARD_MAKER_FEE_PER_SIDE: f64 = 0.00020;
const SPECIAL_MAKER_FEE_PER_SIDE: f64 = 0.00040;
#[cfg(test)]
const ROUND_TRIP_FEE: f64 = 2.0 * TAKER_FEE_PER_SIDE;
const SIGNAL_MODEL: &str =
    "top_of_book_event_ofi_v6_exchange_clock_only_feature_labeled_public_fee_class";
const HOLDING_HORIZONS_SECS: [u64; 4] = [5, 15, 30, 60];
const LEGACY_HOLDING_HORIZON_SECS: u64 = 5;
const TRADE_FLOW_WINDOW: Duration = Duration::from_secs(1);
const BOOK_EVENT_FLOW_WINDOW: Duration = Duration::from_secs(1);
const BOOK_MAX_AGE: Duration = Duration::from_millis(3_500);
const EMIT_COOLDOWN: Duration = Duration::from_secs(1);
const MIN_HISTORY_PER_SYMBOL: usize = 200;
const HISTORY_CAP_PER_SYMBOL: usize = 500;
const MIN_BOOK_EVENT_IMBALANCE: f64 = 0.10;
const MIN_BOOK_EVENTS: usize = 3;
const MIN_TRADE_IMBALANCE: f64 = 0.55;
const MIN_FLOW_NOTIONAL_USD: f64 = 2_000.0;
const MIN_TOP_DEPTH_USD: f64 = 50.0;
const MIN_CONSERVATIVE_EDGE: f64 = 0.00010;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
const RAW_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default)]
struct TopOfBook {
    bid: f64,
    bid_qty: f64,
    ask: f64,
    ask_qty: f64,
    updated_at: Option<Instant>,
}

impl TopOfBook {
    fn is_executable(&self) -> bool {
        self.bid > 0.0
            && self.ask > self.bid
            && self.bid_qty > 0.0
            && self.ask_qty > 0.0
            && self
                .updated_at
                .map(|t| t.elapsed() <= BOOK_MAX_AGE)
                .unwrap_or(false)
    }

    fn imbalance(&self) -> f64 {
        let total = self.bid_qty + self.ask_qty;
        if total <= 0.0 {
            0.0
        } else {
            (self.bid_qty - self.ask_qty) / total
        }
    }

    fn entry(&self, direction: Direction) -> (f64, f64) {
        match direction {
            Direction::Long => (self.ask, self.ask_qty * self.ask),
            Direction::Short => (self.bid, self.bid_qty * self.bid),
        }
    }

    fn exit(&self, direction: Direction) -> (f64, f64) {
        match direction {
            Direction::Long => (self.bid, self.bid_qty * self.bid),
            Direction::Short => (self.ask, self.ask_qty * self.ask),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TradePrint {
    exchange_ts_ms: u64,
    signed_notional: f64,
}

#[derive(Debug, Clone, Copy)]
struct BookEventPrint {
    exchange_ts_ms: u64,
    normalized_ofi: f64,
}

#[derive(Debug)]
struct PendingSignal {
    symbol: String,
    horizon_secs: u64,
    direction: Direction,
    entry_price: f64,
    entry_capacity: f64,
    round_trip_fee: f64,
    direction_sign: i8,
    signal_strength: f64,
    book_event_imbalance: f64,
    book_event_count: usize,
    trade_imbalance: f64,
    trade_notional_usd: f64,
    queue_imbalance: f64,
    spread_bps: f64,
    signal_id: Option<u64>,
    fired_at: Instant,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct MicroSample {
    observed_at_ms: u64,
    gross_return: f64,
    net_return: f64,
    round_trip_fee: f64,
    direction_sign: i8,
    signal_strength: f64,
    book_event_imbalance: f64,
    book_event_count: usize,
    trade_imbalance: f64,
    trade_notional_usd: f64,
    queue_imbalance: f64,
    spread_bps: f64,
    entry_price: f64,
    exit_price: f64,
    capacity_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedHistory(HashMap<String, Vec<MicroSample>>);

#[derive(Debug, Clone, Copy, Serialize)]
struct ValidationStats {
    train_edge_lcb: f64,
    validation_edge_lcb: f64,
    validation_without_best_lcb: f64,
    edge_lcb: f64,
    win_lcb: f64,
    stress_loss: f64,
}

#[derive(Debug, Serialize)]
struct ValidationRow {
    symbol: String,
    horizon_secs: u64,
    samples: usize,
    approved: bool,
    stats: Option<ValidationStats>,
}

#[derive(Debug, Serialize)]
struct ValidationReport {
    schema: &'static str,
    signal_model: &'static str,
    generated_at_ms: u64,
    minimum_samples: usize,
    minimum_edge_lcb: f64,
    standard_taker_fee_per_side: f64,
    unknown_taker_fee_per_side: f64,
    fee_model: &'static str,
    rows: Vec<ValidationRow>,
    research_only_strength_cohorts: Vec<FeatureCohortRow>,
}

#[derive(Debug, Serialize)]
struct FeatureCohortRow {
    horizon_secs: u64,
    minimum_signal_strength: f64,
    samples: usize,
    mean_gross_return: Option<f64>,
    mean_net_return: Option<f64>,
    net_edge_lcb95: Option<f64>,
    net_win_rate: Option<f64>,
}

type Histories = HashMap<String, VecDeque<MicroSample>>;

fn history_path() -> String {
    format!(
        "{}/data/confirmation_history_microstructure_v9.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn validation_path() -> String {
    format!(
        "{}/data/strategy_validation_microstructure_v9.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn history_key(symbol: &str, horizon_secs: u64) -> String {
    format!("{symbol}@{horizon_secs}s")
}

fn parse_history_key(key: &str) -> (&str, u64) {
    let Some((symbol, raw_horizon)) = key.rsplit_once('@') else {
        return (key, LEGACY_HOLDING_HORIZON_SECS);
    };
    let horizon_secs = raw_horizon
        .strip_suffix('s')
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(LEGACY_HOLDING_HORIZON_SECS);
    (symbol, horizon_secs)
}

fn load_histories() -> Histories {
    let Ok(raw) = std::fs::read_to_string(history_path()) else {
        return HashMap::new();
    };
    let Ok(PersistedHistory(saved)) = serde_json::from_str(&raw) else {
        tracing::warn!("histórico de microestrutura inválido; iniciando amostra limpa");
        return HashMap::new();
    };
    saved
        .into_iter()
        .map(|(key, values)| (key, values.into()))
        .collect()
}

fn save_histories(histories: &Histories) {
    let saved = PersistedHistory(
        histories
            .iter()
            .map(|(symbol, values)| (symbol.clone(), values.iter().copied().collect()))
            .collect(),
    );
    let Ok(json) = serde_json::to_string(&saved) else {
        return;
    };
    let path = history_path();
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, json);
    save_validation_report(histories);
}

fn save_validation_report(histories: &Histories) {
    let mut rows: Vec<ValidationRow> = histories
        .iter()
        .map(|(key, values)| {
            let (symbol, horizon_secs) = parse_history_key(key);
            let stats = conservative_edge(values);
            ValidationRow {
                symbol: symbol.to_string(),
                horizon_secs,
                samples: values.len(),
                approved: stats
                    .map(|item| item.edge_lcb > MIN_CONSERVATIVE_EDGE)
                    .unwrap_or(false),
                stats,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.symbol
            .cmp(&b.symbol)
            .then_with(|| a.horizon_secs.cmp(&b.horizon_secs))
    });
    let report = ValidationReport {
        schema: "aurumos.microstructure.validation.v9",
        signal_model: SIGNAL_MODEL,
        generated_at_ms: unix_now_ms(),
        minimum_samples: MIN_HISTORY_PER_SYMBOL,
        minimum_edge_lcb: MIN_CONSERVATIVE_EDGE,
        standard_taker_fee_per_side: TAKER_FEE_PER_SIDE,
        unknown_taker_fee_per_side: UNKNOWN_TAKER_FEE_PER_SIDE,
        fee_model: "public_bybit_fee_group_vip0_with_innovation_prelisting_and_unknown_max",
        rows,
        research_only_strength_cohorts: strength_cohorts(histories),
    };
    let Ok(json) = serde_json::to_string_pretty(&report) else {
        return;
    };
    let _ = std::fs::write(validation_path(), json);
}

fn strength_cohorts(histories: &Histories) -> Vec<FeatureCohortRow> {
    const THRESHOLDS: [f64; 5] = [0.10, 0.25, 0.50, 0.75, 0.90];
    let mut rows = Vec::new();
    for horizon_secs in HOLDING_HORIZONS_SECS {
        for minimum_signal_strength in THRESHOLDS {
            let samples: Vec<&MicroSample> = histories
                .iter()
                .filter(|(key, _)| parse_history_key(key).1 == horizon_secs)
                .flat_map(|(_, history)| history.iter())
                .filter(|sample| sample.signal_strength >= minimum_signal_strength)
                .collect();
            let gross: Vec<f64> = samples.iter().map(|sample| sample.gross_return).collect();
            let net: Vec<f64> = samples.iter().map(|sample| sample.net_return).collect();
            let mean_gross_return = arithmetic_mean(&gross);
            let mean_net_return = arithmetic_mean(&net);
            let net_edge_lcb95 = mean_lower_bound(&net);
            let net_win_rate = (!net.is_empty()).then(|| {
                net.iter().filter(|return_| **return_ > 0.0).count() as f64 / net.len() as f64
            });
            rows.push(FeatureCohortRow {
                horizon_secs,
                minimum_signal_strength,
                samples: samples.len(),
                mean_gross_return,
                mean_net_return,
                net_edge_lcb95,
                net_win_rate,
            });
        }
    }
    rows
}

/// Scanner direcional de microestrutura em uma única corretora. Ele cruza
/// OFI dinâmico de alterações do topo do book com agressão efetivamente
/// negociada no último segundo. Cada candidato é medido em 5, 15, 30 e 60 segundos pelo
/// preço de entrada/saída taker. Apenas o próprio par símbolo+horizonte,
/// após 200 janelas executáveis e aprovação em treino/validação
/// cronológicos, pode alimentar o executor selecionado.
pub struct OrderFlowSource {
    pub symbols_rx: watch::Receiver<Vec<String>>,
    pub bus: EventBus,
    pub edge_scores: EdgeScores,
    pub taker_fee_rates: HashMap<String, f64>,
    pub execution_tx: Sender<ExecutionReport>,
    pub publish_shadow_reports: bool,
}

impl OrderFlowSource {
    pub fn new(
        symbols_rx: watch::Receiver<Vec<String>>,
        bus: EventBus,
        edge_scores: EdgeScores,
        taker_fee_rates: HashMap<String, f64>,
        execution_tx: Sender<ExecutionReport>,
        publish_shadow_reports: bool,
    ) -> Self {
        Self {
            symbols_rx,
            bus,
            edge_scores,
            taker_fee_rates,
            execution_tx,
            publish_shadow_reports,
        }
    }
}

#[async_trait::async_trait]
impl SignalSource for OrderFlowSource {
    fn name(&self) -> &'static str {
        "microstructure_bybit_linear"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        let mut histories = load_histories();
        let mut pending = Vec::new();
        let mut maker_research = MakerEntryResearch::new();
        loop {
            let symbols = self.symbols_rx.borrow().clone();
            tokio::select! {
                result = run_once(
                    &symbols,
                    &tx,
                    &self.execution_tx,
                    &self.bus,
                    &self.edge_scores,
                    &self.taker_fee_rates,
                    &mut histories,
                    &mut pending,
                    &mut maker_research,
                    self.publish_shadow_reports,
                ) => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "stream de microestrutura caiu; reconectando em 3s");
                    }
                    sleep(Duration::from_secs(3)).await;
                }
                _ = self.symbols_rx.changed() => {
                    tracing::info!("microestrutura: universo linear atualizado; reconectando");
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    symbols: &[String],
    tx: &Sender<Opportunity>,
    execution_tx: &Sender<ExecutionReport>,
    bus: &EventBus,
    edge_scores: &EdgeScores,
    taker_fee_rates: &HashMap<String, f64>,
    histories: &mut Histories,
    pending: &mut Vec<PendingSignal>,
    maker_research: &mut MakerEntryResearch,
    publish_shadow_reports: bool,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(BYBIT_LINEAR_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    let args: Vec<String> = symbols
        .iter()
        .flat_map(|symbol| {
            [
                format!("orderbook.1.{symbol}"),
                format!("publicTrade.{symbol}"),
            ]
        })
        .collect();
    for chunk in args.chunks(10) {
        sink.send(WsMessage::Text(
            serde_json::json!({"op":"subscribe", "args":chunk}).to_string(),
        ))
        .await?;
    }

    tracing::info!(
        exchange = "bybit_linear",
        symbols = symbols.len(),
        channels = args.len(),
        "scanner de microestrutura conectado"
    );

    let mut books: HashMap<String, TopOfBook> = HashMap::new();
    let mut book_events: HashMap<String, VecDeque<BookEventPrint>> = HashMap::new();
    let mut trades: HashMap<String, VecDeque<TradePrint>> = HashMap::new();
    let mut last_emitted: HashMap<String, Instant> = HashMap::new();
    let mut best_signal: Option<(String, f64)> = None;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut confirmation = tokio::time::interval(Duration::from_millis(100));
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut save = tokio::time::interval(Duration::from_secs(30));
    let mut raw_snapshot = tokio::time::interval(RAW_SNAPSHOT_INTERVAL);
    let mut last_message = Instant::now();
    let raw_path = format!(
        "{}/data/raw_microstructure_v9.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );

    loop {
        tokio::select! {
            _ = ping.tick() => {
                sink.send(WsMessage::Text(serde_json::json!({"op":"ping"}).to_string())).await?;
            }
            _ = watchdog.tick() => {
                if last_message.elapsed() > Duration::from_secs(25) {
                    anyhow::bail!("stream sem mensagens por 25s");
                }
            }
            _ = confirmation.tick() => {
                resolve_due(&books, histories, pending, execution_tx, edge_scores).await;
                maker_research.tick(&maker_books(&books));
            }
            _ = save.tick() => {
                save_histories(histories);
                maker_research.save();
            },
            _ = heartbeat.tick() => {
                if let Some((symbol, strength)) = best_signal.take() {
                    bus.emit(DashboardEvent::scan_heartbeat(
                        Strategy::OrderFlow,
                        symbols.len() as u32,
                        &symbol,
                        strength * 100.0,
                    ));
                }
            }
            _ = raw_snapshot.tick() => {
                let now = crate::raw_log::now_ms();
                for (symbol, book) in &books {
                    let exchange_now = latest_exchange_timestamp(
                        book_events.get(symbol),
                        trades.get(symbol),
                        now,
                    );
                    let (flow_imbalance, flow_notional) = flow_metrics(trades.entry(symbol.clone()).or_default(), exchange_now);
                    let (book_event_imbalance, book_event_count) = book_event_metrics(book_events.entry(symbol.clone()).or_default(), exchange_now);
                    crate::raw_log::append(&raw_path, serde_json::json!({
                        "ts_ms": now,
                        "signal_model": SIGNAL_MODEL,
                        "symbol": symbol,
                        "bid": book.bid,
                        "ask": book.ask,
                        "bid_qty": book.bid_qty,
                        "ask_qty": book.ask_qty,
                        "book_imbalance": book.imbalance(),
                        "book_event_imbalance_1s": book_event_imbalance,
                        "book_event_count_1s": book_event_count,
                        "trade_imbalance_1s": flow_imbalance,
                        "trade_notional_1s": flow_notional,
                        "taker_fee_per_side": taker_fee_rate(taker_fee_rates, symbol),
                    }));
                }
            }
            message = stream.next() => {
                last_message = Instant::now();
                match message {
                    Some(Ok(WsMessage::Text(text))) => {
                        handle_message(
                            &text,
                            &mut books,
                            &mut book_events,
                            &mut trades,
                            &mut last_emitted,
                            &mut best_signal,
                            histories,
                            taker_fee_rates,
                            maker_research,
                            pending,
                            tx,
                            publish_shadow_reports,
                        ).await;
                    }
                    Some(Ok(WsMessage::Ping(payload))) => sink.send(WsMessage::Pong(payload)).await?,
                    Some(Ok(WsMessage::Close(_))) | None => anyhow::bail!("stream encerrado"),
                    Some(Err(error)) => anyhow::bail!("erro no websocket: {error}"),
                    _ => {}
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_message(
    text: &str,
    books: &mut HashMap<String, TopOfBook>,
    book_events: &mut HashMap<String, VecDeque<BookEventPrint>>,
    trades: &mut HashMap<String, VecDeque<TradePrint>>,
    last_emitted: &mut HashMap<String, Instant>,
    best_signal: &mut Option<(String, f64)>,
    histories: &Histories,
    taker_fee_rates: &HashMap<String, f64>,
    maker_research: &mut MakerEntryResearch,
    pending: &mut Vec<PendingSignal>,
    tx: &Sender<Opportunity>,
    publish_shadow_reports: bool,
) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if value.get("success") == Some(&Value::Bool(false)) {
        tracing::warn!(raw = %text.chars().take(400).collect::<String>(), "Bybit rejeitou assinatura/mensagem");
        return;
    }
    let Some(topic) = value.get("topic").and_then(Value::as_str) else {
        return;
    };

    let (symbol, exchange_now_ms) = if topic.starts_with("orderbook.") {
        let Some(data) = value.get("data") else {
            return;
        };
        let Some(symbol) = data.get("s").and_then(Value::as_str) else {
            return;
        };
        let exchange_ts_ms = integer_value(data, "cts")
            .or_else(|| integer_value(&value, "ts"))
            .unwrap_or_else(unix_now_ms);
        let Some(bid) = best_level(data.get("b")) else {
            return;
        };
        let Some(ask) = best_level(data.get("a")) else {
            return;
        };
        let next_book = TopOfBook {
            bid: bid.0,
            bid_qty: bid.1,
            ask: ask.0,
            ask_qty: ask.1,
            updated_at: Some(Instant::now()),
        };
        if let Some(previous) = books.get(symbol).copied() {
            if let Some(normalized_ofi) = normalized_book_event_ofi(previous, next_book) {
                if normalized_ofi.abs() > f64::EPSILON {
                    let queue = book_events.entry(symbol.to_string()).or_default();
                    queue.push_back(BookEventPrint {
                        exchange_ts_ms,
                        normalized_ofi,
                    });
                    prune_book_events(queue, exchange_ts_ms);
                }
            }
        }
        books.insert(symbol.to_string(), next_book);
        (symbol.to_string(), exchange_ts_ms)
    } else if topic.starts_with("publicTrade.") {
        let symbol = topic.rsplit('.').next().unwrap_or_default().to_string();
        let queue = trades.entry(symbol.clone()).or_default();
        let mut exchange_now_ms = integer_value(&value, "ts").unwrap_or_else(unix_now_ms);
        if let Some(items) = value.get("data").and_then(Value::as_array) {
            for trade in items {
                let price = trade
                    .get("p")
                    .and_then(Value::as_str)
                    .and_then(|v| v.parse::<f64>().ok());
                let qty = trade
                    .get("v")
                    .and_then(Value::as_str)
                    .and_then(|v| v.parse::<f64>().ok());
                let side = trade.get("S").and_then(Value::as_str);
                if let (Some(price), Some(qty), Some(side)) = (price, qty, side) {
                    let trade_ts_ms = integer_value(trade, "T").unwrap_or(exchange_now_ms);
                    exchange_now_ms = exchange_now_ms.max(trade_ts_ms);
                    let (sign, aggressor) = if side.eq_ignore_ascii_case("Buy") {
                        (1.0, Direction::Long)
                    } else {
                        (-1.0, Direction::Short)
                    };
                    maker_research.on_trade(
                        &symbol,
                        MakerTrade {
                            aggressor,
                            price,
                            qty,
                        },
                    );
                    queue.push_back(TradePrint {
                        exchange_ts_ms: trade_ts_ms,
                        signed_notional: sign * price * qty,
                    });
                }
            }
        }
        prune_flow(queue, exchange_now_ms);
        (symbol, exchange_now_ms)
    } else {
        return;
    };

    maybe_emit(
        &symbol,
        exchange_now_ms,
        books,
        book_events,
        trades,
        last_emitted,
        best_signal,
        histories,
        taker_fee_rates,
        maker_research,
        pending,
        tx,
        publish_shadow_reports,
    )
    .await;
}

fn integer_value(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(|raw| {
        raw.as_u64()
            .or_else(|| raw.as_str().and_then(|text| text.parse::<u64>().ok()))
    })
}

fn latest_exchange_timestamp(
    book_events: Option<&VecDeque<BookEventPrint>>,
    trades: Option<&VecDeque<TradePrint>>,
    fallback_ms: u64,
) -> u64 {
    book_events
        .and_then(VecDeque::back)
        .map(|event| event.exchange_ts_ms)
        .into_iter()
        .chain(
            trades
                .and_then(VecDeque::back)
                .map(|trade| trade.exchange_ts_ms),
        )
        .max()
        .unwrap_or(fallback_ms)
}

fn taker_fee_rate(rates: &HashMap<String, f64>, symbol: &str) -> f64 {
    rates
        .get(symbol)
        .copied()
        .filter(|rate| rate.is_finite() && *rate > 0.0 && *rate < 0.01)
        .unwrap_or(UNKNOWN_TAKER_FEE_PER_SIDE)
}

fn maker_fee_rate(taker_fee_per_side: f64) -> f64 {
    if taker_fee_per_side >= 0.001 {
        SPECIAL_MAKER_FEE_PER_SIDE
    } else {
        STANDARD_MAKER_FEE_PER_SIDE
    }
}

fn maker_books(books: &HashMap<String, TopOfBook>) -> HashMap<String, MakerBook> {
    books
        .iter()
        .map(|(symbol, book)| {
            (
                symbol.clone(),
                MakerBook {
                    bid: book.bid,
                    bid_qty: book.bid_qty,
                    bid_notional: book.bid * book.bid_qty,
                    ask: book.ask,
                    ask_qty: book.ask_qty,
                    ask_notional: book.ask * book.ask_qty,
                    executable: book.is_executable(),
                },
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn maybe_emit(
    symbol: &str,
    exchange_now_ms: u64,
    books: &HashMap<String, TopOfBook>,
    book_events: &mut HashMap<String, VecDeque<BookEventPrint>>,
    trades: &mut HashMap<String, VecDeque<TradePrint>>,
    last_emitted: &mut HashMap<String, Instant>,
    best_signal: &mut Option<(String, f64)>,
    histories: &Histories,
    taker_fee_rates: &HashMap<String, f64>,
    maker_research: &mut MakerEntryResearch,
    pending: &mut Vec<PendingSignal>,
    tx: &Sender<Opportunity>,
    publish_shadow_reports: bool,
) {
    let Some(book) = books.get(symbol).copied() else {
        return;
    };
    if !book.is_executable() {
        return;
    }
    let (flow_imbalance, flow_notional) = flow_metrics(
        trades.entry(symbol.to_string()).or_default(),
        exchange_now_ms,
    );
    let (book_event_imbalance, book_event_count) = book_event_metrics(
        book_events.entry(symbol.to_string()).or_default(),
        exchange_now_ms,
    );
    let queue_imbalance = book.imbalance();
    let strength = book_event_imbalance.abs().min(flow_imbalance.abs());
    if best_signal
        .as_ref()
        .map(|(_, old)| strength > *old)
        .unwrap_or(true)
    {
        *best_signal = Some((symbol.to_string(), strength));
    }

    if book_event_count < MIN_BOOK_EVENTS
        || book_event_imbalance.abs() < MIN_BOOK_EVENT_IMBALANCE
        || flow_imbalance.abs() < MIN_TRADE_IMBALANCE
        || flow_notional < MIN_FLOW_NOTIONAL_USD
        || book_event_imbalance.signum() != flow_imbalance.signum()
    {
        return;
    }
    if last_emitted
        .get(symbol)
        .map(|at| at.elapsed() < EMIT_COOLDOWN)
        .unwrap_or(false)
    {
        return;
    }
    let available_horizons: Vec<u64> = HOLDING_HORIZONS_SECS
        .iter()
        .copied()
        .filter(|horizon_secs| {
            !pending
                .iter()
                .any(|item| item.symbol == symbol && item.horizon_secs == *horizon_secs)
        })
        .collect();
    if available_horizons.is_empty() {
        return;
    }

    let direction = if flow_imbalance > 0.0 {
        Direction::Long
    } else {
        Direction::Short
    };
    let (entry_price, entry_capacity) = book.entry(direction);
    if entry_capacity < MIN_TOP_DEPTH_USD {
        return;
    }
    let round_trip_fee = 2.0 * taker_fee_rate(taker_fee_rates, symbol);
    let midpoint = (book.bid + book.ask) / 2.0;
    let spread_bps = (book.ask - book.bid) / midpoint * 10_000.0;

    let assessments: Vec<(u64, usize, Option<ValidationStats>)> = available_horizons
        .iter()
        .map(|&horizon_secs| {
            let key = history_key(symbol, horizon_secs);
            let history = histories.get(&key);
            (
                horizon_secs,
                history.map(VecDeque::len).unwrap_or(0),
                history.and_then(conservative_edge),
            )
        })
        .collect();
    let &(selected_horizon_secs, selected_samples, stats) = assessments
        .iter()
        .max_by(|a, b| {
            let a_approved =
                a.2.is_some_and(|stats| stats.edge_lcb > MIN_CONSERVATIVE_EDGE);
            let b_approved =
                b.2.is_some_and(|stats| stats.edge_lcb > MIN_CONSERVATIVE_EDGE);
            a_approved
                .cmp(&b_approved)
                .then_with(|| {
                    a.2.map(|stats| stats.edge_lcb)
                        .unwrap_or(f64::NEG_INFINITY)
                        .total_cmp(&b.2.map(|stats| stats.edge_lcb).unwrap_or(f64::NEG_INFINITY))
                })
                .then_with(|| a.1.cmp(&b.1))
        })
        .expect("há pelo menos um horizonte disponível");
    let (net_edge, confidence, stress_loss_pct, execution_mode, evidence) = match stats {
        Some(stats) if stats.edge_lcb > MIN_CONSERVATIVE_EDGE => (
            stats.edge_lcb,
            stats.win_lcb,
            stats.stress_loss,
            ExecutionMode::ExecutableQuoted,
            format!(
                "{}s; LCB95 OOS {:+.3}% em {} amostras; taxa RT {:.3}%",
                selected_horizon_secs,
                stats.edge_lcb * 100.0,
                selected_samples,
                round_trip_fee * 100.0
            ),
        ),
        Some(stats) => (
            0.0,
            stats.win_lcb,
            stats.stress_loss,
            ExecutionMode::ObservationOnly,
            format!(
                "{}s; LCB95 OOS {:+.3}% ainda sem vantagem; taxa RT {:.3}%",
                selected_horizon_secs,
                stats.edge_lcb * 100.0,
                round_trip_fee * 100.0
            ),
        ),
        None => (
            0.0,
            0.0,
            0.005,
            ExecutionMode::ObservationOnly,
            format!(
                "{}s; calibrando {}/{} amostras do próprio símbolo+horizonte; taxa RT {:.3}%",
                selected_horizon_secs,
                selected_samples,
                MIN_HISTORY_PER_SYMBOL,
                round_trip_fee * 100.0
            ),
        ),
    };
    let signal_id = next_signal_id();
    let fired_at = Instant::now();
    pending.extend(available_horizons.into_iter().map(|horizon_secs| {
        PendingSignal {
            symbol: symbol.to_string(),
            horizon_secs,
            direction,
            entry_price,
            entry_capacity,
            round_trip_fee,
            direction_sign: if direction == Direction::Long { 1 } else { -1 },
            signal_strength: strength,
            book_event_imbalance,
            book_event_count,
            trade_imbalance: flow_imbalance,
            trade_notional_usd: flow_notional,
            queue_imbalance,
            spread_bps,
            signal_id: (publish_shadow_reports
                && execution_mode == ExecutionMode::ExecutableQuoted
                && horizon_secs == selected_horizon_secs)
                .then_some(signal_id),
            fired_at,
        }
    }));
    let maker_price = match direction {
        Direction::Long => book.bid,
        Direction::Short => book.ask,
    };
    let taker_fee_per_side = taker_fee_rate(taker_fee_rates, symbol);
    maker_research.start(MakerSignal {
        symbol: symbol.to_string(),
        direction,
        price: maker_price,
        order_notional_usd: entry_capacity.min(25.0),
        maker_fee_per_side: maker_fee_rate(taker_fee_per_side),
        taker_fee_per_side,
        features: MakerFeatures {
            signal_strength: strength,
            book_event_imbalance,
            book_event_count,
            trade_imbalance: flow_imbalance,
            trade_notional_usd: flow_notional,
            queue_imbalance,
            spread_bps,
        },
    });
    last_emitted.insert(symbol.to_string(), Instant::now());

    let opportunity = Opportunity {
        signal_id,
        market: Market::Crypto,
        strategy: Strategy::OrderFlow,
        asset: format!(
            "{symbol} [Bybit linear; OFI {book_event_imbalance:+.2}; fila {queue_imbalance:+.2}; trades {flow_imbalance:+.2}; {evidence}]"
        ),
        direction,
        net_edge,
        confidence,
        valid_for_ms: 500,
        expected_holding_secs: selected_horizon_secs as f64,
        capital_needed: entry_capacity.min(25.0),
        reference_price: Some(entry_price),
        max_loss_pct: stress_loss_pct,
        leverage: 1.0,
        correlation_group: symbol.to_string(),
        execution_mode,
        capital_multiplier: 1.0,
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opportunity).await;
}

async fn resolve_due(
    books: &HashMap<String, TopOfBook>,
    histories: &mut Histories,
    pending: &mut Vec<PendingSignal>,
    execution_tx: &Sender<ExecutionReport>,
    edge_scores: &EdgeScores,
) {
    let mut due = Vec::new();
    let mut waiting = Vec::with_capacity(pending.len());
    for item in pending.drain(..) {
        if item.fired_at.elapsed() >= Duration::from_secs(item.horizon_secs) {
            due.push(item);
        } else {
            waiting.push(item);
        }
    }
    *pending = waiting;

    for item in due {
        let latency_ms = item.fired_at.elapsed().as_millis() as u64;
        let (status, return_pct, capacity, note, sample) =
            match books.get(&item.symbol).filter(|b| b.is_executable()) {
                Some(book) => {
                    let (exit_price, exit_capacity) = book.exit(item.direction);
                    let gross_return = match item.direction {
                        Direction::Long => (exit_price - item.entry_price) / item.entry_price,
                        Direction::Short => (item.entry_price - exit_price) / item.entry_price,
                    };
                    let capacity = item.entry_capacity.min(exit_capacity);
                    let net_return = gross_return - item.round_trip_fee;
                    (
                        ExecutionStatus::Filled,
                        net_return,
                        capacity,
                        format!(
                            "entrada_e_saida_taker_no_topo_do_book_{}s",
                            item.horizon_secs
                        ),
                        Some(MicroSample {
                            observed_at_ms: unix_now_ms(),
                            gross_return,
                            net_return,
                            round_trip_fee: item.round_trip_fee,
                            direction_sign: item.direction_sign,
                            signal_strength: item.signal_strength,
                            book_event_imbalance: item.book_event_imbalance,
                            book_event_count: item.book_event_count,
                            trade_imbalance: item.trade_imbalance,
                            trade_notional_usd: item.trade_notional_usd,
                            queue_imbalance: item.queue_imbalance,
                            spread_bps: item.spread_bps,
                            entry_price: item.entry_price,
                            exit_price,
                            capacity_usd: capacity,
                        }),
                    )
                }
                None => (
                    ExecutionStatus::Unfilled,
                    0.0,
                    0.0,
                    format!("book_indisponivel_na_saida_{}s", item.horizon_secs),
                    None,
                ),
            };

        if let Some(sample) = sample {
            // O ranking do universo recebe o resultado observado uma única
            // vez quando a janela vence. Ele só direciona capacidade de
            // coleta; autorização de ordem continua no LCB95 de 200
            // amostras do mesmo símbolo+horizonte.
            record_edge(edge_scores, &item.symbol, sample.net_return);
            let history = histories
                .entry(history_key(&item.symbol, item.horizon_secs))
                .or_default();
            history.push_back(sample);
            while history.len() > HISTORY_CAP_PER_SYMBOL {
                history.pop_front();
            }
        }

        if let Some(signal_id) = item.signal_id {
            let _ = execution_tx
                .send(ExecutionReport {
                    signal_id,
                    status,
                    return_pct,
                    realized_pnl: None,
                    max_executable_notional: capacity,
                    observed_latency_ms: latency_ms,
                    note,
                })
                .await;
        }
    }
}

/// Implementa o OFI do melhor bid/ask de Cont, Kukanov e Stoikov. A
/// contribuição usa mudanças de preço e quantidade nos dois lados e é
/// normalizada pela profundidade média para permitir comparação entre
/// instrumentos de escalas diferentes.
fn normalized_book_event_ofi(previous: TopOfBook, next: TopOfBook) -> Option<f64> {
    let values = [
        previous.bid,
        previous.bid_qty,
        previous.ask,
        previous.ask_qty,
        next.bid,
        next.bid_qty,
        next.ask,
        next.ask_qty,
    ];
    if values
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return None;
    }
    let bid_flow = if next.bid >= previous.bid {
        next.bid_qty
    } else {
        0.0
    } - if next.bid <= previous.bid {
        previous.bid_qty
    } else {
        0.0
    };
    let ask_flow = -(if next.ask <= previous.ask {
        next.ask_qty
    } else {
        0.0
    }) + if next.ask >= previous.ask {
        previous.ask_qty
    } else {
        0.0
    };
    let average_total_depth =
        (previous.bid_qty + previous.ask_qty + next.bid_qty + next.ask_qty) / 2.0;
    (average_total_depth > 0.0)
        .then(|| ((bid_flow + ask_flow) / average_total_depth).clamp(-4.0, 4.0))
}

fn prune_book_events(events: &mut VecDeque<BookEventPrint>, now_ms: u64) {
    while events
        .front()
        .map(|event| {
            now_ms.saturating_sub(event.exchange_ts_ms) > BOOK_EVENT_FLOW_WINDOW.as_millis() as u64
        })
        .unwrap_or(false)
    {
        events.pop_front();
    }
}

fn book_event_metrics(events: &mut VecDeque<BookEventPrint>, now_ms: u64) -> (f64, usize) {
    prune_book_events(events, now_ms);
    let count = events.len();
    if count == 0 {
        return (0.0, 0);
    }
    let normalized_sum = events.iter().map(|event| event.normalized_ofi).sum::<f64>();
    // A raiz de N evita que apenas a taxa de mensagens defina o sinal;
    // tanh limita choques de preço/quantidade sem apagar o sinal.
    ((normalized_sum / (count as f64).sqrt()).tanh(), count)
}

fn prune_flow(flow: &mut VecDeque<TradePrint>, now_ms: u64) {
    while flow
        .front()
        .map(|trade| {
            now_ms.saturating_sub(trade.exchange_ts_ms) > TRADE_FLOW_WINDOW.as_millis() as u64
        })
        .unwrap_or(false)
    {
        flow.pop_front();
    }
}

fn flow_metrics(flow: &mut VecDeque<TradePrint>, now_ms: u64) -> (f64, f64) {
    prune_flow(flow, now_ms);
    let signed = flow.iter().map(|trade| trade.signed_notional).sum::<f64>();
    let total = flow
        .iter()
        .map(|trade| trade.signed_notional.abs())
        .sum::<f64>();
    if total <= 0.0 {
        (0.0, 0.0)
    } else {
        (signed / total, total)
    }
}

/// Divide a série em ordem temporal. O edge aprovado é o menor LCB95 entre
/// treino, validação e validação sem o melhor trade; assim um outlier não
/// pode promover o símbolo sozinho.
fn conservative_edge(history: &VecDeque<MicroSample>) -> Option<ValidationStats> {
    if history.len() < MIN_HISTORY_PER_SYMBOL {
        return None;
    }
    let ordered: Vec<f64> = history.iter().map(|sample| sample.net_return).collect();
    let split = ordered.len() / 2;
    let train = &ordered[..split];
    let validation = &ordered[split..];
    let train_edge_lcb = mean_lower_bound(train)?;
    let validation_edge_lcb = mean_lower_bound(validation)?;
    let best_idx = validation
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(idx, _)| idx)?;
    let validation_without_best: Vec<f64> = validation
        .iter()
        .enumerate()
        .filter_map(|(idx, value)| (idx != best_idx).then_some(*value))
        .collect();
    let validation_without_best_lcb = mean_lower_bound(&validation_without_best)?;
    let edge_lcb = train_edge_lcb
        .min(validation_edge_lcb)
        .min(validation_without_best_lcb);
    let train_wins = train.iter().filter(|&&value| value > 0.0).count() as f64;
    let validation_wins = validation.iter().filter(|&&value| value > 0.0).count() as f64;
    let win_lcb = wilson_lower_bound(train_wins, train.len() as f64)
        .min(wilson_lower_bound(validation_wins, validation.len() as f64));
    let stress_loss = history
        .iter()
        .map(|sample| sample.net_return)
        .fold(0.0_f64, f64::min)
        .abs()
        .max(0.005);
    Some(ValidationStats {
        train_edge_lcb,
        validation_edge_lcb,
        validation_without_best_lcb,
        edge_lcb,
        win_lcb,
        stress_loss,
    })
}

fn mean_lower_bound(values: &[f64]) -> Option<f64> {
    if values.len() < 2 || values.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (n - 1.0);
    Some(mean - 1.96 * (variance / n).sqrt())
}

fn arithmetic_mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty() && values.iter().all(|value| value.is_finite()))
        .then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn wilson_lower_bound(wins: f64, total: f64) -> f64 {
    if total <= 0.0 {
        return 0.0;
    }
    const Z: f64 = 1.96;
    let p = wins / total;
    let z2 = Z * Z;
    let denominator = 1.0 + z2 / total;
    let center = p + z2 / (2.0 * total);
    let margin = Z * ((p * (1.0 - p) / total) + z2 / (4.0 * total * total)).sqrt();
    ((center - margin) / denominator).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(bid: f64, bid_qty: f64, ask: f64, ask_qty: f64) -> TopOfBook {
        TopOfBook {
            bid,
            bid_qty,
            ask,
            ask_qty,
            updated_at: Some(Instant::now()),
        }
    }

    fn sample(net_return: f64) -> MicroSample {
        MicroSample {
            observed_at_ms: 1,
            gross_return: net_return + ROUND_TRIP_FEE,
            net_return,
            round_trip_fee: ROUND_TRIP_FEE,
            direction_sign: 1,
            signal_strength: 0.8,
            book_event_imbalance: 0.8,
            book_event_count: 8,
            trade_imbalance: 0.9,
            trade_notional_usd: 10_000.0,
            queue_imbalance: 0.4,
            spread_bps: 1.0,
            entry_price: 100.0,
            exit_price: 100.0 * (1.0 + net_return + ROUND_TRIP_FEE),
            capacity_usd: 25.0,
        }
    }

    #[test]
    fn conservative_edge_requires_symbol_level_sample() {
        let history: VecDeque<MicroSample> =
            std::iter::repeat_n(sample(0.002), MIN_HISTORY_PER_SYMBOL - 1).collect();
        assert!(conservative_edge(&history).is_none());
    }

    #[test]
    fn fees_are_charged_on_entry_and_exit() {
        let entry = 100.0;
        let exit = 100.05;
        let net = (exit - entry) / entry - ROUND_TRIP_FEE;
        assert!(net < 0.0, "cinco bps brutos não cobrem onze bps de taxas");
    }

    #[test]
    fn dynamic_book_ofi_has_the_economic_direction_of_queue_events() {
        let base = book(100.0, 10.0, 101.0, 10.0);
        let stronger_bid = book(100.0, 20.0, 101.0, 10.0);
        let thinner_ask = book(100.0, 10.0, 101.0, 5.0);
        let lower_bid = book(99.0, 10.0, 101.0, 10.0);
        let lower_ask = book(100.0, 10.0, 100.5, 10.0);

        assert!(normalized_book_event_ofi(base, stronger_bid).unwrap() > 0.0);
        assert!(normalized_book_event_ofi(base, thinner_ask).unwrap() > 0.0);
        assert!(normalized_book_event_ofi(base, lower_bid).unwrap() < 0.0);
        assert!(normalized_book_event_ofi(base, lower_ask).unwrap() < 0.0);
    }

    #[test]
    fn exchange_timestamps_remove_stale_book_and_trade_events() {
        let now_ms = 10_000;
        let mut book_events = VecDeque::from([
            BookEventPrint {
                exchange_ts_ms: now_ms - 1_001,
                normalized_ofi: 1.0,
            },
            BookEventPrint {
                exchange_ts_ms: now_ms,
                normalized_ofi: -0.5,
            },
        ]);
        let mut trades = VecDeque::from([
            TradePrint {
                exchange_ts_ms: now_ms - 1_001,
                signed_notional: 10_000.0,
            },
            TradePrint {
                exchange_ts_ms: now_ms,
                signed_notional: -2_000.0,
            },
        ]);

        let (ofi, count) = book_event_metrics(&mut book_events, now_ms);
        let (trade_imbalance, trade_notional) = flow_metrics(&mut trades, now_ms);
        assert_eq!(count, 1);
        assert!(ofi < 0.0);
        assert_eq!(trade_imbalance, -1.0);
        assert_eq!(trade_notional, 2_000.0);
        assert_eq!(
            latest_exchange_timestamp(Some(&book_events), Some(&trades), now_ms + 60_000),
            now_ms,
            "o relógio local não pode substituir cts/T quando a exchange forneceu timestamp"
        );
    }

    #[test]
    fn profitable_lower_bound_needs_margin_over_noise() {
        let history: VecDeque<MicroSample> =
            std::iter::repeat_n(sample(0.002), MIN_HISTORY_PER_SYMBOL).collect();
        let stats = conservative_edge(&history).unwrap();
        assert!(stats.edge_lcb > 0.0019);
        assert!(stats.win_lcb > 0.96);
        assert_eq!(stats.stress_loss, 0.005);
    }

    #[test]
    fn one_validation_outlier_cannot_create_edge() {
        let mut history: VecDeque<MicroSample> =
            std::iter::repeat_n(sample(0.0002), MIN_HISTORY_PER_SYMBOL / 2).collect();
        history.extend(std::iter::repeat_n(
            sample(-0.0002),
            MIN_HISTORY_PER_SYMBOL / 2 - 1,
        ));
        history.push_back(sample(0.10));
        let stats = conservative_edge(&history).unwrap();
        assert!(stats.validation_without_best_lcb < 0.0);
        assert!(stats.edge_lcb < MIN_CONSERVATIVE_EDGE);
    }

    #[tokio::test]
    async fn horizons_resolve_independently_into_separate_histories() {
        let mut books = HashMap::new();
        books.insert(
            "BTCUSDT".to_string(),
            TopOfBook {
                bid: 100.10,
                bid_qty: 100.0,
                ask: 100.20,
                ask_qty: 100.0,
                updated_at: Some(Instant::now()),
            },
        );
        let fired_at = Instant::now() - Duration::from_secs(6);
        let mut pending = vec![
            PendingSignal {
                symbol: "BTCUSDT".to_string(),
                horizon_secs: 5,
                direction: Direction::Long,
                entry_price: 100.0,
                entry_capacity: 25.0,
                round_trip_fee: ROUND_TRIP_FEE,
                direction_sign: 1,
                signal_strength: 0.8,
                book_event_imbalance: 0.8,
                book_event_count: 8,
                trade_imbalance: 0.9,
                trade_notional_usd: 10_000.0,
                queue_imbalance: 0.4,
                spread_bps: 1.0,
                signal_id: None,
                fired_at,
            },
            PendingSignal {
                symbol: "BTCUSDT".to_string(),
                horizon_secs: 60,
                direction: Direction::Long,
                entry_price: 100.0,
                entry_capacity: 25.0,
                round_trip_fee: ROUND_TRIP_FEE,
                direction_sign: 1,
                signal_strength: 0.8,
                book_event_imbalance: 0.8,
                book_event_count: 8,
                trade_imbalance: 0.9,
                trade_notional_usd: 10_000.0,
                queue_imbalance: 0.4,
                spread_bps: 1.0,
                signal_id: None,
                fired_at,
            },
        ];
        let mut histories = Histories::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);

        let edge_scores = crate::symbol_universe::new_edge_scores();
        resolve_due(&books, &mut histories, &mut pending, &tx, &edge_scores).await;

        assert_eq!(histories[&history_key("BTCUSDT", 5)].len(), 1);
        assert!(!histories.contains_key(&history_key("BTCUSDT", 60)));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].horizon_secs, 60);
    }

    #[test]
    fn public_fee_class_overrides_unknown_conservative_fallback() {
        let rates = HashMap::from([
            ("BTCUSDT".to_string(), 0.0006),
            ("ETHUSDT".to_string(), f64::NAN),
        ]);
        assert_eq!(taker_fee_rate(&rates, "BTCUSDT"), 0.0006);
        assert_eq!(
            taker_fee_rate(&rates, "ETHUSDT"),
            UNKNOWN_TAKER_FEE_PER_SIDE
        );
        assert_eq!(
            taker_fee_rate(&rates, "SOLUSDT"),
            UNKNOWN_TAKER_FEE_PER_SIDE
        );
    }

    #[test]
    fn strength_cohorts_are_research_only_feature_slices() {
        let mut weak = sample(-0.001);
        weak.signal_strength = 0.2;
        let mut strong = sample(0.002);
        strong.signal_strength = 0.9;
        let histories =
            HashMap::from([(history_key("BTCUSDT", 5), VecDeque::from([weak, strong]))]);
        let cohorts = strength_cohorts(&histories);
        let all = cohorts
            .iter()
            .find(|row| row.horizon_secs == 5 && row.minimum_signal_strength == 0.10)
            .unwrap();
        let strongest = cohorts
            .iter()
            .find(|row| row.horizon_secs == 5 && row.minimum_signal_strength == 0.90)
            .unwrap();
        assert_eq!(all.samples, 2);
        assert_eq!(strongest.samples, 1);
        assert_eq!(strongest.mean_net_return, Some(0.002));
        assert!(strongest.net_edge_lcb95.is_none());
    }
}
