use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::Value;
use tokio::sync::{mpsc::Sender, watch, RwLock};
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::sources::{best_level, SignalSource};
use crate::symbol_universe::{record_edge, EdgeScores};
use crate::types::{Direction, Market, Opportunity, Strategy};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
// Snapshot bruto pra disco (book comparado das duas exchanges, todos os
// simbolos) — mesma motivacao do Pump Exhaustion: dado continuo pra analise
// futura, nao so o que ja cruzou MIN_NET_EDGE.
const RAW_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

const BYBIT_WS_URL: &str = "wss://stream.bybit.com/v5/public/spot";
const BITGET_WS_URL: &str = "wss://ws.bitget.com/v2/ws/public";

// Taxa taker por perna. Contas com desconto (VIP, token de taxa) pagam
// menos — ajuste aqui se for o seu caso, senão o motor de risco vai
// subestimar o lucro líquido real. Cada captura de spread aqui é modelada
// como UMA perna vendida na exchange mais cara + UMA perna comprada na
// mais barata (você já mantém saldo pré-financiado nas duas, como
// planejado: US$100 em cada) — não há transferência entre exchanges por
// operação, então não há taxa de rede/saque neste cálculo.
//
// Bybit: 0,10% é a taxa padrão publicada de conta não-VIP (não exposta na
// API pública — precisaria de autenticação — mantido como o valor de
// referência oficial da exchange). Bitget: 0,20%, verificado ao vivo
// consultando a própria API pública (13/08/2026, pedido do usuário: "o que
// falta pra ser 100% real?") — `GET /api/v2/spot/public/symbols` retorna
// `takerFeeRate: "0.002"` pra BTCUSDT, o dobro do que este código assumia
// antes (0,10%). Corrigido — o breakeven real da arbitragem é mais alto do
// que o modelo achava.
const BYBIT_TAKER_FEE: f64 = 0.001;
const BITGET_TAKER_FEE: f64 = 0.002;
const ROUND_TRIP_FEE: f64 = BYBIT_TAKER_FEE + BITGET_TAKER_FEE;
const MIN_NET_EDGE: f64 = 0.0065;
// Não reemitir a mesma direção/símbolo com mais frequência que isso — o book
// atualiza a cada poucos milissegundos, mas o orquestrador não precisa (nem
// quer) reavaliar a mesma oportunidade centenas de vezes por segundo.
const EMIT_COOLDOWN: Duration = Duration::from_millis(700);
// Achado ao vivo (13/08/2026): sem isso, um símbolo cujo book parou de
// atualizar numa das exchanges (baixa liquidez, assinatura silenciosamente
// falha) fica com um "spread" congelado que parece uma vantagem real e
// persistente, mas não é executável — não há ninguém do outro lado do
// preço parado. Pares líquidos atualizam bem mais rápido que isso; 30s já
// é uma folga generosa antes de considerar morto.
const BOOK_STALENESS_LIMIT: Duration = Duration::from_secs(30);

// Camada de confirmação por preço (achado ao vivo, 13/08/2026, pedido do
// usuário: "eu realmente não quero nada falso... tudo completamente próximo
// ou replicado da realidade"): substitui `confidence = 0,5 + edge×25`
// (fórmula nunca validada) por uma pergunta real — o MESMO spread, na MESMA
// direção, ainda existia no book de verdade alguns segundos depois? Isso é
// literalmente o risco de latência de execução cross-exchange: entre ver o
// spread e as duas pernas realmente chegarem nas duas exchanges, o preço
// pode já ter andado. 2s é uma estimativa de latência real de round-trip
// numa conexão doméstica comum (não colocada fisicamente perto do
// servidor) — bem mais realista que assumir fill instantâneo.
const CONFIRMATION_WINDOW: Duration = Duration::from_secs(2);
const MIN_CONFIRMATION_SAMPLES: usize = 30;
const CONFIRMATION_HISTORY_CAP: usize = 500;

#[derive(Debug, Clone, Copy, Default)]
struct TopOfBook {
    bid: f64,
    bid_qty: f64,
    ask: f64,
    ask_qty: f64,
    /// Achado ao vivo (13/08/2026): KUBUSDT ficou com o MESMO bid/ask exato
    /// da Bitget por 8+ minutos seguidos (161 confirmações, 100% "acerto")
    /// enquanto a Bybit continuava atualizando normalmente — o book da
    /// Bitget parou de atualizar de verdade (símbolo sem liquidez real
    /// agora, ou falha silenciosa na assinatura), e o sistema tratava o
    /// preço congelado como se fosse um spread real e capturável. `None`
    /// = nunca recebeu nenhuma atualização ainda.
    last_update: Option<Instant>,
}

impl TopOfBook {
    fn is_valid(&self) -> bool {
        self.bid > 0.0 && self.ask > 0.0 && self.ask > self.bid
    }

    /// Book "vivo" exige atualização recente dos DOIS lados — um preço que
    /// não muda há muito tempo não é mais uma cotação real e executável,
    /// é só o último valor visto antes do feed morrer ou secar.
    fn is_fresh(&self, max_age: Duration) -> bool {
        self.last_update.map(|t| t.elapsed() <= max_age).unwrap_or(false)
    }
}

type BookMap = Arc<RwLock<HashMap<String, TopOfBook>>>;

struct PendingConfirmation {
    symbol: String,
    direction: &'static str,
    fired_at: Instant,
}

#[derive(Default)]
struct ConfirmationState {
    history: VecDeque<f64>,
    pending: Vec<PendingConfirmation>,
}

type SharedConfirmation = Arc<Mutex<ConfirmationState>>;

fn confirmation_history_path() -> String {
    format!("{}/data/confirmation_history_arbitrage.json", env!("CARGO_MANIFEST_DIR"))
}

fn load_confirmation_history() -> VecDeque<f64> {
    let path = confirmation_history_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        tracing::info!(path, "arbitragem: nenhum histórico de confirmação salvo, começando do zero");
        return VecDeque::new();
    };
    match serde_json::from_str::<Vec<f64>>(&raw) {
        Ok(v) => {
            tracing::info!(amostras = v.len(), "arbitragem: histórico de confirmação real recuperado do disco");
            v.into()
        }
        Err(e) => {
            tracing::warn!(error = %e, "arbitragem: histórico de confirmação salvo corrompido, começando do zero");
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

/// Calcula (net_edge, confidence) a partir do histórico real de confirmações
/// — recomputado contra o book de verdade `CONFIRMATION_WINDOW` depois do
/// sinal, nunca escolhido a dedo. `None` enquanto a amostra for pequena
/// demais pra significar algo (mesmo padrão do Pump Exhaustion/Order Flow).
fn empirical_edge(history: &VecDeque<f64>) -> Option<(f64, f64)> {
    if history.len() < MIN_CONFIRMATION_SAMPLES {
        return None;
    }
    let avg_return = history.iter().sum::<f64>() / history.len() as f64;
    let win_rate = history.iter().filter(|&&r| r > 0.0).count() as f64 / history.len() as f64;
    Some((avg_return.max(0.0), win_rate.clamp(0.1, 0.9)))
}

/// Fase 1 do roadmap: compara o topo do book da Bybit e da Bitget para os
/// mesmos pares e emite `Opportunity` reais quando o spread líquido (já
/// descontando taxas estimadas) supera `MIN_NET_EDGE`. Dado 100% real via
/// WebSocket público — não precisa de chave de API para isso.
pub struct ArbitrageSource {
    /// Universo dinâmico PRÓPRIO da arbitragem (spot Bybit x spot Bitget —
    /// pedido do usuário 12/08/2026: "estende essa mesma busca ampla pra
    /// arbitragem também"). Não é o mesmo canal que Order Flow/Pump
    /// Exhaustion usam — aquele vem de `category=linear` (perpétuos), que
    /// inclui símbolos sintéticos (ações/commodities tokenizados) sem par
    /// spot em nenhuma das duas exchanges.
    pub symbols_rx: watch::Receiver<Vec<String>>,
    pub bus: EventBus,
    pub edge_scores: EdgeScores,
}

impl ArbitrageSource {
    pub fn new(symbols_rx: watch::Receiver<Vec<String>>, bus: EventBus, edge_scores: EdgeScores) -> Self {
        Self { symbols_rx, bus, edge_scores }
    }
}

#[async_trait::async_trait]
impl SignalSource for ArbitrageSource {
    fn name(&self) -> &'static str {
        "arbitrage_bybit_bitget"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        // Fora do loop de propósito (mesmo raciocínio do Pump
        // Exhaustion/Order Flow): sobrevive à rotação de universo (a cada
        // 15min) e a reconexões — só ele que torna net_edge real possível.
        // Persistido em disco (auditoria externa, 13/08/2026: "refilar
        // rápido não é aceitável") — carrega no boot, salva
        // periodicamente dentro de `run_comparator`.
        let confirmation: SharedConfirmation = Arc::new(Mutex::new(ConfirmationState {
            history: load_confirmation_history(),
            pending: Vec::new(),
        }));

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
            let edge_scores2 = self.edge_scores.clone();
            let confirmation2 = confirmation.clone();
            let comparator_handle = tokio::spawn(async move {
                run_comparator(symbols, bybit_books, bitget_books, tx2, bus2, edge_scores2, confirmation2).await
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
    entry.last_update = Some(Instant::now());
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
    entry.last_update = Some(Instant::now());
}

// ---------------- comparador ----------------

#[allow(clippy::too_many_arguments)]
async fn run_comparator(
    symbols: Vec<String>,
    bybit: BookMap,
    bitget: BookMap,
    tx: Sender<Opportunity>,
    bus: EventBus,
    edge_scores: EdgeScores,
    confirmation: SharedConfirmation,
) {
    let mut last_emitted: HashMap<(String, &'static str), Instant> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(150));
    let mut last_diag = Instant::now() - Duration::from_secs(60);
    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    let mut last_raw_snapshot = Instant::now() - RAW_SNAPSHOT_INTERVAL;
    let mut last_confirmation_save = Instant::now();
    const CONFIRMATION_SAVE_INTERVAL: Duration = Duration::from_secs(30);
    let raw_log_path = format!("{}/data/raw_arbitrage.jsonl", env!("CARGO_MANIFEST_DIR"));
    let mut best_this_window: Option<(String, f64)> = None;
    loop {
        tick.tick().await;
        let raw_snapshot_now = last_raw_snapshot.elapsed() >= RAW_SNAPSHOT_INTERVAL;
        if raw_snapshot_now {
            last_raw_snapshot = Instant::now();
        }
        if last_confirmation_save.elapsed() >= CONFIRMATION_SAVE_INTERVAL {
            last_confirmation_save = Instant::now();
            let history_snapshot = confirmation.lock().unwrap().history.clone();
            save_confirmation_history(&history_snapshot);
        }
        let diag_now = last_diag.elapsed() >= Duration::from_secs(10);
        if diag_now {
            last_diag = Instant::now();
        }

        // Resolve confirmações cuja janela já passou — recomputa a MESMA
        // direção com o book de VERDADE agora, em vez de sortear.
        let due: Vec<PendingConfirmation> = {
            let mut guard = confirmation.lock().unwrap();
            let now = Instant::now();
            let (due, still_pending): (Vec<_>, Vec<_>) = guard
                .pending
                .drain(..)
                .partition(|p| now.duration_since(p.fired_at) >= CONFIRMATION_WINDOW);
            guard.pending = still_pending;
            due
        };
        if !due.is_empty() {
            let bybit_snapshot = bybit.read().await.clone();
            let bitget_snapshot = bitget.read().await.clone();
            let mut guard = confirmation.lock().unwrap();
            for p in due {
                let (Some(bb), Some(bg)) = (bybit_snapshot.get(&p.symbol), bitget_snapshot.get(&p.symbol)) else {
                    continue;
                };
                if !bb.is_valid() || !bg.is_valid() {
                    continue;
                }
                let realized_return = match p.direction {
                    "buy_bitget_sell_bybit" => (bb.bid - bg.ask) / bg.ask - ROUND_TRIP_FEE,
                    _ => (bg.bid - bb.ask) / bb.ask - ROUND_TRIP_FEE,
                };
                guard.history.push_back(realized_return);
                while guard.history.len() > CONFIRMATION_HISTORY_CAP {
                    guard.history.pop_front();
                }
            }
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
            if !bb.is_fresh(BOOK_STALENESS_LIMIT) || !bg.is_fresh(BOOK_STALENESS_LIMIT) {
                // Achado ao vivo (13/08/2026): KUBUSDT ficou com o preço da
                // Bitget congelado por 8+ minutos (161 "confirmações",
                // 100% de acerto, sempre o mesmo edge exato) enquanto a
                // Bybit continuava atualizando — o feed tinha morrido, não
                // era uma vantagem real e persistente. Um preço que não
                // atualiza há mais de BOOK_STALENESS_LIMIT não é mais uma
                // cotação executável, é o último valor visto antes do
                // book secar ou a assinatura falhar silenciosamente.
                if diag_now {
                    tracing::debug!(symbol, "book desatualizado — pulando (uma das exchanges parou de atualizar)");
                }
                continue;
            }

            // Direção 1: comprar na Bitget (ask), vender na Bybit (bid).
            let edge_1 = (bb.bid - bg.ask) / bg.ask - ROUND_TRIP_FEE;
            // Direção 2: comprar na Bybit (ask), vender na Bitget (bid).
            let edge_2 = (bg.bid - bb.ask) / bb.ask - ROUND_TRIP_FEE;

            // Alimenta o universo dinâmico PRÓPRIO da arbitragem (spot x
            // spot) com o edge medido de verdade a cada tick com book válido
            // nas duas exchanges — igual ao que Order Flow já faz pro
            // universo linear. Registra a melhor das duas direções: é o que
            // decide se vale a pena manter este par ativo.
            record_edge(&edge_scores, symbol, edge_1.max(edge_2));

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
                maybe_emit(&tx, &mut last_emitted, symbol, "buy_bitget_sell_bybit", edge_1, bg.ask_qty * bg.ask, &confirmation).await;
            }
            if edge_2 > MIN_NET_EDGE {
                maybe_emit(&tx, &mut last_emitted, symbol, "buy_bybit_sell_bitget", edge_2, bb.ask_qty * bb.ask, &confirmation).await;
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

fn direction_label(direction_key: &str) -> &'static str {
    match direction_key {
        "buy_bitget_sell_bybit" => "compra Bitget → vende Bybit",
        _ => "compra Bybit → vende Bitget",
    }
}

async fn maybe_emit(
    tx: &Sender<Opportunity>,
    last_emitted: &mut HashMap<(String, &'static str), Instant>,
    symbol: &str,
    direction_key: &'static str,
    instant_edge: f64,
    notional_available: f64,
    confirmation: &SharedConfirmation,
) {
    let key = (symbol.to_string(), direction_key);
    if let Some(t) = last_emitted.get(&key) {
        if t.elapsed() < EMIT_COOLDOWN {
            return;
        }
    }
    last_emitted.insert(key, Instant::now());

    // Registra a hipótese pra ser conferida contra o book real daqui a
    // CONFIRMATION_WINDOW — acontece sempre que o gate de fee é cruzado,
    // independente de já ter amostra suficiente pra confiar.
    //
    // Deduplicação por cluster de evento (auditoria externa, 13/08/2026,
    // mesmo raciocínio do Order Flow): EMIT_COOLDOWN (700ms) é menor que
    // CONFIRMATION_WINDOW (2s) — sem isso, o mesmo spread sobrevivendo por
    // alguns segundos gerava múltiplas confirmações sobrepostas pro mesmo
    // (símbolo, direção), autocorrelacionadas mas contadas como amostras
    // independentes. Só uma confirmação em voo por (símbolo, direção) por
    // vez.
    let (net_edge, confidence, confirmation_note, sampled_return) = {
        let mut guard = confirmation.lock().unwrap();
        let already_pending = guard.pending.iter().any(|p| p.symbol == symbol && p.direction == direction_key);
        if !already_pending {
            guard.pending.push(PendingConfirmation {
                symbol: symbol.to_string(),
                direction: direction_key,
                fired_at: Instant::now(),
            });
        }
        // Reamostragem real (auditoria externa, 13/08/2026): sorteia um
        // desfecho que REALMENTE aconteceu no histórico de confirmação,
        // em vez de deixar o orquestrador decidir por sorteio ponderado
        // pela média. `None` enquanto a amostra ainda for pequena demais.
        let sampled = (guard.history.len() >= MIN_CONFIRMATION_SAMPLES).then(|| {
            let idx = rand::thread_rng().gen_range(0..guard.history.len());
            guard.history[idx]
        });
        match empirical_edge(&guard.history) {
            Some((edge, conf)) => {
                let wins = guard.history.iter().filter(|&&r| r > 0.0).count();
                (edge, conf, format!(
                    "edge instantâneo {:+.3}%, confirmação real: {wins}/{} acertos, retorno médio {:+.3}%",
                    instant_edge * 100.0, guard.history.len(), edge * 100.0
                ), sampled)
            }
            None => (0.0, 0.3, format!(
                "edge instantâneo {:+.3}%, aguardando confirmação ({}/{MIN_CONFIRMATION_SAMPLES} amostras)",
                instant_edge * 100.0, guard.history.len()
            ), sampled),
        }
    };

    // Nunca reivindicar mais capital do que o topo do book realmente
    // suporta, e nunca acima de um teto de sanidade — o motor de risco ainda
    // vai clampar isso ao tamanho de perna configurado.
    let capital_needed = notional_available.min(50.0).max(5.0);
    // Risco de execução de arbitragem (uma perna preenche, a outra não a
    // tempo, ou o preço se move entre as duas pontas) — agora medido pela
    // camada de confirmação acima, não mais uma estimativa fixa.
    let max_loss_pct = 0.004;

    // Rótulo de exchange/direção exposto no próprio `asset` (achado do
    // usuário, 13/08/2026: "onde que é essas operações bybit ou bitget?
    // tem como diferenciar isso em algum lugar?" — não tinha. Agora tem.
    let asset = format!("{symbol} [{}; {confirmation_note}]", direction_label(direction_key));

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::Arbitrage,
        asset,
        direction: Direction::Long,
        net_edge,
        confidence,
        valid_for_ms: 900,
        // Duas pernas em duas exchanges — mesma janela usada pela camada de
        // confirmação acima, não mais uma estimativa solta.
        expected_holding_secs: CONFIRMATION_WINDOW.as_secs_f64(),
        capital_needed,
        max_loss_pct,
        leverage: 1.0,
        // Achado ao vivo (13/08/2026): era "cross_exchange" fixo pra TODO
        // símbolo — inofensivo enquanto a exposição nunca era real, mas
        // virou deadlock de fato assim que a exposição passou a ser
        // contabilizada de verdade (ver order_flow.rs, mesmo achado). Por
        // símbolo, não por mecanismo — dois símbolos diferentes não são a
        // mesma aposta de risco.
        correlation_group: symbol.to_string(),
        sampled_return,
        emitted_at: Instant::now(),
    };

    let _ = tx.send(opp).await;
}
