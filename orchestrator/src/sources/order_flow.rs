use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::sources::{best_level, SignalSource};
use crate::types::{Direction, Market, Opportunity, Strategy};

const BYBIT_WS_URL: &str = "wss://stream.bybit.com/v5/public/spot";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
// Mesma motivacao do log bruto de arbitrage.rs/pump_exhaustion.rs: dado
// continuo de spread pra analise futura, nao so o que ja cruzou MIN_NET_EDGE.
const RAW_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default)]
struct SpreadSnapshot {
    bid: f64,
    ask: f64,
    net_edge: f64,
}

// Fase 1 (arbitragem taker) mostrou que o spread bruto entre Bybit e Bitget
// nesses pares é quase zero — não sobra nada depois de duas taxas taker.
// Aqui a aposta é diferente: cotar como MAKER (adiciona liquidez em vez de
// consumir) paga uma taxa menor, então o próprio spread interno do book já
// pode compensar. Isso não modela fila de execução real (quem chega
// primeiro no preço, risco de seleção adversa) — é uma aproximação inicial
// que assume as duas pontas preenchem; por isso a confiança fica baixa.
const MAKER_FEE: f64 = 0.0008;
// Achado da análise de breakeven (backtests/edge_threshold_analysis.py,
// 12/08/2026): confidence aqui é FIXA em 0.45 (abaixo de 50% de propósito,
// por ser uma aproximação que ignora fila de execução real) — isso implica
// breakeven em ~0,367% de edge líquido (confidence*edge = (1-confidence)*max_loss_pct).
// Qualquer operação abaixo disso já é -EV pelo próprio modelo, o que bate
// com o achado real (43% de acerto, prejuízo líquido acumulado). Antigo
// limiar de 0,04% deixava passar isso o tempo todo.
const MIN_NET_EDGE: f64 = 0.0045;
const EMIT_COOLDOWN: Duration = Duration::from_millis(700);

pub struct OrderFlowSource {
    pub symbols: Vec<String>,
    pub bus: EventBus,
}

impl OrderFlowSource {
    pub fn new(symbols: Vec<&str>, bus: EventBus) -> Self {
        Self {
            symbols: symbols.into_iter().map(String::from).collect(),
            bus,
        }
    }
}

#[async_trait::async_trait]
impl SignalSource for OrderFlowSource {
    fn name(&self) -> &'static str {
        "order_flow_bybit"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        let symbols = self.symbols.clone();
        loop {
            if let Err(e) = run_once(&symbols, &tx, &self.bus).await {
                tracing::warn!(error = %e, "conexão de order flow (Bybit) caiu, reconectando em 3s");
            }
            sleep(Duration::from_secs(3)).await;
        }
    }
}

async fn run_once(symbols: &[String], tx: &Sender<Opportunity>, bus: &EventBus) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(BYBIT_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    // Mesmo limite silencioso encontrado na arbitragem: Bybit spot para de
    // mandar dado com muitos tópicos numa única mensagem. Lotes de 10.
    const CHUNK_SIZE: usize = 10;
    let args: Vec<String> = symbols.iter().map(|s| format!("orderbook.1.{s}")).collect();
    for chunk in args.chunks(CHUNK_SIZE) {
        sink.send(WsMessage::Text(
            serde_json::json!({ "op": "subscribe", "args": chunk }).to_string(),
        ))
        .await?;
    }
    tracing::info!(exchange = "bybit", ?symbols, lotes = args.len().div_ceil(CHUNK_SIZE), "order flow: assinatura de book enviada");

    let mut last_emitted: HashMap<String, Instant> = HashMap::new();
    let mut last_diag: HashMap<String, Instant> = HashMap::new();
    let mut best_seen: HashMap<String, f64> = HashMap::new();
    let mut snapshots: HashMap<String, SpreadSnapshot> = HashMap::new();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut raw_snapshot = tokio::time::interval(RAW_SNAPSHOT_INTERVAL);
    let raw_log_path = format!("{}/data/raw_order_flow.jsonl", env!("CARGO_MANIFEST_DIR"));
    let mut last_msg = Instant::now();
    const READ_TIMEOUT: Duration = Duration::from_secs(25);
    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                sink.send(WsMessage::Text(serde_json::json!({"op":"ping"}).to_string())).await?;
            }
            _ = watchdog.tick() => {
                if last_msg.elapsed() > READ_TIMEOUT {
                    anyhow::bail!("nenhuma mensagem em {}s — conexão provavelmente morta", READ_TIMEOUT.as_secs());
                }
            }
            _ = heartbeat.tick() => {
                if let Some((symbol, edge)) = best_seen.iter().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()) {
                    bus.emit(DashboardEvent::scan_heartbeat(Strategy::OrderFlow, symbols.len() as u32, symbol, edge * 100.0));
                }
            }
            _ = raw_snapshot.tick() => {
                let ts_ms = crate::raw_log::now_ms();
                for (symbol, s) in snapshots.iter() {
                    crate::raw_log::append(&raw_log_path, serde_json::json!({
                        "ts_ms": ts_ms,
                        "symbol": symbol,
                        "bid": s.bid,
                        "ask": s.ask,
                        "net_edge": s.net_edge,
                    }));
                }
            }
            msg = stream.next() => {
                last_msg = Instant::now();
                match msg {
                    Some(Ok(WsMessage::Text(text))) => handle_message(&text, tx, &mut last_emitted, &mut last_diag, &mut best_seen, &mut snapshots).await,
                    Some(Ok(WsMessage::Ping(p))) => { sink.send(WsMessage::Pong(p)).await?; }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        anyhow::bail!("conexão fechada pelo servidor");
                    }
                    Some(Err(e)) => anyhow::bail!("erro no stream: {e}"),
                    _ => {}
                }
            }
        }
    }
}

async fn handle_message(
    text: &str,
    tx: &Sender<Opportunity>,
    last_emitted: &mut HashMap<String, Instant>,
    last_diag: &mut HashMap<String, Instant>,
    best_seen: &mut HashMap<String, f64>,
    snapshots: &mut HashMap<String, SpreadSnapshot>,
) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(topic) = v.get("topic").and_then(Value::as_str) else {
        return;
    };
    if !topic.starts_with("orderbook.") {
        return;
    }
    let Some(data) = v.get("data") else { return };
    let Some(symbol) = data.get("s").and_then(Value::as_str) else {
        return;
    };
    let Some(bid) = best_level(data.get("b")) else { return };
    let Some(ask) = best_level(data.get("a")) else { return };
    if bid.0 <= 0.0 || ask.0 <= bid.0 {
        return;
    }

    let mid = (bid.0 + ask.0) / 2.0;
    let spread_pct = (ask.0 - bid.0) / mid;
    let net_edge = spread_pct - 2.0 * MAKER_FEE;
    best_seen.insert(symbol.to_string(), net_edge);
    snapshots.insert(symbol.to_string(), SpreadSnapshot { bid: bid.0, ask: ask.0, net_edge });

    let should_diag = last_diag
        .get(symbol)
        .map(|t| t.elapsed() >= Duration::from_secs(10))
        .unwrap_or(true);
    if should_diag {
        last_diag.insert(symbol.to_string(), Instant::now());
        tracing::debug!(symbol, bid = bid.0, ask = ask.0, spread_pct = spread_pct * 100.0, net_edge_pct = net_edge * 100.0, "snapshot de spread (order flow)");
    }

    if net_edge <= MIN_NET_EDGE {
        return;
    }

    if let Some(t) = last_emitted.get(symbol) {
        if t.elapsed() < EMIT_COOLDOWN {
            return;
        }
    }
    last_emitted.insert(symbol.to_string(), Instant::now());

    let notional = bid.1.min(ask.1) * mid;
    let capital_needed = notional.min(40.0).max(5.0);

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::OrderFlow,
        asset: symbol.to_string(),
        direction: Direction::Long,
        net_edge,
        confidence: 0.45,
        valid_for_ms: 600,
        // Cotação passiva (maker) — não preenche instantaneamente como uma
        // ordem a mercado; 30s é uma estimativa de espera até o fill, não
        // validada contra fill real (não temos como medir isso em paper
        // trading sem livro de ordens de verdade).
        expected_holding_secs: 30.0,
        capital_needed,
        max_loss_pct: 0.003,
        leverage: 1.0,
        correlation_group: "cross_exchange".to_string(),
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}
