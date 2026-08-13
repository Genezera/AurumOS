use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::events::{DashboardEvent, EventBus, SymbolRanking};

/// Pedido do usuário (13/08/2026): "não quero que fique travado no mesmo,
/// quero uma análise inteira do mercado inteiro buscando por oportunidades
/// como essa que encontramos [QTUMUSDT], talvez essa pode ficar ruim daqui
/// um tempo aí já troca para outra que está bem melhor". Substitui o
/// `WIDE_SYMBOLS` estático por uma lista que se recalcula sozinha: mede o
/// edge real de cada símbolo (mesmo cálculo que o Order Flow já faz a cada
/// atualização de book, agora acumulado numa janela móvel em memória),
/// mantém os que comprovaram edge positivo recente, e usa o resto das
/// vagas pra explorar candidatos novos vindos direto da lista completa de
/// perpétuos USDT da Bybit — não uma lista fixa escolhida à mão.

const EDGE_HISTORY_CAP: usize = 500;
// Pedido do usuário (12/08/2026): "quero que continue rápido, dinâmico,
// acelerado... pesquise o mercado inteiro globalmente, uma pool
// extremamente maior... quero que aprenda... encontrando o melhor mercado".
// Rotação de 2h→15min: reavalia quem está realmente com edge medido MUITO
// mais rápido — um símbolo que esfriar sai da lista de "comprovados" em
// minutos, não horas, e libera vaga pra exploração sem esperar o ciclo
// antigo de 2h. Universo alvo 80→220 e vagas de exploração 15→50: cobre uma
// fatia bem maior do mercado real (Bybit linear tem ~500 pares USDT) em vez
// de só 80, e explora candidatos novos numa taxa proporcionalmente maior a
// cada rotação — sem inventar edge, só olhando mais mercado, mais rápido.
const ROTATION_INTERVAL: Duration = Duration::from_secs(15 * 60);
const TOTAL_TARGET: usize = 220;
const MIN_EXPLORATION_SLOTS: usize = 50;
const MIN_SAMPLES_TO_TRUST: usize = 200;
const BYBIT_TICKERS_URL_BASE: &str = "https://api.bybit.com/v5/market/tickers?category=";
// Top-N por volume 24h vindo da Bybit — teto do pool de candidatos, não do
// que fica ativo (isso é TOTAL_TARGET). Ampliado de 250 pra cobrir
// praticamente todo o mercado linear USDT líquido da exchange.
const CANDIDATE_POOL_CAP: usize = 450;

/// Uma instância de `run()` é um universo dinâmico completo (candidatos +
/// rotação + persistência) pra UM mercado específico. Pedido do usuário
/// (12/08/2026 — "estende essa mesma busca ampla pra arbitragem também"):
/// arbitragem compara SPOT-vs-SPOT (Bybit x Bitget), não perpétuos — usar a
/// mesma lista dinâmica que Order Flow/Pump Exhaustion/Liquidation Hunter
/// usam (baseada em `category=linear`) faria arbitragem "explorar" símbolos
/// que nem existem como par spot nas duas exchanges. Cada mercado roda sua
/// própria instância de `run()`, com seu próprio `EdgeScores` (edge medido
/// de verdade É diferente entre maker-spread de livro único e spread
/// cross-exchange) e seu próprio arquivo de persistência.
#[derive(Debug, Clone, Copy)]
pub struct UniverseKind {
    pub bybit_category: &'static str,
    pub persist_filename: &'static str,
    /// Pedido do usuário (12/08/2026): "eu não quero que perde nada quando
    /// reinicia o sistema" — antes, `EdgeScores` (o histórico de edge
    /// medido por símbolo, a única coisa que faz a exploração e o
    /// escalonamento por Kelly significarem algo) só existia em memória e
    /// era recriado vazio a cada boot. Com quantos restarts uma sessão de
    /// testes acumula, isso na prática apagava o aprendizado inteiro toda
    /// hora. Persistido e recarregado aqui, separado de
    /// `persist_filename` (que é só o ranking pra exibição).
    pub edge_scores_filename: &'static str,
    pub dashboard_kind: &'static str,
}

pub const LINEAR: UniverseKind = UniverseKind {
    bybit_category: "linear",
    persist_filename: "active_symbols.json",
    edge_scores_filename: "edge_scores_linear.json",
    dashboard_kind: "linear",
};

pub const SPOT: UniverseKind = UniverseKind {
    bybit_category: "spot",
    persist_filename: "active_symbols_spot.json",
    edge_scores_filename: "edge_scores_spot.json",
    dashboard_kind: "spot",
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EdgeStats {
    recent: VecDeque<f64>,
}

impl EdgeStats {
    fn record(&mut self, edge: f64) {
        self.recent.push_back(edge);
        if self.recent.len() > EDGE_HISTORY_CAP {
            self.recent.pop_front();
        }
    }

    pub fn mean(&self) -> f64 {
        if self.recent.is_empty() {
            0.0
        } else {
            self.recent.iter().sum::<f64>() / self.recent.len() as f64
        }
    }

    pub fn count(&self) -> usize {
        self.recent.len()
    }
}

/// Compartilhado entre o Order Flow (que alimenta) e a tarefa de rotação
/// (que lê) — `std::sync::Mutex` porque as seções críticas são curtas
/// (inserir um f64, calcular uma média), não vale a complexidade de um
/// lock assíncrono pra isso.
pub type EdgeScores = Arc<Mutex<HashMap<String, EdgeStats>>>;

pub fn new_edge_scores() -> EdgeScores {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn record_edge(scores: &EdgeScores, symbol: &str, net_edge: f64) {
    let Ok(mut map) = scores.lock() else { return };
    map.entry(symbol.to_string()).or_default().record(net_edge);
}

fn edge_scores_path(filename: &str) -> String {
    format!("{}/data/{filename}", env!("CARGO_MANIFEST_DIR"))
}

/// Recupera o edge medido salvo em disco — se não existir ou estiver
/// corrompido, começa vazio normalmente (mesmo padrão de
/// `risk::PortfolioState::load_or_new`: ausência de arquivo não é erro).
fn load_edge_scores(filename: &str) -> HashMap<String, EdgeStats> {
    let path = edge_scores_path(filename);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    serde_json::from_str(raw).unwrap_or_default()
}

/// Salva o edge medido em disco — chamado a cada rotação (15min) E
/// periodicamente entre rotações (ver `main.rs`), pra nunca perder mais
/// que alguns segundos de medição num restart abrupto.
pub fn save_edge_scores(scores: &EdgeScores, filename: &str) {
    let Ok(map) = scores.lock() else { return };
    let Ok(json) = serde_json::to_string(&*map) else { return };
    let path = edge_scores_path(filename);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, json);
}

/// Tarefa de fundo: a cada `ROTATION_INTERVAL`, busca a lista completa de
/// perpétuos USDT da Bybit (ordenada por volume 24h — filtro de liquidez
/// mínima, não filtro de "achismo"), decide a lista ativa com base no edge
/// medido de verdade, persiste em disco pra observabilidade, e avisa todo
/// mundo que está escutando (Arbitragem, Order Flow, Pump Exhaustion,
/// Liquidation Hunter) via `watch::Sender` — cada um reconecta sozinho com
/// a lista nova (mesmo caminho de reconexão que já existia pra erro de
/// rede, só que disparado por uma mudança de lista em vez de uma queda).
pub async fn run(kind: UniverseKind, scores: EdgeScores, tx: watch::Sender<Vec<String>>, seed: Vec<String>, bus: EventBus) {
    let mut exploration_cursor: usize = 0;
    let mut candidate_pool: Vec<String> = seed.clone();
    let mut first_round = true;

    // Recupera o edge medido de sessões anteriores ANTES da primeira
    // rotação — sem isso, um restart (ainda que segundos depois de um
    // save) descartaria todo o histórico acumulado e a primeira rotação
    // trataria tudo como "nunca visto", igual a um boot do zero.
    let restored = load_edge_scores(kind.edge_scores_filename);
    if !restored.is_empty() {
        let count = restored.len();
        if let Ok(mut map) = scores.lock() {
            *map = restored;
        }
        tracing::info!(simbolos = count, categoria = kind.bybit_category, "universo de símbolos: edge medido recuperado do disco");
    }

    loop {
        match fetch_candidate_pool(kind.bybit_category).await {
            Ok(list) if !list.is_empty() => {
                tracing::info!(total = list.len(), categoria = kind.bybit_category, "universo de símbolos: lista de candidatos atualizada (Bybit, por volume 24h)");
                candidate_pool = list;
            }
            Ok(_) => {
                tracing::warn!(categoria = kind.bybit_category, "universo de símbolos: Bybit retornou lista vazia, mantendo candidatos anteriores");
            }
            Err(e) => {
                tracing::warn!(error = %e, categoria = kind.bybit_category, "universo de símbolos: falha ao buscar instrumentos da Bybit, mantendo candidatos anteriores");
            }
        }

        let active = rotate(&scores, &candidate_pool, &seed, first_round, &mut exploration_cursor);
        first_round = false;
        persist(&active, &scores, kind.persist_filename);
        save_edge_scores(&scores, kind.edge_scores_filename);
        tracing::info!(total = active.len(), categoria = kind.bybit_category, "universo de símbolos: rotação concluída");
        bus.emit(DashboardEvent::symbol_universe(kind.dashboard_kind, active.len(), top_ranked(&active, &scores, 12)));
        if tx.send(active).is_err() {
            tracing::warn!(categoria = kind.bybit_category, "universo de símbolos: nenhum consumidor ouvindo mais, encerrando tarefa de rotação");
            return;
        }

        tokio::time::sleep(ROTATION_INTERVAL).await;
    }
}

fn rotate(
    scores: &EdgeScores,
    candidate_pool: &[String],
    seed: &[String],
    first_round: bool,
    exploration_cursor: &mut usize,
) -> Vec<String> {
    let map = scores.lock().map(|g| g.clone()).unwrap_or_default();

    // "Comprovados": amostra suficiente E edge médio recente positivo —
    // não é só "o que já foi tentado", é o que realmente mostrou vantagem.
    let mut proven: Vec<(String, f64)> = map
        .iter()
        .filter(|(_, s)| s.count() >= MIN_SAMPLES_TO_TRUST && s.mean() > 0.0)
        .map(|(sym, s)| (sym.clone(), s.mean()))
        .collect();
    proven.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut active: Vec<String> = proven.iter().map(|(s, _)| s.clone()).collect();

    // No primeiro ciclo (boot, sem histórico ainda) parte da lista atual
    // (antigo WIDE_SYMBOLS como semente) em vez de zerar a cobertura —
    // evita perder de uma vez os símbolos que já sabíamos ter edge
    // (GRTUSDT, QTUMUSDT) só porque o processo reiniciou.
    if first_round {
        for s in seed {
            if !active.contains(s) {
                active.push(s.clone());
            }
        }
    }

    // Vagas de exploração: sempre reservadas (mínimo MIN_EXPLORATION_SLOTS),
    // crescem sozinhas quando ainda não achamos muito comprovado — é assim
    // que o sistema nunca "fica travado no mesmo": mesmo com 60+ símbolos
    // comprovados, uma fatia continua girando por candidatos novos, e um
    // símbolo comprovado que piorar (janela móvel decai o mean) some
    // sozinho de `proven` no próximo ciclo, liberando espaço.
    let exploration_needed = MIN_EXPLORATION_SLOTS.max(TOTAL_TARGET.saturating_sub(active.len()));
    let mut added = 0;
    let mut idx = *exploration_cursor;
    let pool_len = candidate_pool.len().max(1);
    for _ in 0..pool_len {
        if added >= exploration_needed {
            break;
        }
        let candidate = &candidate_pool[idx % pool_len];
        if !active.contains(candidate) {
            active.push(candidate.clone());
            added += 1;
        }
        idx += 1;
    }
    *exploration_cursor = idx % pool_len;

    active.truncate(TOTAL_TARGET.max(active.len().min(TOTAL_TARGET + MIN_EXPLORATION_SLOTS)));
    active
}

fn ranked_symbols(active: &[String], scores: &EdgeScores) -> Vec<SymbolRanking> {
    let map = scores.lock().map(|g| g.clone()).unwrap_or_default();
    let mut ranked: Vec<SymbolRanking> = active
        .iter()
        .map(|sym| {
            let stats = map.get(sym);
            SymbolRanking {
                symbol: sym.clone(),
                mean_edge_pct: stats.map(|s| s.mean() * 100.0).unwrap_or(0.0),
                samples: stats.map(|s| s.count()).unwrap_or(0),
            }
        })
        .collect();
    ranked.sort_by(|a, b| b.mean_edge_pct.partial_cmp(&a.mean_edge_pct).unwrap_or(std::cmp::Ordering::Equal));
    ranked
}

fn top_ranked(active: &[String], scores: &EdgeScores, n: usize) -> Vec<SymbolRanking> {
    let mut ranked = ranked_symbols(active, scores);
    ranked.truncate(n);
    ranked
}

fn persist(active: &[String], scores: &EdgeScores, filename: &str) {
    let ranked = ranked_symbols(active, scores);
    let path = format!("{}/data/{filename}", env!("CARGO_MANIFEST_DIR"));
    let payload = serde_json::json!({
        "updated_ms": crate::raw_log::now_ms(),
        "total": active.len(),
        "symbols": ranked,
    });
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, json);
    }
}

/// Lista completa de instrumentos USDT ativos na Bybit pra uma `category`
/// (linear = perpétuos, spot = pares à vista), ordenada por `turnover24h`
/// (volume em dólar nas últimas 24h) — filtro de liquidez mínima real, não
/// uma lista escolhida à mão. Top CANDIDATE_POOL_CAP por volume vira o pool
/// de candidatos pra exploração.
async fn fetch_candidate_pool(category: &str) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .user_agent("AurumOS-ResearchBot/0.1")
        .timeout(Duration::from_secs(15))
        .build()?;
    let url = format!("{BYBIT_TICKERS_URL_BASE}{category}");
    let resp = client.get(&url).send().await?.error_for_status()?;
    let body: Value = resp.json().await?;
    let list = body
        .get("result")
        .and_then(|r| r.get("list"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut with_volume: Vec<(String, f64)> = list
        .iter()
        .filter_map(|item| {
            let symbol = item.get("symbol")?.as_str()?;
            if !symbol.ends_with("USDT") {
                return None;
            }
            let turnover: f64 = item.get("turnover24h")?.as_str()?.parse().ok()?;
            Some((symbol.to_string(), turnover))
        })
        .collect();
    with_volume.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    with_volume.truncate(CANDIDATE_POOL_CAP);
    Ok(with_volume.into_iter().map(|(s, _)| s).collect())
}
