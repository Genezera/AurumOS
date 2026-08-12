use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Comunicação real entre módulos (pedido do usuário, 13/08/2026): Whale
/// Watch e Liquidation Hunter coletam dado real mas ficavam isolados,
/// sempre em net_edge=0, nunca influenciando nada. Aqui eles passam a
/// alimentar o Pump Exhaustion como reforço de confiança — exatamente o
/// exemplo dado: "pump detectado + depósitos de whales em exchange +
/// liquidações compradoras crescendo = candidato forte de exaustão".

/// ---------------- Liquidation Hunter → quadro por símbolo ----------------

#[derive(Debug, Clone, Copy)]
pub struct LiquidationSignal {
    pub liquidated_longs: bool,
    pub same_side_count: usize,
    pub at: Instant,
}

pub type LiquidationBoard = Arc<Mutex<HashMap<String, LiquidationSignal>>>;

pub fn new_liquidation_board() -> LiquidationBoard {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn record_cascade(board: &LiquidationBoard, symbol: &str, liquidated_longs: bool, same_side_count: usize) {
    if let Ok(mut map) = board.lock() {
        map.insert(symbol.to_string(), LiquidationSignal { liquidated_longs, same_side_count, at: Instant::now() });
    }
}

/// true se uma cascata de liquidação de LONGS (posições compradas forçadas
/// a fechar) ocorreu pra esse símbolo dentro da janela — sinal clássico de
/// pressão vendedora que reforça uma leitura de exaustão de alta.
pub fn recent_long_liquidation_cascade(board: &LiquidationBoard, symbol: &str, within: Duration) -> bool {
    board
        .lock()
        .ok()
        .and_then(|map| map.get(symbol).copied())
        .map(|sig| sig.liquidated_longs && sig.at.elapsed() < within)
        .unwrap_or(false)
}

/// ---------------- Whale Watch → quadro agregado de mercado ----------------

// Stablecoin não tem "símbolo" de trading igual par cripto — o sinal do
// Whale Watch é de mercado geral (capital indo pra exchanges = pressão
// vendedora ampla), não específico de um símbolo. Janela de 30min: tempo
// suficiente pra capturar um movimento coordenado sem ficar preso a um
// único evento isolado.
const WHALE_FLOW_WINDOW: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Default)]
pub struct WhaleFlow {
    deposits: VecDeque<(Instant, f64)>,
}

pub type WhaleBoard = Arc<Mutex<WhaleFlow>>;

pub fn new_whale_board() -> WhaleBoard {
    Arc::new(Mutex::new(WhaleFlow::default()))
}

pub fn record_deposit(board: &WhaleBoard, amount_usd: f64) {
    let Ok(mut flow) = board.lock() else { return };
    flow.deposits.push_back((Instant::now(), amount_usd));
    while flow.deposits.front().map(|(t, _)| t.elapsed() > WHALE_FLOW_WINDOW).unwrap_or(false) {
        flow.deposits.pop_front();
    }
}

/// Total depositado em exchanges conhecidas dentro da janela recente.
pub fn recent_deposit_pressure_usd(board: &WhaleBoard) -> f64 {
    board.lock().ok().map(|flow| flow.deposits.iter().map(|(_, v)| v).sum()).unwrap_or(0.0)
}
