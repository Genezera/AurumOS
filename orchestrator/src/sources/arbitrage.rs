use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc::Sender, watch, RwLock};
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::sources::{best_level, SignalSource};
use crate::types::{Direction, Market, Opportunity, Strategy};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
// Snapshot bruto pra disco (book comparado das duas exchanges, todos os
// simbolos) — mesma motivacao do Pump Exhaustion: dado continuo pra analise
// futura, nao so o que ja cruzou MIN_NET_EDGE.
const RAW_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

const BYBIT_WS_URL: &str = "wss://stream.bybit.com/v5/public/spot";
const BITGET_WS_URL: &str = "wss://ws.bitget.com/v2/ws/public";

// Estimativa de taxa taker por perna. Contas com desconto (VIP, token de
// taxa) pagam menos — ajuste aqui se for o seu caso, senão o motor de risco
// vai subestimar o lucro líquido real. Cada captura de spread aqui é
// modelada como UMA perna vendida na exchange mais cara + UMA perna comprada
// na mais barata (você já mantém saldo pré-financiado nas duas, como
// planejado: US$100 em cada) — não há transferência entre exchanges por
// operação, então não há taxa de rede/saque neste cálculo.
const BYBIT_TAKER_FEE: f64 = 0.001;
const BITGET_TAKER_FEE: f64 = 0.001;
const ROUND_TRIP_FEE: f64 = BYBIT_TAKER_FEE + BITGET_TAKER_FEE;

// Achado da análise de breakeven (backtests/edge_threshold_analysis.py,
// 12/08/2026): o próprio modelo de confiança usado na simulação
// (confidence = 0.5 + edge*25, clamp 0.4–0.9) implica que o valor esperado
// só fica positivo a partir de ~0,54% de edge líquido — abaixo disso, o
// ganho médio esperado (confidence*edge) não cobre a perda esperada
// ((1-confidence)*max_loss_pct). O limiar antigo de 0,06% deixava passar
// operações que o PRÓPRIO sistema já sabia ser -EV, o que bate exatamente
// com o achado real: 56%+ de acerto e ainda assim prejuízo líquido
// acumulado. Ajustado com uma margem de segurança acima do breakeven
// teórico (0,544%) para cobrir custos de execução não modelados.
const MIN_NET_EDGE: f64 = 0.0065;
// Não reemitir a mesma direção/símbolo com mais frequência que isso — o book
// atualiza a cada poucos milissegundos, mas o orquestrador não precisa (nem
// quer) reavaliar a mesma oportunidade centenas de vezes por segundo.
const EMIT_COOLDOWN: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, Default)]
struct TopOfBook {
    bid: f64,
    bid_qty: f64,
    ask: f64,
    ask_qty: f64,
}

impl TopOfBook {
    fn is_valid(&self) -> bool {
        self.bid > 0.0 && self.ask > 0.0 && self.ask > self.bid
    }
}

type BookMap = Arc<RwLock<HashMap<String, TopOfBook>>>;

/// Fase 1 do roadmap: compara o topo do book da Bybit e da Bitget para os
/// mesmos pares e emite `Opportunity` reais quando o spread líquido (já
/// descontando taxas estimadas) supera `MIN_NET_EDGE`. Dado 100% real via
/// WebSocket público — não precisa de chave de API para isso.
pub struct ArbitrageSource {
    pub symbols_rx: watch::Receiver<Vec<String>>,
    pub bus: EventBus,
}

impl ArbitrageSource {
    pub fn new(symbols_rx: watch::Receiver<Vec<String>>, bus: EventBus) -> Self {
        Self { symbols_rx, bus }
    }
}

#[async_trait::async_trait]
impl SignalSource for ArbitrageSource {
    fn name(&self) -> &'static str {
        "arbitrage_bybit_bitget"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        loop {
            let symbols = self.symbols_rx.borrow().clone();
            let bybit_books: BookMap = Arc::new(RwLock::new(HashMap::new()));
            let bitget_books: BookMap = Arc::new(RwLock::new(HashMap::new()));

            let b1 = bybit_books.clone();
            let syms1 = symbols.clone();
            let bybit_handle = tokio::spawn(async move { run_bybit_forever(syms1, b1).await });

            let b2 = bitget_books.clone();
            let syms2 = symbols.clone();
            let bitget_handle = tokio::spawn(async move { run_bitget_forever(syms2, b2).await });

            let tx2 = tx.clone();
            let bus2 = self.bus.clone();
            let comparator_handle = tokio::spawn(async move {
                run_comparator(symbols, bybit_books, bitget_books, tx2, bus2).await
            });

            // As 3 tarefas rodam pra sempre sozinhas (cada uma já reconecta
            // em erro internamente) — o único motivo pra voltar aqui é o
            // universo de símbolos mudar, e aí reinicia tudo com a lista nova.
            let _ = self.symbols_rx.changed().await;
            tracing::info!("arbitragem: universo de símbolos atualizado — reiniciando conexões com lista nova");
            bybit_handle.abort();
            bitget_handle.abort();
            comparator_handle.abort();
        }
    }
}

// ---------------- Bybit ----------------

async fn run_bybit_forever(symbols: Vec<String>, books: BookMap) {
    loop {
        if let Err(e) = run_bybit_once(&symbols, &books).await {
            tracing::warn!(error = %e, "conexão com Bybit caiu, reconectando em 3s");
        }
        sleep(Duration::from_secs(3)).await;
    }
}

async fn run_bybit_once(symbols: &[String], books: &BookMap) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(BYBIT_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    // Bybit spot parou de mandar dado quando enviamos os 30 tópicos numa
    // única mensagem de subscribe (sem erro explícito — só silêncio total).
    // Lotes de 10 por mensagem é o padrão comum entre APIs de exchange e
    // resolveu; se a Bybit documentar um limite oficial diferente no
    // futuro, ajuste `CHUNK_SIZE`.
    const CHUNK_SIZE: usize = 10;
    let args: Vec<String> = symbols.iter().map(|s| format!("orderbook.1.{s}")).collect();
    for chunk in args.chunks(CHUNK_SIZE) {
        let sub = serde_json::json!({ "op": "subscribe", "args": chunk });
        sink.send(WsMessage::Text(sub.to_string())).await?;
    }
    tracing::info!(exchange = "bybit", ?symbols, lotes = args.len().div_ceil(CHUNK_SIZE), "assinatura de orderbook enviada");

    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    // Watchdog: se o provedor derrubar a conexão sem mandar um Close frame,
    // `stream.next()` ficaria pendurado para sempre e o reconector nunca
    // seria acionado. Book de cripto líquido nunca fica 25s em silêncio.
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
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
            msg = stream.next() => {
                last_msg = Instant::now();
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if !text.starts_with("{\"topic\"") {
                            tracing::debug!(raw = %text.chars().take(300).collect::<String>(), "bybit: mensagem não-topic (diagnóstico)");
                        }
                        handle_bybit_message(&text, books).await;
                    }
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

async fn handle_bybit_message(text: &str, books: &BookMap) {
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

    let mut guard = books.write().await;
    let entry = guard.entry(symbol.to_string()).or_default();
    if let Some(bid) = best_level(data.get("b")) {
        entry.bid = bid.0;
        entry.bid_qty = bid.1;
    }
    if let Some(ask) = best_level(data.get("a")) {
        entry.ask = ask.0;
        entry.ask_qty = ask.1;
    }
}

// ---------------- Bitget ----------------

async fn run_bitget_forever(symbols: Vec<String>, books: BookMap) {
    loop {
        if let Err(e) = run_bitget_once(&symbols, &books).await {
            tracing::warn!(error = %e, "conexão com Bitget caiu, reconectando em 3s");
        }
        sleep(Duration::from_secs(3)).await;
    }
}

async fn run_bitget_once(symbols: &[String], books: &BookMap) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(BITGET_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    let args: Vec<Value> = symbols
        .iter()
        .map(|s| serde_json::json!({ "instType": "SPOT", "channel": "books1", "instId": s }))
        .collect();
    let sub = serde_json::json!({ "op": "subscribe", "args": args });
    sink.send(WsMessage::Text(sub.to_string())).await?;
    tracing::info!(exchange = "bitget", ?symbols, "assinatura de orderbook enviada");

    let mut ping_interval = tokio::time::interval(Duration::from_secs(25));
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut last_msg = Instant::now();
    const READ_TIMEOUT: Duration = Duration::from_secs(25);
    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                sink.send(WsMessage::Text("ping".to_string())).await?;
            }
            _ = watchdog.tick() => {
                if last_msg.elapsed() > READ_TIMEOUT {
                    anyhow::bail!("nenhuma mensagem em {}s — conexão provavelmente morta", READ_TIMEOUT.as_secs());
                }
            }
            msg = stream.next() => {
                last_msg = Instant::now();
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if text != "pong" {
                            handle_bitget_message(&text, books).await;
                        }
                    }
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

async fn handle_bitget_message(text: &str, books: &BookMap) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(symbol) = v
        .get("arg")
        .and_then(|a| a.get("instId"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let Some(entry_data) = v.get("data").and_then(Value::as_array).and_then(|a| a.first()) else {
        return;
    };

    let mut guard = books.write().await;
    let entry = guard.entry(symbol.to_string()).or_default();
    if let Some(bid) = best_level(entry_data.get("bids")) {
        entry.bid = bid.0;
        entry.bid_qty = bid.1;
    }
    if let Some(ask) = best_level(entry_data.get("asks")) {
        entry.ask = ask.0;
        entry.ask_qty = ask.1;
    }
}

// ---------------- comparador ----------------

async fn run_comparator(symbols: Vec<String>, bybit: BookMap, bitget: BookMap, tx: Sender<Opportunity>, bus: EventBus) {
    let mut last_emitted: HashMap<(String, &'static str), Instant> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(150));
    let mut last_diag = Instant::now() - Duration::from_secs(60);
    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    let mut last_raw_snapshot = Instant::now() - RAW_SNAPSHOT_INTERVAL;
    let raw_log_path = format!("{}/data/raw_arbitrage.jsonl", env!("CARGO_MANIFEST_DIR"));
    let mut best_this_window: Option<(String, f64)> = None;
    loop {
        tick.tick().await;
        let raw_snapshot_now = last_raw_snapshot.elapsed() >= RAW_SNAPSHOT_INTERVAL;
        if raw_snapshot_now {
            last_raw_snapshot = Instant::now();
        }
        let diag_now = last_diag.elapsed() >= Duration::from_secs(10);
        if diag_now {
            last_diag = Instant::now();
        }
        for symbol in &symbols {
            let (bb, bg) = {
                let a = bybit.read().await;
                let b = bitget.read().await;
                (a.get(symbol).copied(), b.get(symbol).copied())
            };
            let (Some(bb), Some(bg)) = (bb, bg) else {
                if diag_now {
                    tracing::debug!(symbol, bybit_seen = bb.is_some(), bitget_seen = bg.is_some(), "book ainda sem dado de uma das exchanges");
                }
                continue;
            };
            if !bb.is_valid() || !bg.is_valid() {
                if diag_now {
                    tracing::debug!(symbol, ?bb, ?bg, "book recebido mas inválido (bid/ask zerado ou invertido)");
                }
                continue;
            }

            // Direção 1: comprar na Bitget (ask), vender na Bybit (bid).
            let edge_1 = (bb.bid - bg.ask) / bg.ask - ROUND_TRIP_FEE;
            // Direção 2: comprar na Bybit (ask), vender na Bitget (bid).
            let edge_2 = (bg.bid - bb.ask) / bb.ask - ROUND_TRIP_FEE;

            if diag_now {
                tracing::debug!(
                    symbol, bybit_bid = bb.bid, bybit_ask = bb.ask,
                    bitget_bid = bg.bid, bitget_ask = bg.ask,
                    edge_1_pct = edge_1 * 100.0, edge_2_pct = edge_2 * 100.0,
                    "snapshot de book comparado"
                );
            }
            if raw_snapshot_now {
                crate::raw_log::append(&raw_log_path, serde_json::json!({
                    "ts_ms": crate::raw_log::now_ms(),
                    "symbol": symbol,
                    "bybit_bid": bb.bid, "bybit_ask": bb.ask,
                    "bitget_bid": bg.bid, "bitget_ask": bg.ask,
                    "edge_1": edge_1, "edge_2": edge_2,
                }));
            }

            if edge_1 > MIN_NET_EDGE {
                maybe_emit(&tx, &mut last_emitted, symbol, "buy_bitget_sell_bybit", edge_1, bg.ask_qty * bg.ask, &tx_asset(symbol)).await;
            }
            if edge_2 > MIN_NET_EDGE {
                maybe_emit(&tx, &mut last_emitted, symbol, "buy_bybit_sell_bitget", edge_2, bb.ask_qty * bb.ask, &tx_asset(symbol)).await;
            }

            let best_edge_here = edge_1.max(edge_2);
            if best_this_window.as_ref().map(|(_, e)| best_edge_here > *e).unwrap_or(true) {
                best_this_window = Some((symbol.clone(), best_edge_here));
            }
        }

        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            last_heartbeat = Instant::now();
            if let Some((symbol, edge)) = best_this_window.take() {
                bus.emit(DashboardEvent::scan_heartbeat(Strategy::Arbitrage, symbols.len() as u32, &symbol, edge * 100.0));
            }
        }
    }
}

fn tx_asset(symbol: &str) -> String {
    symbol.to_string()
}

async fn maybe_emit(
    tx: &Sender<Opportunity>,
    last_emitted: &mut HashMap<(String, &'static str), Instant>,
    symbol: &str,
    direction_key: &'static str,
    net_edge: f64,
    notional_available: f64,
    asset: &str,
) {
    let key = (symbol.to_string(), direction_key);
    if let Some(t) = last_emitted.get(&key) {
        if t.elapsed() < EMIT_COOLDOWN {
            return;
        }
    }
    last_emitted.insert(key, Instant::now());

    // Nunca reivindicar mais capital do que o topo do book realmente
    // suporta, e nunca acima de um teto de sanidade — o motor de risco ainda
    // vai clampar isso ao tamanho de perna configurado.
    let capital_needed = notional_available.min(50.0).max(5.0);
    // Risco de execução de arbitragem (uma perna preenche, a outra não a
    // tempo, ou o preço se move entre as duas pontas) — não é uma distância
    // de stop como em trades direcionais; usamos uma estimativa fixa
    // conservadora até termos dado real de fill-rate para calibrar.
    let max_loss_pct = 0.004;
    let confidence = (0.5 + net_edge * 25.0).clamp(0.4, 0.9);

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::Arbitrage,
        asset: asset.to_string(),
        direction: Direction::Long,
        net_edge,
        confidence,
        valid_for_ms: 900,
        // Duas pernas em duas exchanges, mas cada uma resolve em
        // milissegundos — segundos de sobra pra latência de rede real.
        expected_holding_secs: 3.0,
        capital_needed,
        max_loss_pct,
        leverage: 1.0,
        correlation_group: "cross_exchange".to_string(),
        emitted_at: Instant::now(),
    };

    let _ = tx.send(opp).await;
}
