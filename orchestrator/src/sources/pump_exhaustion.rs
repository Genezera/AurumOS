use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::{DashboardEvent, EventBus};
use crate::fusion::{
    recent_deposit_pressure_usd, recent_long_liquidation_cascade, LiquidationBoard, WhaleBoard,
};
use crate::sources::SignalSource;
use crate::types::{next_signal_id, Direction, ExecutionMode, Market, Opportunity, Strategy};

const BYBIT_LINEAR_WS_URL: &str = "wss://stream.bybit.com/v5/public/linear";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
// Snapshot bruto pra disco (todos os simbolos, nao so o melhor candidato do
// dashboard) — insumo pra recalibrar FUNDING_EXTREME/PUMP_24H_PCNT com dado
// proprio no futuro, sem depender de rebuscar historico externo de novo.
const RAW_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

// Recalibrado em 13/08/2026 a partir de dado real: os 16.994 snapshots
// acumulados em raw_pump_exhaustion.jsonl (~17h, 30 símbolos) mostraram que
// os limiares antigos (0,10%/8h funding, +15% pump 24h, +20% OI 1h) estavam
// calibrados pra um regime bem mais volátil do que o observado — zero
// sinais em 17h de operação porque pump nunca passou de 7,5% nem OI de
// 5,2% no período. Novos valores = percentil 90 real de cada métrica na
// própria distribuição observada (não chute): p90 conjunto (>=2 de 3 acima
// do limiar) ocorre em 0,35% dos snapshots, ~59 vezes no período — raro o
// suficiente pra ainda significar algo, frequente o suficiente pra a
// camada de confirmação de preço (MIN_CONFIRMATION_SAMPLES=20) conseguir
// acumular amostra em tempo razoável. Recalibrar de novo à medida que mais
// dado (e mais regimes de mercado) acumular — isto não é definitivo.
//
// Funding "normal" gira em torno de 0,01%/8h. Acima do limiar os comprados
// (longs) estão pagando um prêmio caro pra manter a posição — sinal
// clássico de mercado esticado. Combinado com alta grande em 24h, é um dos
// padrões de exaustão descritos no roadmap (não uma certeza).
const FUNDING_EXTREME: f64 = 0.00015; // 0,015%/8h (p90 observado: 0,00017)
const PUMP_24H_PCNT: f64 = 0.03; // +3% em 24h (p90 observado: 2,84%)
                                 // Terceira dimensão do scoring (antes só funding+pump) — crescimento de
                                 // open interest é o "dinheiro novo alavancado entrando" que o roadmap pede
                                 // ("crescimento de open interest" na lista de métricas da Fase 6).
const OI_GROWTH_1H_EXTREME: f64 = 0.01; // +1% em 1h (p90 observado, só amostras válidas: 0,95%)
                                        // Janela mínima de histórico de OI antes de confiar no cálculo de
                                        // crescimento — evita "crescimento" espúrio nos primeiros minutos após o
                                        // boot, quando só temos 1-2 amostras.
const OI_HISTORY_MIN_WINDOW: Duration = Duration::from_secs(50 * 60);
const OI_HISTORY_MAX_WINDOW: Duration = Duration::from_secs(70 * 60);
// Ampliado de 45s para 30min (13/08/2026, revisão técnica externa): 45s
// era curto demais pra separar eventos de verdade — um pump sustentado por
// minutos disparava a mesma janela repetidamente. Combinado com a checagem
// de "já pendente" acima, um símbolo só contribui uma nova amostra pra
// confirmation_history depois de: 20min de janela de confirmação + 30min
// de descanso = pelo menos ~50min entre amostras do mesmo símbolo.
const EMIT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
// Janela em que uma cascata de liquidação de longs no MESMO símbolo ainda
// conta como reforço de fusão — mais curta que o cooldown acima, porque uma
// cascata é um evento pontual, não um estado sustentado.
const LIQUIDATION_FUSION_WINDOW: Duration = Duration::from_secs(15 * 60);
// Depósito agregado em exchanges conhecidas (janela de 30min do próprio
// Whale Watch) acima disso conta como pressão vendedora relevante o
// suficiente pra reforçar um candidato de exaustão.
const WHALE_FUSION_THRESHOLD_USD: f64 = 2_000_000.0;

// Recalibrado em 12/08/2026 a partir de 6.000 amostras reais recentes
// (universo dinâmico de 87 símbolos, raw_pump_exhaustion.jsonl): o gatilho
// antigo ("2 de 3 sinais booleanos", cada um calibrado por PERCENTIL
// MARGINAL isolado) partia da premissa implícita de que os 3 sinais
// coocorrem no mesmo símbolo — não coocorrem. Em 3.000 amostras: funding
// cruzou o limiar 249 vezes, pump 175 vezes, OI 4 vezes, mas NUNCA dois ao
// mesmo tempo no mesmo símbolo (0 ocorrências de signals_true>=2 em 24h de
// operação real). Resultado: zero sinais emitidos, mesmo com dado passando
// perto do limiar o tempo todo — o gate estava estruturalmente inatingível,
// não só raro.
//
// exhaustion_score() já calcula a combinação ponderada contínua das 3
// dimensões (usada até agora só pro heartbeat do dashboard); a distribuição
// REAL desse score nas mesmas 6.000 amostras tem espalhamento genuíno
// (p50=0,27 p75=0,43 p90=0,58 p95=0,72 p99=0,91) — calibrar o gatilho
// diretamente no percentil real do score (em vez de tentar re-calibrar 3
// limiares marginais pra um co-ocorrência que na prática não acontece) é o
// que reflete o dado observado sem inflar a seletividade de forma artificial.
const EXHAUSTION_SCORE_TRIGGER: f64 = 0.60; // ~p90 real observado — sinal forte, dispara sozinho
const EXHAUSTION_SCORE_WEAK: f64 = 0.40; // ~p75 — só dispara combinado com reforço de fusão

// --- Camada de confirmação por preço (roadmap Seção 0: "toda oportunidade
// de ouro é tratada como hipótese, não fato... precisa de confirmação de
// preço/livro/volume antes de virar ordem") ---
//
// Em vez de chutar um net_edge, o próprio módulo mede: quando um candidato
// dispara, registra o preço de entrada; passados CONFIRMATION_WINDOW,
// confere se o preço realmente caiu (a hipótese de exaustão) usando o
// mesmo feed de ticker já em uso. Isso alimenta um histórico contínuo — só
// depois de MIN_CONFIRMATION_SAMPLES desfechos reais o módulo passa a
// emitir net_edge > 0, e mesmo assim calculado do próprio histórico
// (retorno médio de uma posição short = -variação de preço observada),
// nunca um número escolhido a dedo. Antes disso, ou se o histórico mostrar
// que o padrão não tem vantagem real, net_edge continua 0.0 — igual a
// antes.
const CONFIRMATION_WINDOW: Duration = Duration::from_secs(20 * 60);
const MIN_CONFIRMATION_SAMPLES: usize = 20;
const CONFIRMATION_HISTORY_CAP: usize = 500;
const CONFIRMATION_CHECK_INTERVAL: Duration = Duration::from_secs(60);

// Achado ao vivo (13/08/2026, pedido do usuário: "confirme que está tudo
// rodando realmente real, nada de coisa falsa"): `empirical_edge` calculava
// o retorno médio direto da variação de preço observada, sem descontar
// NENHUMA taxa de execução — diferente de Order Flow (MAKER_FEE) e
// Arbitragem (ROUND_TRIP_FEE), que já descontam. Isso inflava o net_edge
// medido pelo custo real de abrir/fechar uma posição short em perpétuo.
// Taxa taker publicada padrão (não-VIP) da Bybit USDT-perpétuo é 0,055%
// por lado — não exposta na API pública de mercado (precisaria de
// autenticação, como o maker da Bybit spot), então uso o valor
// conservador/publicado, mesmo racional já aplicado ao MAKER_FEE de
// order_flow.rs. Entrada e saída são ambas taker (reação rápida a um
// sinal de exaustão já detectado, não uma cotação passiva).
const ROUND_TRIP_TAKER_FEE: f64 = 2.0 * 0.00055;

#[derive(Debug, Clone, Default, Copy)]
struct TickerState {
    funding_rate: f64,
    price_24h_pcnt: f64,
    last_price: f64,
    open_interest_value: f64,
    oi_growth_1h_pct: f64,
}

struct PendingConfirmation {
    symbol: String,
    entry_price: f64,
    fired_at: Instant,
}

/// Parte do roadmap de Pump Exhaustion (Fase 6): observa funding rate,
/// variação de 24h e crescimento de open interest de perpétuos reais na
/// Bybit — 3 dimensões combinadas de forma ponderada (`exhaustion_score`),
/// não uma métrica isolada, pra sinalizar candidatos a mercado esticado.
/// Desde esta revisão, primeiro módulo de evento a ter a camada de
/// confirmação por preço implementada (ver `CONFIRMATION_WINDOW` acima) —
/// `net_edge` deixa de ser sempre 0.0 uma vez que há amostra real
/// suficiente, calculado do próprio histórico de confirmações, nunca
/// estimado de cabeça. Ainda não é o detector completo descrito no PDF
/// (falta desaceleração de compra agressiva, fluxo pra exchange).
pub struct PumpExhaustionSource {
    pub symbols_rx: watch::Receiver<Vec<String>>,
    pub bus: EventBus,
    pub liquidation_board: LiquidationBoard,
    pub whale_board: WhaleBoard,
}

impl PumpExhaustionSource {
    pub fn new(
        symbols_rx: watch::Receiver<Vec<String>>,
        bus: EventBus,
        liquidation_board: LiquidationBoard,
        whale_board: WhaleBoard,
    ) -> Self {
        Self {
            symbols_rx,
            bus,
            liquidation_board,
            whale_board,
        }
    }
}

#[async_trait::async_trait]
impl SignalSource for PumpExhaustionSource {
    fn name(&self) -> &'static str {
        "pump_exhaustion_bybit_linear"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        // Vivem fora de run_once de propósito: uma queda de conexão (comum,
        // já tratada com reconexão automática) não pode apagar semanas de
        // histórico de confirmação acumulado — só ele que torna net_edge
        // real possível. Sobrevivem também à troca de universo de símbolos
        // (agora a cada 15min — 12/08/2026, pedido do usuário por rotação
        // mais rápida), pelo mesmo motivo: sem isso, oi_history nunca
        // acumularia os 50min mínimos exigidos pra confiar no crescimento de
        // OI, cegando permanentemente essa dimensão do score.
        let mut confirmation_history: VecDeque<f64> = VecDeque::new();
        let mut oi_history: HashMap<String, VecDeque<(Instant, f64)>> = HashMap::new();
        // Precisa sobreviver à rotação do universo. A rotação ocorre a cada
        // 15 minutos e a confirmação demora 20; alocar isto em `run_once`
        // apagava toda amostra antes que pudesse maturar.
        let mut pending_confirmations: Vec<PendingConfirmation> = Vec::new();
        loop {
            let symbols = self.symbols_rx.borrow().clone();
            tokio::select! {
                result = run_once(&symbols, &tx, &self.bus, &mut confirmation_history, &mut pending_confirmations, &mut oi_history, &self.liquidation_board, &self.whale_board) => {
                    if let Err(e) = result {
                        tracing::warn!(error = %e, "conexão de pump exhaustion (Bybit linear) caiu, reconectando em 3s");
                    }
                    sleep(Duration::from_secs(3)).await;
                }
                _ = self.symbols_rx.changed() => {
                    tracing::info!("pump exhaustion: universo de símbolos atualizado — reconectando com lista nova");
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
    confirmation_history: &mut VecDeque<f64>,
    pending_confirmations: &mut Vec<PendingConfirmation>,
    oi_history: &mut HashMap<String, VecDeque<(Instant, f64)>>,
    liquidation_board: &LiquidationBoard,
    whale_board: &WhaleBoard,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(BYBIT_LINEAR_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    // Chunked em lotes de 10 tópicos por mensagem, como os outros feeds da Bybit — antes mandava tudo
    // numa mensagem só, o que funcionava com ~70 símbolos mas arrisca
    // rejeição da Bybit com o universo ampliado (task de 12/08/2026: "pool
    // extremamente maior pesquisando todo o mercado").
    const CHUNK_SIZE: usize = 10;
    let args: Vec<String> = symbols.iter().map(|s| format!("tickers.{s}")).collect();
    for chunk in args.chunks(CHUNK_SIZE) {
        sink.send(WsMessage::Text(
            serde_json::json!({ "op": "subscribe", "args": chunk }).to_string(),
        ))
        .await?;
    }
    tracing::info!(
        exchange = "bybit_linear",
        symbols = symbols.len(),
        lotes = args.len().div_ceil(CHUNK_SIZE),
        "pump exhaustion: assinatura de tickers enviada"
    );

    let mut state: HashMap<String, TickerState> = HashMap::new();
    let mut last_emitted: HashMap<String, Instant> = HashMap::new();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut raw_snapshot = tokio::time::interval(RAW_SNAPSHOT_INTERVAL);
    let raw_log_path = format!(
        "{}/data/raw_pump_exhaustion.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut confirmation_check = tokio::time::interval(CONFIRMATION_CHECK_INTERVAL);
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
                // "best_edge_pct" aqui não é edge de preço — é a média das 3
                // dimensões do score (funding, pump 24h, crescimento de OI),
                // cada uma normalizada contra seu próprio limiar. Reflete o
                // score ponderado usado no gatilho abaixo, não só 1 métrica.
                let best = state.iter().map(|(symbol, s)| {
                    (symbol.clone(), exhaustion_score(s) * 100.0)
                }).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                if let Some((symbol, proximity)) = best {
                    bus.emit(DashboardEvent::scan_heartbeat(Strategy::PumpExhaustion, symbols.len() as u32, &symbol, proximity));
                }
            }
            _ = raw_snapshot.tick() => {
                let ts_ms = crate::raw_log::now_ms();
                let now = Instant::now();
                for (symbol, s) in state.iter_mut() {
                    let hist = oi_history.entry(symbol.clone()).or_default();
                    hist.push_back((now, s.open_interest_value));
                    while hist.front().map(|(t, _)| now.duration_since(*t) > OI_HISTORY_MAX_WINDOW).unwrap_or(false) {
                        hist.pop_front();
                    }
                    // Só confia no crescimento quando já temos ~1h de historico —
                    // com poucas amostras, uma flutuação normal pareceria um pico.
                    if let Some((_, oldest_oi)) = hist.iter().find(|(t, _)| now.duration_since(*t) >= OI_HISTORY_MIN_WINDOW) {
                        if *oldest_oi > 0.0 {
                            s.oi_growth_1h_pct = (s.open_interest_value - oldest_oi) / oldest_oi;
                        }
                    }

                    crate::raw_log::append(&raw_log_path, serde_json::json!({
                        "ts_ms": ts_ms,
                        "symbol": symbol,
                        "funding_rate": s.funding_rate,
                        "price_24h_pcnt": s.price_24h_pcnt,
                        "last_price": s.last_price,
                        "open_interest_value": s.open_interest_value,
                        "oi_growth_1h_pct": s.oi_growth_1h_pct,
                    }));
                }
            }
            _ = confirmation_check.tick() => {
                // Passou a janela de confirmação pra esses candidatos —
                // confere se o preço caiu (short teria lucrado) usando o
                // mesmo estado de ticker já em memória, sem chamada extra.
                let now = Instant::now();
                let mut still_pending = Vec::with_capacity(pending_confirmations.len());
                for p in pending_confirmations.drain(..) {
                    if now.duration_since(p.fired_at) < CONFIRMATION_WINDOW {
                        still_pending.push(p);
                        continue;
                    }
                    if let Some(s) = state.get(&p.symbol) {
                        if p.entry_price > 0.0 && s.last_price > 0.0 {
                            let pct_move = (s.last_price - p.entry_price) / p.entry_price;
                            confirmation_history.push_back(pct_move);
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
                    Some(Ok(WsMessage::Text(text))) => {
                        handle_message(&text, &mut state, &mut last_emitted, tx, pending_confirmations, confirmation_history, liquidation_board, whale_board).await;
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

/// Score de exaustão ponderado — média das 3 dimensões (funding, pump 24h,
/// crescimento de OI), cada uma normalizada contra seu próprio limiar
/// "extremo" e clampada antes de mediar, pra nenhuma métrica isolada dominar
/// o score sozinha. É a "combinação ponderada" que a Fase 6 do roadmap pede
/// no lugar de uma única métrica isolada.
fn exhaustion_score(s: &TickerState) -> f64 {
    // Exaustão de alta/entrada short requer longs pagando funding positivo.
    // Funding negativo extremo descreve shorts lotados e não pode reforçar
    // a mesma direção por meio de `abs()`.
    let funding_component = (s.funding_rate.max(0.0) / FUNDING_EXTREME).min(1.5);
    let pump_component = (s.price_24h_pcnt.max(0.0) / PUMP_24H_PCNT).min(1.5);
    let oi_component = (s.oi_growth_1h_pct.max(0.0) / OI_GROWTH_1H_EXTREME).min(1.5);
    (funding_component + pump_component + oi_component) / 3.0
}

/// Calcula (net_edge, confidence) a partir do histórico real de
/// confirmações — retorno médio de uma posição short (ganha quando o preço
/// cai) e taxa de acerto, ambos medidos, nunca escolhidos a dedo. `None`
/// enquanto a amostra for pequena demais pra significar algo.
fn empirical_edge(history: &VecDeque<f64>) -> Option<(f64, f64)> {
    if history.len() < MIN_CONFIRMATION_SAMPLES {
        return None;
    }
    let avg_return = history
        .iter()
        .map(|m| -m - ROUND_TRIP_TAKER_FEE)
        .sum::<f64>()
        / history.len() as f64;
    // Acerto continua definido pelo movimento bruto de preço (a taxa não
    // muda a DIREÇÃO do resultado, só o tamanho) — não usar `net_of_fee <
    // 0.0` aqui faria um trade que teria empatado sem taxa contar como
    // derrota só por causa da taxa, o que é verdade pro PnL mas não é
    // "acerto direcional", que é o que este número representa.
    let win_rate = history.iter().filter(|&&m| m < 0.0).count() as f64 / history.len() as f64;
    // Nunca emite edge negativo — se o histórico mostra que o padrão não
    // funciona, o módulo simplesmente continua informativo (net_edge=0),
    // igual a antes. Confidence sempre fica num intervalo sao mesmo com
    // amostra ainda no limiar minimo (evita excesso de confiança por ruido).
    Some((avg_return.max(0.0), win_rate.clamp(0.1, 0.9)))
}

#[allow(clippy::too_many_arguments)]
async fn handle_message(
    text: &str,
    state: &mut HashMap<String, TickerState>,
    last_emitted: &mut HashMap<String, Instant>,
    tx: &Sender<Opportunity>,
    pending_confirmations: &mut Vec<PendingConfirmation>,
    confirmation_history: &VecDeque<f64>,
    liquidation_board: &LiquidationBoard,
    whale_board: &WhaleBoard,
) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(topic) = v.get("topic").and_then(Value::as_str) else {
        return;
    };
    if !topic.starts_with("tickers.") {
        return;
    }
    let Some(data) = v.get("data") else { return };
    let Some(symbol) = data.get("symbol").and_then(Value::as_str) else {
        return;
    };

    // "snapshot" vem completo; "delta" só traz os campos que mudaram — por
    // isso fazemos merge em cima do estado anterior em vez de substituir.
    let entry = state.entry(symbol.to_string()).or_default();
    if let Some(v) = data
        .get("fundingRate")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
    {
        entry.funding_rate = v;
    }
    if let Some(v) = data
        .get("price24hPcnt")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
    {
        entry.price_24h_pcnt = v;
    }
    if let Some(v) = data
        .get("lastPrice")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
    {
        entry.last_price = v;
    }
    if let Some(v) = data
        .get("openInterestValue")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
    {
        entry.open_interest_value = v;
    }
    let snapshot = *entry;

    let should_diag = last_emitted
        .get(&format!("__diag_{symbol}"))
        .map(|t| t.elapsed() >= Duration::from_secs(15))
        .unwrap_or(true);
    if should_diag {
        last_emitted.insert(format!("__diag_{symbol}"), Instant::now());
        tracing::debug!(
            symbol,
            funding_rate_pct = snapshot.funding_rate * 100.0,
            change_24h_pct = snapshot.price_24h_pcnt * 100.0,
            last_price = snapshot.last_price,
            oi_value_usd = snapshot.open_interest_value,
            "snapshot de ticker (pump exhaustion)"
        );
    }

    // Gatilho por score contínuo (revisão 12/08/2026 — ver comentário em
    // EXHAUSTION_SCORE_TRIGGER acima): o "2 de 3 booleanos" antigo nunca
    // coocorria de verdade no dado real, travando o módulo em zero sinais
    // por 24h+ mesmo com 87 símbolos monitorados. O score pondera as 3
    // dimensões continuamente — usar o próprio percentil real dele como
    // limiar dispara quando o CONJUNTO está esticado, não quando 2 métricas
    // isoladas cruzam limiares que raramente se encontram no mesmo símbolo.
    let score = exhaustion_score(&snapshot);

    // Fusão (pedido do usuário, 13/08/2026 — "faça intercomunicação"):
    // Whale Watch e Liquidation Hunter já coletam dado real do mesmo mercado
    // mas ficavam isolados, sempre em net_edge=0. Exemplo dado pelo usuário:
    // "pump + depósitos de whales em exchange + liquidações compradoras
    // crescendo = candidato forte de exaustão". Uma cascata de liquidação de
    // LONGS neste símbolo específico, ou pressão agregada de depósito em
    // exchanges conhecidas (mercado geral), contam como reforço — com o
    // score já moderadamente esticado (EXHAUSTION_SCORE_WEAK) MAIS pelo
    // menos 1 reforço externo, dispensa o score forte sozinho.
    let fusion_liq =
        recent_long_liquidation_cascade(liquidation_board, symbol, LIQUIDATION_FUSION_WINDOW);
    let fusion_whale_usd = recent_deposit_pressure_usd(whale_board);
    let fusion_whale = fusion_whale_usd > WHALE_FUSION_THRESHOLD_USD;
    let fusion_count = [fusion_liq, fusion_whale].iter().filter(|&&b| b).count();

    let strong_signal = score >= EXHAUSTION_SCORE_TRIGGER;
    let weak_signal_with_fusion = score >= EXHAUSTION_SCORE_WEAK && fusion_count >= 1;
    if !(strong_signal || weak_signal_with_fusion) {
        return;
    }

    if let Some(t) = last_emitted.get(symbol) {
        if t.elapsed() < EMIT_COOLDOWN {
            return;
        }
    }
    // Achado da revisão técnica externa (13/08/2026): um único pump
    // sustentado por minutos gera "sinal true" em várias janelas de scan
    // seguidas — sem esta checagem, cada uma virava uma amostra nova na
    // confirmation_history, inflando a contagem com repetições do MESMO
    // evento em vez de eventos independentes. Se este símbolo já tem uma
    // confirmação pendente (ainda dentro da janela de 20min), não registra
    // outra — só a maturação da pendente conta como uma amostra.
    let already_pending = pending_confirmations.iter().any(|p| p.symbol == symbol);
    last_emitted.insert(symbol.to_string(), Instant::now());
    if already_pending {
        return;
    }

    // Registra o preço de entrada agora — a confirmação (o preço realmente
    // caiu?) só é conferida daqui a CONFIRMATION_WINDOW, no tick separado.
    pending_confirmations.push(PendingConfirmation {
        symbol: symbol.to_string(),
        entry_price: snapshot.last_price,
        fired_at: Instant::now(),
    });

    let (net_edge, base_confidence, confirmation_note) = match empirical_edge(confirmation_history)
    {
        Some((edge, conf)) => {
            let wins = confirmation_history.iter().filter(|&&m| m < 0.0).count();
            (
                edge,
                conf,
                format!(
                    "confirmação real: {wins}/{} acertos, edge médio {:+.2}%",
                    confirmation_history.len(),
                    edge * 100.0
                ),
            )
        }
        None => (
            0.0,
            0.3,
            format!(
                "aguardando confirmação ({}/{MIN_CONFIRMATION_SAMPLES} amostras)",
                confirmation_history.len()
            ),
        ),
    };

    // Reforço de fusão na confiança (nunca no net_edge — esse continua vindo
    // só da confirmação de preço medida, não de um sinal de outro módulo).
    // +15% por reforço confirmado, até +30% com os dois — nunca acima de
    // 0,95 pra não fingir certeza absoluta.
    let confidence = (base_confidence * (1.0 + 0.15 * fusion_count as f64)).min(0.95);
    let fusion_note = match (fusion_liq, fusion_whale) {
        (true, true) => " [fusão: cascata de longs + depósito em exchange]".to_string(),
        (true, false) => " [fusão: cascata de liquidação de longs]".to_string(),
        (false, true) => format!(
            " [fusão: depósito em exchange ${:.0}k/30min]",
            fusion_whale_usd / 1000.0
        ),
        (false, false) => String::new(),
    };
    let confirmation_note = format!("{confirmation_note}{fusion_note}");

    tracing::debug!(
        symbol,
        score,
        funding_rate_pct = snapshot.funding_rate * 100.0,
        change_24h_pct = snapshot.price_24h_pcnt * 100.0,
        oi_growth_1h_pct = snapshot.oi_growth_1h_pct * 100.0,
        last_price = snapshot.last_price,
        net_edge,
        confirmation_note,
        "candidato a exaustão de pump detectado"
    );

    let opp = Opportunity {
        signal_id: next_signal_id(),
        market: Market::Crypto,
        strategy: Strategy::PumpExhaustion,
        asset: format!(
            "{symbol} (funding {:.3}%, 24h +{:.1}%, OI 1h {:+.1}%) [{confirmation_note}]",
            snapshot.funding_rate * 100.0,
            snapshot.price_24h_pcnt * 100.0,
            snapshot.oi_growth_1h_pct * 100.0,
        ),
        direction: Direction::Short,
        net_edge,
        confidence,
        valid_for_ms: 60_000,
        // Mesma janela usada pela camada de confirmação — é literalmente
        // quanto tempo o módulo espera pra saber se o sinal deu certo.
        expected_holding_secs: CONFIRMATION_WINDOW.as_secs_f64(),
        capital_needed: 10.0,
        reference_price: Some(snapshot.last_price),
        // Continua fixo por enquanto — calibrar isso também a partir do
        // histórico (ex.: pior variação adversa observada) é o próximo
        // passo natural, não feito nesta revisão pra não inflar o escopo
        // de uma vez só.
        max_loss_pct: 0.02,
        leverage: 1.0,
        correlation_group: "altcoins".to_string(),
        // A confirmação histórica estima a hipótese, mas não é o desfecho
        // desta oportunidade. Fica observacional até um broker shadow
        // abrir e fechar esta posição específica com quotes rastreáveis.
        execution_mode: ExecutionMode::ObservationOnly,
        capital_multiplier: 1.0,
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_funding_does_not_support_a_short_exhaustion_signal() {
        let state = TickerState {
            funding_rate: -FUNDING_EXTREME * 10.0,
            ..TickerState::default()
        };
        assert_eq!(exhaustion_score(&state), 0.0);
    }
}
