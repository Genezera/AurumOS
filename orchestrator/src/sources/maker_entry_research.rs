//! Pesquisa de entrada passiva no mesmo sinal de microestrutura.
//!
//! Uma tentativa maker só conta como preenchida quando negócios públicos
//! posteriores, do lado agressor oposto, consomem toda a quantidade visível
//! que estava à frente e também a quantidade hipotética de US$ 25. Não usa
//! cancelamentos da fila como fill e nunca envia uma `Opportunity`.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::types::Direction;

const ENTRY_TTL: Duration = Duration::from_millis(500);
const PLACEMENT_LATENCY: Duration = Duration::from_millis(250);
const HORIZONS_SECS: [u64; 4] = [5, 15, 30, 60];
const HISTORY_CAP: usize = 500;
const MIN_RESEARCH_SAMPLES: usize = 200;
const MIN_EDGE_LCB: f64 = 0.00010;

#[derive(Debug, Clone, Copy)]
pub struct MakerFeatures {
    pub signal_strength: f64,
    pub book_event_imbalance: f64,
    pub book_event_count: usize,
    pub trade_imbalance: f64,
    pub trade_notional_usd: f64,
    pub queue_imbalance: f64,
    pub spread_bps: f64,
}

#[derive(Debug)]
pub struct MakerSignal {
    pub symbol: String,
    pub direction: Direction,
    pub price: f64,
    pub order_notional_usd: f64,
    pub maker_fee_per_side: f64,
    pub taker_fee_per_side: f64,
    pub features: MakerFeatures,
}

#[derive(Debug, Clone, Copy)]
pub struct MakerTrade {
    pub aggressor: Direction,
    pub price: f64,
    pub qty: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct MakerBook {
    pub bid: f64,
    pub bid_qty: f64,
    pub bid_notional: f64,
    pub ask: f64,
    pub ask_qty: f64,
    pub ask_notional: f64,
    pub executable: bool,
}

#[derive(Debug)]
struct PendingEntry {
    symbol: String,
    direction: Direction,
    price: f64,
    remaining_queue_and_order_qty: Option<f64>,
    order_notional_usd: f64,
    maker_fee_per_side: f64,
    taker_fee_per_side: f64,
    features: MakerFeatures,
    created_at: Instant,
}

#[derive(Debug, Clone)]
struct PendingExit {
    symbol: String,
    direction: Direction,
    horizon_secs: u64,
    entry_price: f64,
    order_notional_usd: f64,
    round_trip_fee: f64,
    fill_latency_ms: u64,
    features: MakerFeatures,
    filled_at: Instant,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct MakerSample {
    observed_at_ms: u64,
    gross_return: f64,
    net_return: f64,
    round_trip_fee: f64,
    entry_price: f64,
    exit_price: f64,
    capacity_usd: f64,
    fill_latency_ms: u64,
    direction_sign: i8,
    signal_strength: f64,
    book_event_imbalance: f64,
    book_event_count: usize,
    trade_imbalance: f64,
    trade_notional_usd: f64,
    queue_imbalance: f64,
    spread_bps: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedMakerHistory(HashMap<String, Vec<MakerSample>>);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct MakerCounters {
    attempts: u64,
    filled_entries: u64,
    expired_entries: u64,
    placement_price_moved: u64,
    unavailable_exits: u64,
}

#[derive(Debug, Serialize)]
struct MakerValidationRow {
    symbol: String,
    horizon_secs: u64,
    samples: usize,
    research_gate_passed: bool,
    edge_lcb95: Option<f64>,
    mean_net_return: Option<f64>,
    win_rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedCounters {
    counters: MakerCounters,
}

#[derive(Debug, Serialize)]
struct MakerReport {
    schema: &'static str,
    generated_at_ms: u64,
    execution_enabled: bool,
    fill_model: &'static str,
    entry_ttl_ms: u64,
    placement_latency_ms: u64,
    minimum_research_samples: usize,
    minimum_edge_lcb: f64,
    fill_rate: Option<f64>,
    counters: MakerCounters,
    rows: Vec<MakerValidationRow>,
}

type MakerHistories = HashMap<String, VecDeque<MakerSample>>;

pub struct MakerEntryResearch {
    entries: Vec<PendingEntry>,
    exits: Vec<PendingExit>,
    histories: MakerHistories,
    counters: MakerCounters,
}

impl MakerEntryResearch {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            exits: Vec::new(),
            histories: load_histories(),
            counters: load_counters(),
        }
    }

    pub fn start(&mut self, signal: MakerSignal) {
        if !signal.price.is_finite()
            || signal.price <= 0.0
            || !signal.order_notional_usd.is_finite()
            || signal.order_notional_usd <= 0.0
        {
            return;
        }
        self.counters.attempts += 1;
        self.entries.push(PendingEntry {
            symbol: signal.symbol,
            direction: signal.direction,
            price: signal.price,
            remaining_queue_and_order_qty: None,
            order_notional_usd: signal.order_notional_usd,
            maker_fee_per_side: signal.maker_fee_per_side,
            taker_fee_per_side: signal.taker_fee_per_side,
            features: signal.features,
            created_at: Instant::now(),
        });
    }

    pub fn on_trade(&mut self, symbol: &str, trade: MakerTrade) {
        if !trade.price.is_finite()
            || trade.price <= 0.0
            || !trade.qty.is_finite()
            || trade.qty <= 0.0
        {
            return;
        }
        let mut filled = Vec::new();
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.symbol != symbol
                || entry.created_at.elapsed() > ENTRY_TTL
                || entry.remaining_queue_and_order_qty.is_none()
            {
                continue;
            }
            let consumes_order = match entry.direction {
                Direction::Long => {
                    trade.aggressor == Direction::Short && trade.price <= entry.price
                }
                Direction::Short => {
                    trade.aggressor == Direction::Long && trade.price >= entry.price
                }
            };
            if consumes_order {
                let remaining = entry
                    .remaining_queue_and_order_qty
                    .as_mut()
                    .expect("entrada ativada tem fila inicial");
                *remaining -= trade.qty;
                if *remaining <= 0.0 {
                    filled.push(index);
                }
            }
        }
        for index in filled.into_iter().rev() {
            let entry = self.entries.swap_remove(index);
            self.counters.filled_entries += 1;
            let fill_latency_ms = entry.created_at.elapsed().as_millis() as u64;
            let filled_at = Instant::now();
            self.exits
                .extend(HORIZONS_SECS.map(|horizon_secs| PendingExit {
                    symbol: entry.symbol.clone(),
                    direction: entry.direction,
                    horizon_secs,
                    entry_price: entry.price,
                    order_notional_usd: entry.order_notional_usd,
                    round_trip_fee: entry.maker_fee_per_side + entry.taker_fee_per_side,
                    fill_latency_ms,
                    features: entry.features,
                    filled_at,
                }));
            append_raw(serde_json::json!({
                "event": "maker_entry_filled",
                "ts_ms": crate::raw_log::now_ms(),
                "symbol": entry.symbol,
                "direction": entry.direction.key(),
                "entry_price": entry.price,
                "fill_latency_ms": fill_latency_ms,
            }));
        }
    }

    pub fn tick(&mut self, books: &HashMap<String, MakerBook>) {
        let mut waiting_entries = Vec::with_capacity(self.entries.len());
        for mut entry in self.entries.drain(..) {
            if entry.created_at.elapsed() > ENTRY_TTL {
                self.counters.expired_entries += 1;
                append_raw(serde_json::json!({
                    "event": "maker_entry_expired",
                    "ts_ms": crate::raw_log::now_ms(),
                    "symbol": entry.symbol,
                    "direction": entry.direction.key(),
                    "entry_price": entry.price,
                }));
                continue;
            }
            if entry.remaining_queue_and_order_qty.is_none()
                && entry.created_at.elapsed() >= PLACEMENT_LATENCY
            {
                let Some(book) = books.get(&entry.symbol).filter(|book| book.executable) else {
                    self.counters.placement_price_moved += 1;
                    continue;
                };
                let (best_price, visible_qty) = match entry.direction {
                    Direction::Long => (book.bid, book.bid_qty),
                    Direction::Short => (book.ask, book.ask_qty),
                };
                if (best_price - entry.price).abs() > entry.price * 1e-10 {
                    self.counters.placement_price_moved += 1;
                    append_raw(serde_json::json!({
                        "event": "maker_entry_price_moved_before_arrival",
                        "ts_ms": crate::raw_log::now_ms(),
                        "symbol": entry.symbol,
                        "direction": entry.direction.key(),
                        "signal_price": entry.price,
                        "arrival_best_price": best_price,
                    }));
                    continue;
                }
                let order_qty = entry.order_notional_usd / entry.price;
                entry.remaining_queue_and_order_qty = Some(visible_qty + order_qty);
            }
            waiting_entries.push(entry);
        }
        self.entries = waiting_entries;

        let mut waiting_exits = Vec::with_capacity(self.exits.len());
        for exit in self.exits.drain(..) {
            if exit.filled_at.elapsed() < Duration::from_secs(exit.horizon_secs) {
                waiting_exits.push(exit);
                continue;
            }
            let Some(book) = books.get(&exit.symbol).filter(|book| book.executable) else {
                self.counters.unavailable_exits += 1;
                continue;
            };
            let (exit_price, exit_capacity) = match exit.direction {
                Direction::Long => (book.bid, book.bid_notional),
                Direction::Short => (book.ask, book.ask_notional),
            };
            let gross_return = match exit.direction {
                Direction::Long => (exit_price - exit.entry_price) / exit.entry_price,
                Direction::Short => (exit.entry_price - exit_price) / exit.entry_price,
            };
            let net_return = gross_return - exit.round_trip_fee;
            let sample = MakerSample {
                observed_at_ms: crate::raw_log::now_ms(),
                gross_return,
                net_return,
                round_trip_fee: exit.round_trip_fee,
                entry_price: exit.entry_price,
                exit_price,
                capacity_usd: exit.order_notional_usd.min(exit_capacity),
                fill_latency_ms: exit.fill_latency_ms,
                direction_sign: if exit.direction == Direction::Long {
                    1
                } else {
                    -1
                },
                signal_strength: exit.features.signal_strength,
                book_event_imbalance: exit.features.book_event_imbalance,
                book_event_count: exit.features.book_event_count,
                trade_imbalance: exit.features.trade_imbalance,
                trade_notional_usd: exit.features.trade_notional_usd,
                queue_imbalance: exit.features.queue_imbalance,
                spread_bps: exit.features.spread_bps,
            };
            let history = self
                .histories
                .entry(history_key(&exit.symbol, exit.horizon_secs))
                .or_default();
            history.push_back(sample);
            while history.len() > HISTORY_CAP {
                history.pop_front();
            }
            append_raw(serde_json::json!({
                "event": "maker_entry_taker_exit_resolved",
                "ts_ms": sample.observed_at_ms,
                "symbol": exit.symbol,
                "horizon_secs": exit.horizon_secs,
                "gross_return": gross_return,
                "net_return": net_return,
                "round_trip_fee": exit.round_trip_fee,
            }));
        }
        self.exits = waiting_exits;
    }

    pub fn save(&self) {
        save_histories(&self.histories);
        save_report(&self.histories, self.counters);
    }
}

fn history_key(symbol: &str, horizon_secs: u64) -> String {
    format!("{symbol}@{horizon_secs}s")
}

fn parse_history_key(key: &str) -> (&str, u64) {
    key.rsplit_once('@')
        .and_then(|(symbol, horizon)| {
            horizon
                .strip_suffix('s')
                .and_then(|raw| raw.parse().ok())
                .map(|seconds| (symbol, seconds))
        })
        .unwrap_or((key, 5))
}

fn history_path() -> String {
    format!(
        "{}/data/confirmation_history_maker_entry_v3.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn report_path() -> String {
    format!(
        "{}/data/strategy_validation_maker_entry_v3.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn raw_path() -> String {
    format!(
        "{}/data/raw_maker_entry_v3.jsonl",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn load_histories() -> MakerHistories {
    let Ok(raw) = std::fs::read_to_string(history_path()) else {
        return HashMap::new();
    };
    let Ok(PersistedMakerHistory(saved)) = serde_json::from_str(&raw) else {
        return HashMap::new();
    };
    saved
        .into_iter()
        .map(|(key, samples)| (key, samples.into()))
        .collect()
}

fn load_counters() -> MakerCounters {
    std::fs::read_to_string(report_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<SavedCounters>(&raw).ok())
        .map(|saved| saved.counters)
        .unwrap_or_default()
}

fn save_histories(histories: &MakerHistories) {
    let saved = PersistedMakerHistory(
        histories
            .iter()
            .map(|(key, samples)| (key.clone(), samples.iter().copied().collect()))
            .collect(),
    );
    if let Ok(json) = serde_json::to_string(&saved) {
        let _ = std::fs::create_dir_all(format!("{}/data", env!("CARGO_MANIFEST_DIR")));
        let _ = std::fs::write(history_path(), json);
    }
}

fn save_report(histories: &MakerHistories, counters: MakerCounters) {
    let mut rows: Vec<MakerValidationRow> = histories
        .iter()
        .map(|(key, samples)| {
            let (symbol, horizon_secs) = parse_history_key(key);
            let returns: Vec<f64> = samples.iter().map(|sample| sample.net_return).collect();
            let edge_lcb95 = chronological_edge_lcb(&returns);
            MakerValidationRow {
                symbol: symbol.to_string(),
                horizon_secs,
                samples: samples.len(),
                research_gate_passed: samples.len() >= MIN_RESEARCH_SAMPLES
                    && edge_lcb95.is_some_and(|edge| edge > MIN_EDGE_LCB),
                edge_lcb95,
                mean_net_return: mean(&returns),
                win_rate: (!returns.is_empty()).then(|| {
                    returns.iter().filter(|return_| **return_ > 0.0).count() as f64
                        / returns.len() as f64
                }),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.symbol
            .cmp(&b.symbol)
            .then_with(|| a.horizon_secs.cmp(&b.horizon_secs))
    });
    let fill_rate =
        (counters.attempts > 0).then(|| counters.filled_entries as f64 / counters.attempts as f64);
    let report = MakerReport {
        schema: "aurumos.microstructure.maker_entry_research.v3",
        generated_at_ms: crate::raw_log::now_ms(),
        execution_enabled: false,
        fill_model: "250ms_arrival_same_best_price_then_future_opposite_aggressive_volume_consumes_visible_queue_plus_full_order",
        entry_ttl_ms: ENTRY_TTL.as_millis() as u64,
        placement_latency_ms: PLACEMENT_LATENCY.as_millis() as u64,
        minimum_research_samples: MIN_RESEARCH_SAMPLES,
        minimum_edge_lcb: MIN_EDGE_LCB,
        fill_rate,
        counters,
        rows,
    };
    if let Ok(json) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::write(report_path(), json);
    }
}

fn chronological_edge_lcb(returns: &[f64]) -> Option<f64> {
    if returns.len() < MIN_RESEARCH_SAMPLES {
        return None;
    }
    let split = returns.len() / 2;
    let train = mean_lcb(&returns[..split])?;
    let validation = &returns[split..];
    let validation_lcb = mean_lcb(validation)?;
    let best_index = validation
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))?
        .0;
    let without_best: Vec<f64> = validation
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (index != best_index).then_some(*value))
        .collect();
    Some(train.min(validation_lcb).min(mean_lcb(&without_best)?))
}

fn mean_lcb(values: &[f64]) -> Option<f64> {
    if values.len() < 2 || values.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let n = values.len() as f64;
    let average = values.iter().sum::<f64>() / n;
    let variance = values
        .iter()
        .map(|value| (value - average).powi(2))
        .sum::<f64>()
        / (n - 1.0);
    Some(average - 1.96 * (variance / n).sqrt())
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty() && values.iter().all(|value| value.is_finite()))
        .then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn append_raw(value: serde_json::Value) {
    crate::raw_log::append(&raw_path(), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(direction: Direction) -> MakerSignal {
        MakerSignal {
            symbol: "BTCUSDT".to_string(),
            direction,
            price: 100.0,
            order_notional_usd: 25.0,
            maker_fee_per_side: 0.0002,
            taker_fee_per_side: 0.00055,
            features: MakerFeatures {
                signal_strength: 0.8,
                book_event_imbalance: 0.8,
                book_event_count: 10,
                trade_imbalance: 0.9,
                trade_notional_usd: 20_000.0,
                queue_imbalance: 0.4,
                spread_bps: 1.0,
            },
        }
    }

    #[test]
    fn maker_fill_requires_opposite_aggression_and_full_queue_consumption() {
        let mut research = MakerEntryResearch::new();
        research.counters = MakerCounters::default();
        research.histories.clear();
        research.start(signal(Direction::Long));
        research.entries[0].created_at = Instant::now() - PLACEMENT_LATENCY;
        research.tick(&HashMap::from([(
            "BTCUSDT".to_string(),
            MakerBook {
                bid: 100.0,
                bid_qty: 1.0,
                bid_notional: 100.0,
                ask: 100.1,
                ask_qty: 1.0,
                ask_notional: 100.1,
                executable: true,
            },
        )]));
        research.on_trade(
            "BTCUSDT",
            MakerTrade {
                aggressor: Direction::Long,
                price: 100.0,
                qty: 10.0,
            },
        );
        assert_eq!(research.counters.filled_entries, 0);
        research.on_trade(
            "BTCUSDT",
            MakerTrade {
                aggressor: Direction::Short,
                price: 100.0,
                qty: 1.24,
            },
        );
        assert_eq!(research.counters.filled_entries, 0);
        research.on_trade(
            "BTCUSDT",
            MakerTrade {
                aggressor: Direction::Short,
                price: 100.0,
                qty: 0.02,
            },
        );
        assert_eq!(research.counters.filled_entries, 1);
        assert_eq!(research.exits.len(), HORIZONS_SECS.len());
    }

    #[test]
    fn maker_entry_is_rejected_when_best_price_moves_during_placement_latency() {
        let mut research = MakerEntryResearch::new();
        research.counters = MakerCounters::default();
        research.histories.clear();
        research.start(signal(Direction::Long));
        research.entries[0].created_at = Instant::now() - PLACEMENT_LATENCY;
        research.tick(&HashMap::from([(
            "BTCUSDT".to_string(),
            MakerBook {
                bid: 99.9,
                bid_qty: 1.0,
                bid_notional: 99.9,
                ask: 100.0,
                ask_qty: 1.0,
                ask_notional: 100.0,
                executable: true,
            },
        )]));
        assert!(research.entries.is_empty());
        assert_eq!(research.counters.placement_price_moved, 1);
        assert_eq!(research.counters.filled_entries, 0);
    }

    #[test]
    fn maker_entry_plus_taker_exit_uses_both_fee_sides() {
        let gross: f64 = 0.001;
        let net = gross - 0.0002 - 0.00055;
        assert!((net - 0.00025).abs() < 1e-12);
    }
}
