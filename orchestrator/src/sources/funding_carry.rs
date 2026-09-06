use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;

use crate::events::{DashboardEvent, EventBus};
use crate::types::Strategy;

const SPOT_TICKERS_URL: &str = "https://api.bybit.com/v5/market/tickers?category=spot";
const LINEAR_TICKERS_URL: &str = "https://api.bybit.com/v5/market/tickers?category=linear";
const FUNDING_HISTORY_URL: &str = "https://api.bybit.com/v5/market/funding/history";
const POLL_INTERVAL: Duration = Duration::from_secs(300);

// Taxas publicadas pela Bybit para VIP 0 em 02/09/2026. O cenário base
// usa os pares regulares; o cenário de estresse usa as maiores taxas
// publicadas para as zonas especiais, porque o endpoint público de ticker
// não informa a taxa específica da conta.
const SPOT_TAKER_BASE: f64 = 0.0010;
const LINEAR_TAKER_BASE: f64 = 0.00055;
const SPOT_TAKER_STRESS: f64 = 0.0020;
const LINEAR_TAKER_STRESS: f64 = 0.00110;
const BASIS_ADVERSE_BUFFER: f64 = 0.0010;
const COST_SAFETY_MULTIPLIER: f64 = 1.25;
const MAX_PAYBACK_PERIODS: f64 = 3.0;
const MIN_TURNOVER_USD: f64 = 5_000_000.0;
const MIN_L1_CAPACITY_USD: f64 = 25.0;
const REPORT_CANDIDATE_CAP: usize = 100;
const HISTORY_FETCH_CAP: usize = 10;
const MIN_SETTLED_SAMPLES: usize = 30;
const FORWARD_FORECAST_CAP: usize = 10;
const FORWARD_MAX_LEAD_MINUTES: f64 = 30.0;
const SETTLEMENT_PUBLICATION_GRACE_MS: u64 = 120_000;
/// Registros liquidados há mais que isto saem da memória e do WAL — já
/// contribuíram pro resumo agregado e, sem isto, o `BTreeMap` e o arquivo
/// em disco crescem pra sempre num processo pensado pra rodar indefinidamente.
const FORWARD_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
struct TopOfBook {
    symbol: String,
    bid: f64,
    bid_size: f64,
    ask: f64,
    ask_size: f64,
    turnover_24h: f64,
    funding_rate: Option<f64>,
    funding_interval_hours: Option<f64>,
    next_funding_time_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct CarryCandidate {
    observed_at_ms: u64,
    venue: &'static str,
    symbol: String,
    funding_rate: f64,
    funding_interval_hours: f64,
    next_funding_time_ms: u64,
    minutes_to_funding: f64,
    spot_bid: f64,
    spot_ask: f64,
    perp_bid: f64,
    perp_ask: f64,
    spot_turnover_24h_usd: f64,
    perp_turnover_24h_usd: f64,
    l1_round_trip_capacity_usd: f64,
    entry_basis_fraction: f64,
    book_cross_cost_fraction: f64,
    base_all_in_cost_fraction: f64,
    stress_all_in_cost_fraction: f64,
    conservative_required_fraction: f64,
    payback_periods: f64,
    payback_hours: f64,
    stress_net_three_periods: f64,
    current_screen_pass: bool,
    rejection_reasons: Vec<&'static str>,
    settled_history: Option<SettledHistoryScreen>,
}

#[derive(Debug, Clone, Serialize)]
struct SettledHistoryScreen {
    settled_samples: usize,
    rolling_three_period_windows: usize,
    profitable_windows: usize,
    profitable_window_fraction: f64,
    mean_net_fraction: Option<f64>,
    lcb95_net_fraction: Option<f64>,
    without_best_lcb95_net_fraction: Option<f64>,
    pass: bool,
    limitation: &'static str,
}

#[derive(Debug, Clone)]
struct SettledFunding {
    timestamp_ms: u64,
    rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ForwardForecast {
    symbol: String,
    observed_at_ms: u64,
    settlement_time_ms: u64,
    lead_minutes: f64,
    predicted_funding_rate: f64,
    conservative_required_fraction: f64,
    current_screen_pass: bool,
    settled_funding_rate: Option<f64>,
    prediction_error: Option<f64>,
    resolved_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ForwardEvent {
    Forecast {
        record: ForwardForecast,
    },
    Settlement {
        symbol: String,
        settlement_time_ms: u64,
        settled_funding_rate: f64,
        prediction_error: f64,
        resolved_at_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize)]
struct ForwardSummary {
    schema: &'static str,
    forecast_lead_limit_minutes: f64,
    recorded_forecasts: usize,
    pending_forecasts: usize,
    settled_forecasts: usize,
    screened_when_recorded: usize,
    mean_prediction_error: Option<f64>,
    mean_absolute_prediction_error: Option<f64>,
    repeated_rate_would_cover_three_period_cost: usize,
    limitation: &'static str,
}

struct ForwardTracker {
    wal_path: String,
    records: BTreeMap<String, ForwardForecast>,
}

#[derive(Debug, Serialize)]
struct CarryReport<'a> {
    schema: &'static str,
    generated_at_ms: u64,
    venue: &'static str,
    mode: &'static str,
    execution_enabled: bool,
    evidence: &'static str,
    common_spot_perp_pairs: usize,
    positive_funding_pairs: usize,
    current_screened_pairs: usize,
    history_checked_pairs: usize,
    history_screened_pairs: usize,
    promotion_status: &'static str,
    forward_validation: ForwardSummary,
    assumptions: CarryAssumptions,
    candidates: &'a [CarryCandidate],
}

#[derive(Debug, Serialize)]
struct CarryAssumptions {
    spot_taker_base: f64,
    linear_taker_base: f64,
    spot_taker_stress: f64,
    linear_taker_stress: f64,
    basis_adverse_buffer: f64,
    cost_safety_multiplier: f64,
    max_payback_periods: f64,
    min_turnover_usd_each_leg: f64,
    min_l1_round_trip_capacity_usd: f64,
}

impl ForwardTracker {
    fn load() -> Self {
        let wal_path = format!(
            "{}/data/funding_carry_forward_v1.jsonl",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut records = BTreeMap::new();
        let mut malformed = 0_u64;
        if let Ok(raw) = std::fs::read_to_string(&wal_path) {
            for line in raw.lines().filter(|line| !line.trim().is_empty()) {
                let Ok(event) = serde_json::from_str::<ForwardEvent>(line) else {
                    malformed += 1;
                    continue;
                };
                match event {
                    ForwardEvent::Forecast { record } => {
                        records.insert(
                            forward_key(&record.symbol, record.settlement_time_ms),
                            record,
                        );
                    }
                    ForwardEvent::Settlement {
                        symbol,
                        settlement_time_ms,
                        settled_funding_rate,
                        prediction_error,
                        resolved_at_ms,
                    } => {
                        if let Some(record) =
                            records.get_mut(&forward_key(&symbol, settlement_time_ms))
                        {
                            record.settled_funding_rate = Some(settled_funding_rate);
                            record.prediction_error = Some(prediction_error);
                            record.resolved_at_ms = Some(resolved_at_ms);
                        }
                    }
                }
            }
        }
        if malformed > 0 {
            tracing::warn!(
                malformed_lines = malformed,
                path = wal_path,
                "linhas inválidas ignoradas no WAL forward de funding"
            );
        }
        Self { wal_path, records }
    }

    fn record_forecasts(&mut self, candidates: &[CarryCandidate]) -> usize {
        let mut added = 0;
        for candidate in candidates
            .iter()
            .take(FORWARD_FORECAST_CAP)
            .filter(|row| row.minutes_to_funding <= FORWARD_MAX_LEAD_MINUTES)
        {
            let key = forward_key(&candidate.symbol, candidate.next_funding_time_ms);
            if self.records.contains_key(&key) {
                continue;
            }
            let record = ForwardForecast {
                symbol: candidate.symbol.clone(),
                observed_at_ms: candidate.observed_at_ms,
                settlement_time_ms: candidate.next_funding_time_ms,
                lead_minutes: candidate.minutes_to_funding,
                predicted_funding_rate: candidate.funding_rate,
                conservative_required_fraction: candidate.conservative_required_fraction,
                current_screen_pass: candidate.current_screen_pass,
                settled_funding_rate: None,
                prediction_error: None,
                resolved_at_ms: None,
            };
            append_forward_event(
                &self.wal_path,
                &ForwardEvent::Forecast {
                    record: record.clone(),
                },
            );
            self.records.insert(key, record);
            added += 1;
        }
        added
    }

    async fn reconcile(&mut self, client: &reqwest::Client) {
        let now_ms = crate::raw_log::now_ms();
        let due_symbols: BTreeSet<String> = self
            .records
            .values()
            .filter(|record| {
                record.settled_funding_rate.is_none()
                    && now_ms
                        >= record
                            .settlement_time_ms
                            .saturating_add(SETTLEMENT_PUBLICATION_GRACE_MS)
            })
            .map(|record| record.symbol.clone())
            .collect();

        for symbol in due_symbols {
            let history = match fetch_funding_history(client, &symbol).await {
                Ok(history) => history,
                Err(error) => {
                    tracing::debug!(%symbol, %error, "settlement forward ainda indisponível");
                    continue;
                }
            };
            let mut resolved_events = Vec::new();
            for record in self.records.values_mut().filter(|record| {
                record.symbol == symbol
                    && record.settled_funding_rate.is_none()
                    && now_ms
                        >= record
                            .settlement_time_ms
                            .saturating_add(SETTLEMENT_PUBLICATION_GRACE_MS)
            }) {
                let Some(settled) = history
                    .iter()
                    .find(|row| row.timestamp_ms == record.settlement_time_ms)
                else {
                    continue;
                };
                let prediction_error = settled.rate - record.predicted_funding_rate;
                record.settled_funding_rate = Some(settled.rate);
                record.prediction_error = Some(prediction_error);
                record.resolved_at_ms = Some(now_ms);
                resolved_events.push(ForwardEvent::Settlement {
                    symbol: record.symbol.clone(),
                    settlement_time_ms: record.settlement_time_ms,
                    settled_funding_rate: settled.rate,
                    prediction_error,
                    resolved_at_ms: now_ms,
                });
            }
            for event in resolved_events {
                append_forward_event(&self.wal_path, &event);
            }
        }
    }

    /// Remove registros já liquidados há mais de `FORWARD_RETENTION_MS`.
    /// Pendentes (ainda sem `settled_funding_rate`) nunca são removidos
    /// aqui — só depois de liquidar e envelhecer. Reescreve o WAL a partir
    /// do estado podado, senão o arquivo em disco continuaria crescendo
    /// sem limite mesmo com a memória sob controle.
    fn prune(&mut self, now_ms: u64) {
        let before = self.records.len();
        self.records
            .retain(|_, record| match record.resolved_at_ms {
                Some(resolved_at) => now_ms.saturating_sub(resolved_at) < FORWARD_RETENTION_MS,
                None => true,
            });
        if self.records.len() != before {
            self.rewrite_wal();
        }
    }

    /// Recria o WAL do zero a partir do estado atual em memória (escreve
    /// em `.tmp` e promove por rename, mesmo padrão de escrita segura já
    /// usado pro estado do portfólio) — usado só depois de uma poda, não
    /// em toda iteração.
    fn rewrite_wal(&self) {
        let mut buffer = String::new();
        for record in self.records.values() {
            if let Ok(value) = serde_json::to_value(ForwardEvent::Forecast {
                record: record.clone(),
            }) {
                buffer.push_str(&value.to_string());
                buffer.push('\n');
            }
            if let (Some(settled_funding_rate), Some(prediction_error), Some(resolved_at_ms)) = (
                record.settled_funding_rate,
                record.prediction_error,
                record.resolved_at_ms,
            ) {
                let event = ForwardEvent::Settlement {
                    symbol: record.symbol.clone(),
                    settlement_time_ms: record.settlement_time_ms,
                    settled_funding_rate,
                    prediction_error,
                    resolved_at_ms,
                };
                if let Ok(value) = serde_json::to_value(event) {
                    buffer.push_str(&value.to_string());
                    buffer.push('\n');
                }
            }
        }
        let temporary = format!("{}.tmp", self.wal_path);
        if std::fs::write(&temporary, buffer).is_ok() {
            let _ = std::fs::rename(&temporary, &self.wal_path);
        }
    }

    fn summary(&self) -> ForwardSummary {
        let settled: Vec<&ForwardForecast> = self
            .records
            .values()
            .filter(|record| record.settled_funding_rate.is_some())
            .collect();
        let errors: Vec<f64> = settled
            .iter()
            .filter_map(|record| record.prediction_error)
            .collect();
        ForwardSummary {
            schema: "aurumos.funding_carry.forward.v1",
            forecast_lead_limit_minutes: FORWARD_MAX_LEAD_MINUTES,
            recorded_forecasts: self.records.len(),
            pending_forecasts: self.records.len().saturating_sub(settled.len()),
            settled_forecasts: settled.len(),
            screened_when_recorded: self
                .records
                .values()
                .filter(|record| record.current_screen_pass)
                .count(),
            mean_prediction_error: mean(&errors),
            mean_absolute_prediction_error: mean(
                &errors.iter().map(|error| error.abs()).collect::<Vec<_>>(),
            ),
            repeated_rate_would_cover_three_period_cost: settled
                .iter()
                .filter(|record| {
                    record.settled_funding_rate.is_some_and(|rate| {
                        MAX_PAYBACK_PERIODS * rate > record.conservative_required_fraction
                    })
                })
                .count(),
            limitation: "compara a taxa pública observada até 30 min antes com a taxa pública liquidada; ainda não prova fill, basis de saída nem crédito na conta",
        }
    }
}

fn forward_key(symbol: &str, settlement_time_ms: u64) -> String {
    format!("{symbol}:{settlement_time_ms}")
}

fn append_forward_event(path: &str, event: &ForwardEvent) {
    if let Ok(value) = serde_json::to_value(event) {
        crate::raw_log::append(path, value);
    }
}

pub struct FundingCarrySource {
    bus: EventBus,
}

impl FundingCarrySource {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .user_agent("AurumOS-FundingCarryResearch/0.1")
            .timeout(Duration::from_secs(20))
            .build()?;
        let mut forward_tracker = ForwardTracker::load();

        loop {
            match scan(&client).await {
                Ok((common_pairs, mut candidates)) => {
                    candidates.sort_by(|a, b| {
                        a.payback_periods
                            .total_cmp(&b.payback_periods)
                            .then_with(|| {
                                b.l1_round_trip_capacity_usd
                                    .total_cmp(&a.l1_round_trip_capacity_usd)
                            })
                    });
                    let new_forward_forecasts = forward_tracker.record_forecasts(&candidates);
                    forward_tracker.reconcile(&client).await;
                    forward_tracker.prune(crate::raw_log::now_ms());
                    let forward_summary = forward_tracker.summary();
                    persist(common_pairs, &candidates, forward_summary);

                    let current_screened = candidates
                        .iter()
                        .filter(|row| row.current_screen_pass)
                        .count();
                    let history_screened = candidates
                        .iter()
                        .filter(|row| row.settled_history.as_ref().is_some_and(|h| h.pass))
                        .count();
                    let best = candidates.first();
                    self.bus.emit(DashboardEvent::scan_heartbeat(
                        Strategy::FundingCarry,
                        common_pairs as u32,
                        best.map(|row| row.symbol.as_str()).unwrap_or("—"),
                        best.map(|row| row.stress_net_three_periods).unwrap_or(0.0),
                    ));
                    tracing::info!(
                        common_pairs,
                        positive_funding_pairs = candidates.len(),
                        current_screened_pairs = current_screened,
                        history_screened_pairs = history_screened,
                        new_forward_forecasts,
                        best_symbol = best.map(|row| row.symbol.as_str()).unwrap_or("—"),
                        best_payback_periods =
                            best.map(|row| row.payback_periods).unwrap_or(f64::INFINITY),
                        "funding carry Bybit atualizado (pesquisa; execução desabilitada)"
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "falha no radar funding carry; mantendo execução desabilitada");
                }
            }
            sleep(POLL_INTERVAL).await;
        }
    }
}

async fn scan(client: &reqwest::Client) -> anyhow::Result<(usize, Vec<CarryCandidate>)> {
    let (spot, linear) = tokio::try_join!(
        fetch_tickers(client, SPOT_TICKERS_URL, false),
        fetch_tickers(client, LINEAR_TICKERS_URL, true)
    )?;
    let observed_at_ms = crate::raw_log::now_ms();
    let common_pairs = linear
        .keys()
        .filter(|symbol| spot.contains_key(*symbol))
        .count();
    let mut candidates = Vec::new();

    for (symbol, perp) in &linear {
        let Some(spot_book) = spot.get(symbol) else {
            continue;
        };
        let Some(row) = calculate_candidate(observed_at_ms, spot_book, perp) else {
            continue;
        };
        candidates.push(row);
    }
    candidates.sort_by(|a, b| a.payback_periods.total_cmp(&b.payback_periods));
    let symbols: Vec<String> = candidates
        .iter()
        .take(HISTORY_FETCH_CAP)
        .map(|row| row.symbol.clone())
        .collect();
    let histories = join_all(
        symbols
            .iter()
            .map(|symbol| fetch_funding_history(client, symbol)),
    )
    .await;
    for (row, history) in candidates.iter_mut().zip(histories) {
        match history {
            Ok(settlements) => {
                let rates: Vec<f64> = settlements.iter().map(|row| row.rate).collect();
                row.settled_history = Some(analyze_settled_history(
                    &rates,
                    row.conservative_required_fraction,
                ));
            }
            Err(error) => {
                tracing::debug!(symbol = %row.symbol, %error, "histórico de funding indisponível");
            }
        }
    }
    Ok((common_pairs, candidates))
}

async fn fetch_funding_history(
    client: &reqwest::Client,
    symbol: &str,
) -> anyhow::Result<Vec<SettledFunding>> {
    let body: Value = client
        .get(FUNDING_HISTORY_URL)
        .query(&[("category", "linear"), ("symbol", symbol), ("limit", "60")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let ret_code = body.get("retCode").and_then(Value::as_i64).unwrap_or(-1);
    if ret_code != 0 {
        anyhow::bail!(
            "Bybit V5 funding history retCode={ret_code}: {}",
            body.get("retMsg")
                .and_then(Value::as_str)
                .unwrap_or("sem mensagem")
        );
    }
    let list = body
        .pointer("/result/list")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Bybit V5 funding history sem result.list"))?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let rate = item
                .get("fundingRate")
                .and_then(Value::as_str)
                .and_then(|raw| raw.parse::<f64>().ok())
                .filter(|rate| rate.is_finite())?;
            let timestamp_ms = item
                .get("fundingRateTimestamp")
                .and_then(Value::as_str)
                .and_then(|raw| raw.parse::<u64>().ok())?;
            Some(SettledFunding { timestamp_ms, rate })
        })
        .collect())
}

async fn fetch_tickers(
    client: &reqwest::Client,
    url: &str,
    linear: bool,
) -> anyhow::Result<HashMap<String, TopOfBook>> {
    let body: Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let ret_code = body.get("retCode").and_then(Value::as_i64).unwrap_or(-1);
    if ret_code != 0 {
        anyhow::bail!(
            "Bybit V5 retCode={ret_code}: {}",
            body.get("retMsg")
                .and_then(Value::as_str)
                .unwrap_or("sem mensagem")
        );
    }
    let list = body
        .pointer("/result/list")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Bybit V5 sem result.list"))?;
    let mut result = HashMap::new();
    for item in list {
        let Some(symbol) = item.get("symbol").and_then(Value::as_str) else {
            continue;
        };
        if !symbol.ends_with("USDT") {
            continue;
        }
        let row = TopOfBook {
            symbol: symbol.to_string(),
            bid: number(item, "bid1Price"),
            bid_size: number(item, "bid1Size"),
            ask: number(item, "ask1Price"),
            ask_size: number(item, "ask1Size"),
            turnover_24h: number(item, "turnover24h"),
            funding_rate: linear.then(|| number(item, "fundingRate")),
            funding_interval_hours: linear.then(|| number(item, "fundingIntervalHour")),
            next_funding_time_ms: linear.then(|| integer(item, "nextFundingTime")).flatten(),
        };
        if valid_book(&row) {
            result.insert(row.symbol.clone(), row);
        }
    }
    Ok(result)
}

fn number(value: &Value, field: &str) -> f64 {
    value
        .get(field)
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn integer(value: &Value, field: &str) -> Option<u64> {
    value
        .get(field)
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse::<u64>().ok())
}

fn valid_book(row: &TopOfBook) -> bool {
    row.bid > 0.0
        && row.ask >= row.bid
        && row.bid_size > 0.0
        && row.ask_size > 0.0
        && row.turnover_24h >= 0.0
}

fn calculate_candidate(
    observed_at_ms: u64,
    spot: &TopOfBook,
    perp: &TopOfBook,
) -> Option<CarryCandidate> {
    let funding_rate = perp.funding_rate?;
    if funding_rate <= 0.0 || !funding_rate.is_finite() {
        return None;
    }
    let funding_interval_hours = perp.funding_interval_hours?;
    let next_funding_time_ms = perp.next_funding_time_ms?;
    if funding_interval_hours <= 0.0 || next_funding_time_ms <= observed_at_ms {
        return None;
    }

    let spot_mid = (spot.bid + spot.ask) / 2.0;
    let perp_mid = (perp.bid + perp.ask) / 2.0;
    let spot_spread = (spot.ask - spot.bid) / spot_mid;
    let perp_spread = (perp.ask - perp.bid) / perp_mid;
    let book_cross_cost_fraction = spot_spread + perp_spread;
    let entry_basis_fraction = (perp.bid - spot.ask) / spot.ask;
    let base_fees = 2.0 * (SPOT_TAKER_BASE + LINEAR_TAKER_BASE);
    let stress_fees = 2.0 * (SPOT_TAKER_STRESS + LINEAR_TAKER_STRESS);
    let base_all_in_cost_fraction = base_fees + book_cross_cost_fraction;
    let stress_all_in_cost_fraction = stress_fees + book_cross_cost_fraction;
    // Não conta prêmio positivo de basis como lucro garantido. Se o perp
    // já está abaixo do spot, trata a diferença como custo; sempre adiciona
    // um buffer para uma piora do basis até a saída.
    let adverse_basis = (-entry_basis_fraction).max(0.0) + BASIS_ADVERSE_BUFFER;
    let conservative_required_fraction =
        stress_all_in_cost_fraction * COST_SAFETY_MULTIPLIER + adverse_basis;
    let payback_periods = conservative_required_fraction / funding_rate;
    let payback_hours = payback_periods * funding_interval_hours;
    let stress_net_three_periods =
        MAX_PAYBACK_PERIODS * funding_rate - conservative_required_fraction;
    let l1_round_trip_capacity_usd = (spot.ask_size * spot.ask)
        .min(spot.bid_size * spot.bid)
        .min(perp.bid_size * perp.bid)
        .min(perp.ask_size * perp.ask);
    let minutes_to_funding = (next_funding_time_ms - observed_at_ms) as f64 / 60_000.0;

    let mut rejection_reasons = Vec::new();
    if spot.turnover_24h < MIN_TURNOVER_USD {
        rejection_reasons.push("spot_turnover_low");
    }
    if perp.turnover_24h < MIN_TURNOVER_USD {
        rejection_reasons.push("perp_turnover_low");
    }
    if l1_round_trip_capacity_usd < MIN_L1_CAPACITY_USD {
        rejection_reasons.push("l1_capacity_low");
    }
    if payback_periods > MAX_PAYBACK_PERIODS {
        rejection_reasons.push("payback_too_long");
    }
    if stress_net_three_periods <= 0.0 {
        rejection_reasons.push("stress_net_not_positive");
    }
    let current_screen_pass = rejection_reasons.is_empty();

    Some(CarryCandidate {
        observed_at_ms,
        venue: "bybit",
        symbol: spot.symbol.clone(),
        funding_rate,
        funding_interval_hours,
        next_funding_time_ms,
        minutes_to_funding,
        spot_bid: spot.bid,
        spot_ask: spot.ask,
        perp_bid: perp.bid,
        perp_ask: perp.ask,
        spot_turnover_24h_usd: spot.turnover_24h,
        perp_turnover_24h_usd: perp.turnover_24h,
        l1_round_trip_capacity_usd,
        entry_basis_fraction,
        book_cross_cost_fraction,
        base_all_in_cost_fraction,
        stress_all_in_cost_fraction,
        conservative_required_fraction,
        payback_periods,
        payback_hours,
        stress_net_three_periods,
        current_screen_pass,
        rejection_reasons,
        settled_history: None,
    })
}

fn analyze_settled_history(rates: &[f64], current_cost_fraction: f64) -> SettledHistoryScreen {
    // Blocos não sobrepostos evitam fingir que três janelas deslizantes que
    // compartilham dois dos mesmos settlements são três amostras independentes.
    let windows: Vec<f64> = rates
        .chunks_exact(MAX_PAYBACK_PERIODS as usize)
        .map(|window| window.iter().sum::<f64>() - current_cost_fraction)
        .collect();
    let profitable_windows = windows.iter().filter(|&&net| net > 0.0).count();
    let mean_net_fraction = mean(&windows);
    let lcb95_net_fraction = mean_lcb95(&windows);
    let without_best_lcb95_net_fraction = if windows.len() >= 3 {
        let best_idx = windows
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(idx, _)| idx);
        let without_best: Vec<f64> = windows
            .iter()
            .enumerate()
            .filter_map(|(idx, value)| (Some(idx) != best_idx).then_some(*value))
            .collect();
        mean_lcb95(&without_best)
    } else {
        None
    };
    let pass = rates.len() >= MIN_SETTLED_SAMPLES
        && lcb95_net_fraction.is_some_and(|value| value > 0.0)
        && without_best_lcb95_net_fraction.is_some_and(|value| value > 0.0);
    SettledHistoryScreen {
        settled_samples: rates.len(),
        rolling_three_period_windows: windows.len(),
        profitable_windows,
        profitable_window_fraction: if windows.is_empty() {
            0.0
        } else {
            profitable_windows as f64 / windows.len() as f64
        },
        mean_net_fraction,
        lcb95_net_fraction,
        without_best_lcb95_net_fraction,
        pass,
        limitation: "usa funding liquidado historico com custo executavel do book atual; ainda nao e backtest de fills nem autoriza execucao",
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn mean_lcb95(values: &[f64]) -> Option<f64> {
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

fn persist(common_pairs: usize, candidates: &[CarryCandidate], forward_validation: ForwardSummary) {
    let positive_funding_pairs = candidates.len();
    let current_screened_pairs = candidates
        .iter()
        .filter(|row| row.current_screen_pass)
        .count();
    let history_checked_pairs = candidates
        .iter()
        .filter(|row| row.settled_history.is_some())
        .count();
    let history_screened_pairs = candidates
        .iter()
        .filter(|row| row.settled_history.as_ref().is_some_and(|h| h.pass))
        .count();
    let report_rows = &candidates[..candidates.len().min(REPORT_CANDIDATE_CAP)];
    let report = CarryReport {
        schema: "aurumos.funding_carry.v1",
        generated_at_ms: crate::raw_log::now_ms(),
        venue: "bybit",
        mode: "research_observation_only",
        execution_enabled: false,
        evidence: "public_bybit_v5_spot_and_linear_tickers",
        common_spot_perp_pairs: common_pairs,
        positive_funding_pairs,
        current_screened_pairs,
        history_checked_pairs,
        history_screened_pairs,
        promotion_status: "BLOCKED_NEEDS_FORWARD_SETTLEMENTS_AND_TWO_LEG_EXECUTOR",
        forward_validation,
        assumptions: CarryAssumptions {
            spot_taker_base: SPOT_TAKER_BASE,
            linear_taker_base: LINEAR_TAKER_BASE,
            spot_taker_stress: SPOT_TAKER_STRESS,
            linear_taker_stress: LINEAR_TAKER_STRESS,
            basis_adverse_buffer: BASIS_ADVERSE_BUFFER,
            cost_safety_multiplier: COST_SAFETY_MULTIPLIER,
            max_payback_periods: MAX_PAYBACK_PERIODS,
            min_turnover_usd_each_leg: MIN_TURNOVER_USD,
            min_l1_round_trip_capacity_usd: MIN_L1_CAPACITY_USD,
        },
        candidates: report_rows,
    };

    let report_path = format!(
        "{}/data/funding_carry_validation_v1.json",
        env!("CARGO_MANIFEST_DIR")
    );
    if let Some(parent) = std::path::Path::new(&report_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::write(report_path, json);
    }

    let raw_path = format!(
        "{}/data/raw_funding_carry_bybit.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    for row in candidates {
        if let Ok(value) = serde_json::to_value(row) {
            crate::raw_log::append(&raw_path, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(symbol: &str, bid: f64, ask: f64, funding: Option<f64>) -> TopOfBook {
        TopOfBook {
            symbol: symbol.to_string(),
            bid,
            bid_size: 10_000.0,
            ask,
            ask_size: 10_000.0,
            turnover_24h: 20_000_000.0,
            funding_rate: funding,
            funding_interval_hours: funding.map(|_| 8.0),
            next_funding_time_ms: funding.map(|_| 2_000_000),
        }
    }

    #[test]
    fn ordinary_funding_does_not_pay_back_rapidly() {
        let spot = book("BTCUSDT", 99.99, 100.01, None);
        let perp = book("BTCUSDT", 99.99, 100.01, Some(0.0001));
        let candidate = calculate_candidate(1_000_000, &spot, &perp).unwrap();
        assert!(!candidate.current_screen_pass);
        assert!(candidate.payback_periods > 70.0);
        assert!(candidate.stress_net_three_periods < 0.0);
    }

    #[test]
    fn extreme_funding_still_needs_liquidity_and_stress_margin() {
        let spot = book("TESTUSDT", 100.0, 100.01, None);
        let perp = book("TESTUSDT", 100.02, 100.03, Some(0.004));
        let candidate = calculate_candidate(1_000_000, &spot, &perp).unwrap();
        assert!(candidate.current_screen_pass);
        assert!(candidate.payback_periods < MAX_PAYBACK_PERIODS);
        assert!(candidate.stress_net_three_periods > 0.0);
    }

    #[test]
    fn expired_funding_timestamp_is_rejected() {
        let spot = book("BTCUSDT", 99.99, 100.01, None);
        let mut perp = book("BTCUSDT", 99.99, 100.01, Some(0.004));
        perp.next_funding_time_ms = Some(999_999);
        assert!(calculate_candidate(1_000_000, &spot, &perp).is_none());
    }

    #[test]
    fn settled_history_cannot_pass_on_one_outlier() {
        let mut rates = vec![0.003; MIN_SETTLED_SAMPLES];
        rates[0] = 0.50;
        let screen = analyze_settled_history(&rates, 0.010);
        assert!(!screen.pass);
        assert!(screen.without_best_lcb95_net_fraction.unwrap() < 0.0);
    }

    #[test]
    fn forward_forecast_is_frozen_once_per_symbol_and_settlement() {
        let path = std::env::temp_dir().join(format!(
            "aurumos-funding-forward-{}-{}.jsonl",
            std::process::id(),
            crate::raw_log::now_ms()
        ));
        let mut tracker = ForwardTracker {
            wal_path: path.to_string_lossy().into_owned(),
            records: BTreeMap::new(),
        };
        let spot = book("TESTUSDT", 100.0, 100.01, None);
        let perp = book("TESTUSDT", 100.02, 100.03, Some(0.004));
        let candidate = calculate_candidate(1_000_000, &spot, &perp).unwrap();

        assert_eq!(
            tracker.record_forecasts(std::slice::from_ref(&candidate)),
            1
        );
        assert_eq!(tracker.record_forecasts(&[candidate]), 0);
        let summary = tracker.summary();
        assert_eq!(summary.recorded_forecasts, 1);
        assert_eq!(summary.pending_forecasts, 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn prune_drops_old_resolved_but_keeps_recent_and_pending() {
        let path = std::env::temp_dir().join(format!(
            "aurumos-funding-forward-prune-{}-{}.jsonl",
            std::process::id(),
            crate::raw_log::now_ms()
        ));
        let now = 1_000_000_000_000_u64;
        let mut records = BTreeMap::new();
        records.insert(
            forward_key("OLDUSDT", 1),
            ForwardForecast {
                symbol: "OLDUSDT".to_string(),
                observed_at_ms: 1,
                settlement_time_ms: 1,
                lead_minutes: 0.0,
                predicted_funding_rate: 0.001,
                conservative_required_fraction: 0.001,
                current_screen_pass: false,
                settled_funding_rate: Some(0.001),
                prediction_error: Some(0.0),
                resolved_at_ms: Some(now - FORWARD_RETENTION_MS - 1),
            },
        );
        records.insert(
            forward_key("RECENTUSDT", 2),
            ForwardForecast {
                symbol: "RECENTUSDT".to_string(),
                observed_at_ms: 2,
                settlement_time_ms: 2,
                lead_minutes: 0.0,
                predicted_funding_rate: 0.001,
                conservative_required_fraction: 0.001,
                current_screen_pass: false,
                settled_funding_rate: Some(0.001),
                prediction_error: Some(0.0),
                resolved_at_ms: Some(now - 1_000),
            },
        );
        records.insert(
            forward_key("PENDINGUSDT", 3),
            ForwardForecast {
                symbol: "PENDINGUSDT".to_string(),
                observed_at_ms: 3,
                settlement_time_ms: 3,
                lead_minutes: 0.0,
                predicted_funding_rate: 0.001,
                conservative_required_fraction: 0.001,
                current_screen_pass: false,
                settled_funding_rate: None,
                prediction_error: None,
                resolved_at_ms: None,
            },
        );
        let mut tracker = ForwardTracker {
            wal_path: path.to_string_lossy().into_owned(),
            records,
        };

        tracker.prune(now);

        assert_eq!(tracker.records.len(), 2, "só o antigo liquidado deve sair");
        assert!(!tracker.records.contains_key(&forward_key("OLDUSDT", 1)));
        assert!(tracker.records.contains_key(&forward_key("RECENTUSDT", 2)));
        assert!(tracker.records.contains_key(&forward_key("PENDINGUSDT", 3)));

        // A poda também reescreve o WAL em disco a partir do estado podado.
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("OLDUSDT"));
        assert!(persisted.contains("RECENTUSDT"));
        assert!(persisted.contains("PENDINGUSDT"));

        let _ = std::fs::remove_file(path);
    }
}
