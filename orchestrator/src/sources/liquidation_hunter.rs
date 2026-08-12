use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

const BYBIT_LINEAR_WS_URL: &str = "wss://stream.bybit.com/v5/public/linear";
const CHUNK_SIZE: usize = 10;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);

// Janela em que contamos liquidações como parte da "mesma cascata".
const CASCADE_WINDOW: Duration = Duration::from_secs(30);
// Mínimo de liquidações no mesmo lado, dentro da janela, para considerar
// que virou cascata (não só um evento isolado).
const CASCADE_MIN_COUNT: usize = 5;
// Nocional mínimo somado na janela para o alerta valer a pena.
const CASCADE_MIN_NOTIONAL_USD: f64 = 30_000.0;
const EMIT_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
struct LiqEvent {
    at: Instant,
    side: bool, // true = Buy (posição comprada foi liquidada), false = Sell
    notional_usd: f64,
}

/// Módulo do roadmap que faltava construir: "Liquidation Hunter" — cascatas
/// de liquidação e a recuperação rápida que costuma vir depois. Dado 100%
/// real via `allLiquidation.{symbol}` da Bybit (sem chave de API).
///
/// A leitura clássica: uma cascata de liquidações de posições COMPRADAS
/// (side="Buy" sendo liquidado) empurra o preço pra baixo artificialmente
/// além do que o mercado "quer" — quando as liquidações forçadas acabam,
/// costuma vir uma recuperação técnica. O inverso vale para cascata de
/// posições vendidas. Isso é um padrão observado, não uma lei — por isso,
/// como os outros módulos de detecção, sai com `net_edge = 0.0`
/// (informativo, nunca opera sozinho) até passar por validação histórica
/// de verdade (Fase 10 do roadmap).
pub struct LiquidationHunterSource {
    pub symbols: Vec<String>,
    pub bus: EventBus,
}

impl LiquidationHunterSource {
    pub fn new(symbols: Vec<&str>, bus: EventBus) -> Self {
        Self {
            symbols: symbols.into_iter().map(String::from).collect(),
            bus,
        }
    }
}

#[async_trait::async_trait]
impl SignalSource for LiquidationHunterSource {
    fn name(&self) -> &'static str {
        "liquidation_hunter_bybit"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        let symbols = self.symbols.clone();
        loop {
            if let Err(e) = run_once(&symbols, &tx, &self.bus).await {
                tracing::warn!(error = %e, "conexão de liquidation hunter (Bybit) caiu, reconectando em 3s");
            }
            sleep(Duration::from_secs(3)).await;
        }
    }
}

async fn run_once(symbols: &[String], tx: &Sender<Opportunity>, bus: &EventBus) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(BYBIT_LINEAR_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    let args: Vec<String> = symbols.iter().map(|s| format!("allLiquidation.{s}")).collect();
    for chunk in args.chunks(CHUNK_SIZE) {
        sink.send(WsMessage::Text(serde_json::json!({ "op": "subscribe", "args": chunk }).to_string()))
            .await?;
    }
    tracing::info!(exchange = "bybit_linear", ?symbols, lotes = args.len().div_ceil(CHUNK_SIZE), "liquidation hunter: assinatura enviada");

    let mut windows: HashMap<String, VecDeque<LiqEvent>> = HashMap::new();
    let mut last_emitted: HashMap<String, Instant> = HashMap::new();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut last_msg = Instant::now();
    const READ_TIMEOUT: Duration = Duration::from_secs(25);
    let mut total_seen: u64 = 0;

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
                // "best_edge_pct" aqui carrega a maior contagem de
                // liquidações-na-janela entre os símbolos observados — não
                // é edge de preço, é intensidade de cascata agora.
                let best = windows.iter().map(|(s, w)| (s.clone(), w.len() as f64)).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                if let Some((symbol, count)) = best {
                    bus.emit(DashboardEvent::scan_heartbeat(Strategy::LiquidationHunter, symbols.len() as u32, &symbol, count));
                } else {
                    bus.emit(DashboardEvent::scan_heartbeat(Strategy::LiquidationHunter, symbols.len() as u32, "—", 0.0));
                }
                let _ = total_seen; // mantém vivo para futura métrica de volume total
            }
            msg = stream.next() => {
                last_msg = Instant::now();
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        total_seen += handle_message(&text, &mut windows, &mut last_emitted, tx).await;
                    }
                    Some(Ok(WsMessage::Ping(p))) => { sink.send(WsMessage::Pong(p)).await?; }
                    Some(Ok(WsMessage::Close(_))) | None => anyhow::bail!("conexão fechada pelo servidor"),
                    Some(Err(e)) => anyhow::bail!("erro no stream: {e}"),
                    _ => {}
                }
            }
        }
    }
}

async fn handle_message(
    text: &str,
    windows: &mut HashMap<String, VecDeque<LiqEvent>>,
    last_emitted: &mut HashMap<String, Instant>,
    tx: &Sender<Opportunity>,
) -> u64 {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let Some(topic) = v.get("topic").and_then(Value::as_str) else {
        return 0;
    };
    if !topic.starts_with("allLiquidation.") {
        return 0;
    }
    let Some(entries) = v.get("data").and_then(Value::as_array) else {
        return 0;
    };

    let mut count = 0u64;
    for entry in entries {
        let Some(symbol) = entry.get("s").and_then(Value::as_str) else { continue };
        let Some(side_str) = entry.get("S").and_then(Value::as_str) else { continue };
        let Some(size) = entry
            .get("v")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<f64>().ok())
        else {
            continue;
        };
        let Some(price) = entry
            .get("p")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<f64>().ok())
        else {
            continue;
        };

        let side = side_str == "Buy";
        let notional_usd = size * price;
        count += 1;

        let window = windows.entry(symbol.to_string()).or_default();
        window.push_back(LiqEvent { at: Instant::now(), side, notional_usd });
        while window.front().map(|e| e.at.elapsed() > CASCADE_WINDOW).unwrap_or(false) {
            window.pop_front();
        }

        let same_side_count = window.iter().filter(|e| e.side == side).count();
        let same_side_notional: f64 = window.iter().filter(|e| e.side == side).map(|e| e.notional_usd).sum();

        if same_side_count < CASCADE_MIN_COUNT || same_side_notional < CASCADE_MIN_NOTIONAL_USD {
            continue;
        }

        if let Some(t) = last_emitted.get(symbol) {
            if t.elapsed() < EMIT_COOLDOWN {
                continue;
            }
        }
        last_emitted.insert(symbol.to_string(), Instant::now());

        let liquidated_longs = side; // Buy = posição comprada sendo liquidada
        tracing::info!(
            symbol,
            liquidated_longs,
            same_side_count,
            same_side_notional_usd = same_side_notional,
            "cascata de liquidação detectada"
        );

        let opp = Opportunity {
            market: Market::Crypto,
            strategy: Strategy::LiquidationHunter,
            asset: format!(
                "{symbol} (cascata {}, {same_side_count}x, ${same_side_notional:.0})",
                if liquidated_longs { "longs" } else { "shorts" }
            ),
            // Leitura clássica de mean-reversion: cascata de longs liquidados
            // → possível recuperação (Long); cascata de shorts liquidados →
            // possível continuação de alta forçada (Short como contra-sinal
            // não modelado aqui — mantemos Long/Short espelhando o lado
            // liquidado, já que é isso que o preço acabou de fazer).
            direction: if liquidated_longs { Direction::Long } else { Direction::Short },
            net_edge: 0.0,
            confidence: 0.3,
            valid_for_ms: 45_000,
            // Informativo (net_edge=0) — placeholder consistente com valid_for_ms.
            expected_holding_secs: 45.0,
            capital_needed: 10.0,
            max_loss_pct: 0.02,
            leverage: 1.0,
            correlation_group: "altcoins".to_string(),
            emitted_at: Instant::now(),
        };
        let _ = tx.send(opp).await;
    }
    count
}
