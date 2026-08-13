use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::sources::{best_level, SignalSource};
use crate::symbol_universe::{record_edge, EdgeScores};
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
// pode compensar.
//
// Corrigido (13/08/2026, auditoria externa): estava 0,08%, assumindo
// desconto de conta (VIP/token de taxa) sem confirmar que a conta real
// teria isso. A taxa maker publicada padrão (não-VIP) da Bybit é 0,10% —
// não exposta na API pública de mercado (precisaria de autenticação, como
// a da Bitget não precisa), então uso o valor conservador/publicado até
// haver como confirmar um desconto real.
const MAKER_FEE: f64 = 0.001;
const MIN_NET_EDGE: f64 = 0.0045;
const EMIT_COOLDOWN: Duration = Duration::from_millis(700);

// Camada de confirmação por preço (achado ao vivo, 13/08/2026, pedido do
// usuário: "eu realmente não quero nada falso... taxas, movimentações,
// operações, tudo completamente próximo ou replicado da realidade"):
// substitui o `confidence=0.45` fixo (chute nunca validado) pelo mesmo
// mecanismo já provado no Pump Exhaustion — registra o preço no instante do
// sinal, espera uma janela real, e só confia no edge/confiança depois de
// confirmar contra o preço real subsequente. Antes de ter amostra
// suficiente, a estratégia fica informativa (net_edge=0.0), igual a
// Whale Watch/News/Macro hoje — nenhum trade sintético é contado como
// lucro.
//
// Janela curta (30s, ~expected_holding_secs) porque Order Flow é uma
// cotação passiva de segundos, não uma aposta de minutos como Pump
// Exhaustion. MIN_CONFIRMATION_SAMPLES mais alto (50, não 20) porque o
// volume de sinais é muito maior — dá pra exigir mais rigor estatístico
// sem esperar muito tempo real.
const CONFIRMATION_WINDOW: Duration = Duration::from_secs(30);
const MIN_CONFIRMATION_SAMPLES: usize = 50;
const CONFIRMATION_HISTORY_CAP: usize = 1000;
const CONFIRMATION_CHECK_INTERVAL: Duration = Duration::from_secs(5);

struct PendingConfirmation {
    symbol: String,
    entry_mid: f64,
    entry_edge: f64,
    fired_at: Instant,
}

fn confirmation_history_path() -> String {
    format!("{}/data/confirmation_history_order_flow.json", env!("CARGO_MANIFEST_DIR"))
}

fn load_confirmation_history() -> VecDeque<f64> {
    let path = confirmation_history_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        tracing::info!(path, "order flow: nenhum histórico de confirmação salvo, começando do zero");
        return VecDeque::new();
    };
    match serde_json::from_str::<Vec<f64>>(&raw) {
        Ok(v) => {
            tracing::info!(amostras = v.len(), "order flow: histórico de confirmação real recuperado do disco");
            v.into()
        }
        Err(e) => {
            tracing::warn!(error = %e, "order flow: histórico de confirmação salvo corrompido, começando do zero");
            VecDeque::new()
        }
    }
}

fn save_confirmation_history(history: &VecDeque<f64>) {
    let path = confirmation_history_path();
    let items: Vec<f64> = history.iter().copied().collect();
    let Ok(json) = serde_json::to_string(&items) else { return };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, json);
}

pub struct OrderFlowSource {
    pub symbols_rx: watch::Receiver<Vec<String>>,
    pub bus: EventBus,
    pub edge_scores: EdgeScores,
}

impl OrderFlowSource {
    pub fn new(symbols_rx: watch::Receiver<Vec<String>>, bus: EventBus, edge_scores: EdgeScores) -> Self {
        Self { symbols_rx, bus, edge_scores }
    }
}

#[async_trait::async_trait]
impl SignalSource for OrderFlowSource {
    fn name(&self) -> &'static str {
        "order_flow_bybit"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        // Vivem fora de run_once de propósito (mesmo raciocínio do Pump
        // Exhaustion): uma queda de conexão ou rotação de universo (a cada
        // 15min) não pode apagar o histórico de confirmação acumulado — só
        // ele que torna net_edge real possível.
        //
        // Persistido em disco (auditoria externa, 13/08/2026: "refilar
        // rápido não é aceitável — pode ser exatamente o problema de
        // contar várias observações correlacionadas [se refizer do zero a
        // cada restart, no início da janela as poucas amostras que
        // existem tendem a vir do mesmo movimento de mercado]"). Mesmo
        // padrão já usado pra edge_scores: carrega no boot, salva
        // periodicamente.
        let mut confirmation_history: VecDeque<f64> = load_confirmation_history();
        let mut pending_confirmations: Vec<PendingConfirmation> = Vec::new();
        loop {
            let symbols = self.symbols_rx.borrow().clone();
            tokio::select! {
                result = run_once(&symbols, &tx, &self.bus, &self.edge_scores, &mut confirmation_history, &mut pending_confirmations) => {
                    if let Err(e) = result {
                        tracing::warn!(error = %e, "conexão de order flow (Bybit) caiu, reconectando em 3s");
                    }
                    sleep(Duration::from_secs(3)).await;
                }
                _ = self.symbols_rx.changed() => {
                    tracing::info!("order flow: universo de símbolos atualizado — reconectando com lista nova");
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    symbols: &[String],
    tx: &Sender<Opportunity>,
    bus: &EventBus,
    edge_scores: &EdgeScores,
    confirmation_history: &mut VecDeque<f64>,
    pending_confirmations: &mut Vec<PendingConfirmation>,
) -> anyhow::Result<()> {
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
    let mut confirmation_check = tokio::time::interval(CONFIRMATION_CHECK_INTERVAL);
    let mut confirmation_save = tokio::time::interval(Duration::from_secs(30));
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
            _ = confirmation_save.tick() => {
                save_confirmation_history(confirmation_history);
            }
            _ = confirmation_check.tick() => {
                // Passou a janela de confirmação — confere o preço real
                // AGORA contra o preço no instante do sinal. Se o mercado
                // andou mais que o próprio edge esperado, o edge foi
                // corroído (ou invertido) por movimento real — não por
                // sorteio.
                let now = Instant::now();
                let mut still_pending = Vec::with_capacity(pending_confirmations.len());
                for p in pending_confirmations.drain(..) {
                    if now.duration_since(p.fired_at) < CONFIRMATION_WINDOW {
                        still_pending.push(p);
                        continue;
                    }
                    if let Some(s) = snapshots.get(&p.symbol) {
                        let current_mid = (s.bid + s.ask) / 2.0;
                        if p.entry_mid > 0.0 && current_mid > 0.0 {
                            let pct_move_abs = ((current_mid - p.entry_mid) / p.entry_mid).abs();
                            let realized_return = p.entry_edge - pct_move_abs;
                            confirmation_history.push_back(realized_return);
                            while confirmation_history.len() > CONFIRMATION_HISTORY_CAP {
                                confirmation_history.pop_front();
                            }
                        }
                    }
                }
                *pending_confirmations = still_pending;
            }
            msg = stream.next() => {
                last_msg = Instant::now();
                match msg {
                    Some(Ok(WsMessage::Text(text))) => handle_message(&text, tx, &mut last_emitted, &mut last_diag, &mut best_seen, &mut snapshots, edge_scores, pending_confirmations, confirmation_history).await,
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

/// Calcula (net_edge, confidence) a partir do histórico real de confirmações
/// — retorno médio e taxa de acerto, ambos medidos contra o preço real
/// subsequente, nunca escolhidos a dedo. `None` enquanto a amostra for
/// pequena demais pra significar algo (mesmo padrão do Pump Exhaustion).
fn empirical_edge(history: &VecDeque<f64>) -> Option<(f64, f64)> {
    if history.len() < MIN_CONFIRMATION_SAMPLES {
        return None;
    }
    let avg_return = history.iter().sum::<f64>() / history.len() as f64;
    let win_rate = history.iter().filter(|&&r| r > 0.0).count() as f64 / history.len() as f64;
    Some((avg_return.max(0.0), win_rate.clamp(0.1, 0.9)))
}

#[allow(clippy::too_many_arguments)]
async fn handle_message(
    text: &str,
    tx: &Sender<Opportunity>,
    last_emitted: &mut HashMap<String, Instant>,
    last_diag: &mut HashMap<String, Instant>,
    best_seen: &mut HashMap<String, f64>,
    snapshots: &mut HashMap<String, SpreadSnapshot>,
    edge_scores: &EdgeScores,
    pending_confirmations: &mut Vec<PendingConfirmation>,
    confirmation_history: &VecDeque<f64>,
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
    let instant_edge = spread_pct - 2.0 * MAKER_FEE;
    best_seen.insert(symbol.to_string(), instant_edge);
    snapshots.insert(symbol.to_string(), SpreadSnapshot { bid: bid.0, ask: ask.0, net_edge: instant_edge });
    record_edge(edge_scores, symbol, instant_edge);

    let should_diag = last_diag
        .get(symbol)
        .map(|t| t.elapsed() >= Duration::from_secs(10))
        .unwrap_or(true);
    if should_diag {
        last_diag.insert(symbol.to_string(), Instant::now());
        tracing::debug!(symbol, bid = bid.0, ask = ask.0, spread_pct = spread_pct * 100.0, net_edge_pct = instant_edge * 100.0, "snapshot de spread (order flow)");
    }

    if instant_edge <= MIN_NET_EDGE {
        return;
    }

    if let Some(t) = last_emitted.get(symbol) {
        if t.elapsed() < EMIT_COOLDOWN {
            return;
        }
    }
    last_emitted.insert(symbol.to_string(), Instant::now());

    // Registra a hipótese pra ser conferida contra o preço real daqui a
    // CONFIRMATION_WINDOW — isso acontece SEMPRE que o gate de fee é
    // cruzado, independente de já ter amostra suficiente pra confiar
    // (é assim que a amostra cresce).
    pending_confirmations.push(PendingConfirmation {
        symbol: symbol.to_string(),
        entry_mid: mid,
        entry_edge: instant_edge,
        fired_at: Instant::now(),
    });

    let (net_edge, confidence, confirmation_note) = match empirical_edge(confirmation_history) {
        Some((edge, conf)) => {
            let wins = confirmation_history.iter().filter(|&&r| r > 0.0).count();
            (edge, conf, format!(
                "edge instantâneo {:+.3}%, confirmação real: {wins}/{} acertos, retorno médio {:+.3}%",
                instant_edge * 100.0, confirmation_history.len(), edge * 100.0
            ))
        }
        None => (0.0, 0.3, format!(
            "edge instantâneo {:+.3}%, aguardando confirmação ({}/{MIN_CONFIRMATION_SAMPLES} amostras)",
            instant_edge * 100.0, confirmation_history.len()
        )),
    };

    // Reamostragem real (auditoria externa, 13/08/2026): em vez de deixar
    // o orquestrador decidir ganhou/perdeu por sorteio ponderado pela
    // média (`confidence`/`net_edge` acima), sorteia AQUI um desfecho que
    // REALMENTE aconteceu — um valor real do histórico de confirmação,
    // não uma fórmula. `None` (estratégia sem amostra suficiente ainda)
    // faz o orquestrador cair no sorteio antigo por confidence/net_edge.
    let sampled_return = (confirmation_history.len() >= MIN_CONFIRMATION_SAMPLES).then(|| {
        let idx = rand::thread_rng().gen_range(0..confirmation_history.len());
        confirmation_history[idx]
    });

    let notional = bid.1.min(ask.1) * mid;
    let capital_needed = notional.min(40.0).max(5.0);

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::OrderFlow,
        // Bybit spot é a única exchange do Order Flow — rotulado aqui pra
        // ficar visível no dashboard sem depender de saber o código de cor.
        asset: format!("{symbol} [Bybit spot; {confirmation_note}]"),
        direction: Direction::Long,
        net_edge,
        confidence,
        valid_for_ms: 600,
        // Cotação passiva (maker) — mesma janela usada pela camada de
        // confirmação acima, não mais uma estimativa solta.
        expected_holding_secs: CONFIRMATION_WINDOW.as_secs_f64(),
        capital_needed,
        max_loss_pct: 0.003,
        leverage: 1.0,
        // Achado ao vivo (13/08/2026): era "cross_exchange" fixo pra TODO
        // símbolo — inofensivo enquanto a exposição nunca era real (não
        // bloqueava nada), mas virou um deadlock de fato assim que
        // `open_exposure`/`resolve_position` passaram a contabilizar
        // exposição de verdade: a primeira posição aberta de qualquer
        // símbolo bloqueava TODAS as outras até fechar, porque todas
        // "competiam" pelo mesmo grupo. O próprio comentário do campo em
        // types.rs já dizia a intenção certa: agrupar por MESMA APOSTA de
        // risco — dois símbolos diferentes não são a mesma aposta; o mesmo
        // símbolo via dois sinais diferentes, sim. Por símbolo, não por
        // mecanismo.
        correlation_group: symbol.to_string(),
        sampled_return,
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}
