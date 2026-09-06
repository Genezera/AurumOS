use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static SIGNAL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Market {
    Crypto,
    Stocks,
    Index,
}

impl Market {
    pub fn key(self) -> &'static str {
        match self {
            Market::Crypto => "crypto",
            Market::Stocks => "stocks",
            Market::Index => "index",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strategy {
    OrderFlow,
    FundingCarry,
    News,
    Launch,
    PumpExhaustion,
    WhaleWatch,
    Macro,
    LiquidationHunter,
}

impl Strategy {
    /// Todas as variantes — usada pra inicializar/reidratar estruturas que
    /// precisam de um estado por estratégia (ex.: `risk::StrategyScaling`)
    /// sem depender de uma crate de enum-iteração externa.
    pub const ALL: [Strategy; 8] = [
        Strategy::OrderFlow,
        Strategy::FundingCarry,
        Strategy::News,
        Strategy::Launch,
        Strategy::PumpExhaustion,
        Strategy::WhaleWatch,
        Strategy::Macro,
        Strategy::LiquidationHunter,
    ];

    /// Chave usada para casar com as tabelas do config/risk.toml.
    pub fn key(&self) -> &'static str {
        match self {
            Strategy::OrderFlow => "order_flow",
            Strategy::FundingCarry => "funding_carry",
            Strategy::News => "news",
            Strategy::Launch => "launch",
            Strategy::PumpExhaustion => "pump_exhaustion",
            Strategy::WhaleWatch => "whale_watch",
            Strategy::Macro => "macro",
            Strategy::LiquidationHunter => "liquidation_hunter",
        }
    }

    /// Order Flow vive de movimentos pequenos e repetidos e forma o motor de base,
    /// reinvestindo 80%/protegendo 20%. As demais são "eventos raros"
    /// (lançamento, baleia, notícia, macro, exaustão de pump, liquidação):
    /// quando dão lucro, o PDF pede uma divisão de 3 vias — 70% reinvestido,
    /// 20% reserva protegida, 10% pra infraestrutura/custos — porque são
    /// oportunidades esporádicas, não um fluxo constante que sustenta custo
    /// operacional sozinho.
    pub fn is_rare_event(&self) -> bool {
        !matches!(self, Strategy::OrderFlow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Long,
    Short,
}

impl Direction {
    pub fn key(self) -> &'static str {
        match self {
            Direction::Long => "long",
            Direction::Short => "short",
        }
    }
}

/// Define se uma oportunidade pode chegar ao livro paper. Fontes de
/// inteligência continuam visíveis no dashboard, mas não podem gerar PnL
/// até existir um executor que devolva um resultado ligado ao mesmo sinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    ObservationOnly,
    ExecutableQuoted,
}

/// Destino das oportunidades aprovadas. O binário não contém backend para
/// o domínio de produção da Bybit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionBackend {
    Shadow,
    BybitDemo,
}

impl ExecutionBackend {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "shadow" => Ok(Self::Shadow),
            "demo" | "bybit_demo" => Ok(Self::BybitDemo),
            other => anyhow::bail!("AURUMOS_EXECUTION_MODE inválido: {other}; use shadow ou demo"),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::BybitDemo => "demo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Filled,
    Unfilled,
    OpenRisk,
}

/// Ordem já aprovada pelo risk engine e pronta para o adaptador Demo.
#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub signal_id: u64,
    pub symbol: String,
    pub direction: Direction,
    pub quantity: f64,
    pub price_tick: f64,
    pub stop_loss_pct: f64,
    pub expected_holding_secs: f64,
}

/// Resultado observável produzido pelo executor shadow ou Bybit Demo. Ele referencia a
/// oportunidade original por `signal_id`; o orquestrador não aceita um
/// retorno histórico aleatório nem inventa o desfecho por probabilidade.
#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub signal_id: u64,
    pub status: ExecutionStatus,
    /// Retorno líquido sobre o notional de uma perna, após taxas.
    pub return_pct: f64,
    /// PnL absoluto informado pela venue. `None` no shadow, onde o valor é
    /// derivado da cotação e do notional reservado; `Some` no Demo preserva
    /// exatamente `execValue` e `execFee` mesmo se o preço mudou.
    pub realized_pnl: Option<f64>,
    /// Notional máximo suportado simultaneamente pelo topo dos dois books.
    /// Zero significa que a cotação não pôde ser considerada executável.
    pub max_executable_notional: f64,
    pub observed_latency_ms: u64,
    pub note: String,
}

/// ID temporal com contador de três dígitos. O valor permanece abaixo do
/// limite inteiro exato do JavaScript, para que JSON e dashboard exibam o
/// mesmo ID que o Rust usa na ligação entre sinal e resultado shadow.
pub fn next_signal_id() -> u64 {
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    epoch_ms
        .saturating_mul(1_000)
        .saturating_add(SIGNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed) % 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_id_is_exact_in_javascript_numbers() {
        assert!(next_signal_id() <= 9_007_199_254_740_991);
    }

    #[test]
    fn execution_backend_is_explicit_and_fail_closed() {
        assert_eq!(
            ExecutionBackend::parse("shadow").unwrap(),
            ExecutionBackend::Shadow
        );
        assert_eq!(
            ExecutionBackend::parse("demo").unwrap(),
            ExecutionBackend::BybitDemo
        );
        assert!(ExecutionBackend::parse("mainnet").is_err());
    }
}

/// Um sinal bruto emitido por um módulo de dados. O orquestrador nunca
/// confia cegamente nele: só `ExecutableQuoted` passa pelo motor de risco.
#[derive(Debug, Clone)]
pub struct Opportunity {
    pub signal_id: u64,
    pub market: Market,
    pub strategy: Strategy,
    pub asset: String,
    pub direction: Direction,
    /// Vantagem líquida esperada, já descontando taxas/slippage estimados.
    /// Fração, ex.: 0.004 = 0.40%.
    pub net_edge: f64,
    /// Confiança do módulo emissor no sinal, 0.0 a 1.0.
    pub confidence: f64,
    /// Por quanto tempo esse sinal continua válido depois de emitido.
    pub valid_for_ms: u64,
    /// Estimativa de quanto tempo o capital ficaria comprometido se essa
    /// oportunidade virasse ordem. O executor atual mede cinco segundos;
    /// sensores podem declarar horizontes maiores apenas para contexto.
    pub expected_holding_secs: f64,
    /// Capital necessário para executar essa perna, em USD.
    pub capital_needed: f64,
    /// Preço usado para converter notional em quantidade e aplicar
    /// minOrderQty/qtyStep da corretora. Fontes observacionais podem omitir.
    pub reference_price: Option<f64>,
    /// Perda máxima estimada se o sinal falhar, como fração do capital da perna.
    pub max_loss_pct: f64,
    /// Alavancagem solicitada por essa oportunidade (1.0 = sem alavancagem).
    pub leverage: f64,
    /// Agrupa oportunidades que representam essencialmente a mesma aposta de
    /// risco (ex.: "altcoins", "nasdaq_tech", "usd_macro") para impedir que o
    /// orquestrador acumule exposição correlacionada sem perceber.
    pub correlation_group: String,
    /// Apenas `ExecutableQuoted` pode ser aprovado. `ObservationOnly` mantém o
    /// scanner ativo sem transformar uma hipótese em lucro contábil.
    pub execution_mode: ExecutionMode,
    /// Quantas pernas do mesmo notional comprometem capital. O executor
    /// direcional atual usa 1; o campo evita subcontagem em extensões.
    pub capital_multiplier: f64,
    pub emitted_at: Instant,
}

impl Opportunity {
    pub fn is_expired(&self) -> bool {
        self.emitted_at.elapsed().as_millis() as u64 > self.valid_for_ms
    }

    /// score = (vantagem_liquida * confianca) / risco_de_cauda / capital_necessario / horas_de_capital_preso
    /// Usamos max_loss_pct como proxy de "risco de cauda": quanto maior a
    /// perda potencial relativa, menor o score para o mesmo edge. O termo
    /// de tempo (horas, não segundos — evita que score exploda pra
    /// oportunidades sub-segundo) converte isso numa aproximação de
    /// "retorno por unidade de capital E de tempo": duas oportunidades com
    /// o mesmo edge/risco, mas uma prende capital por 20 minutos e outra
    /// por 2 segundos, não são igualmente atraentes — a rápida libera
    /// capital pra ser reaproveitado muito mais vezes no mesmo período.
    pub fn score(&self) -> f64 {
        let tail_risk = self.max_loss_pct.max(0.0001);
        let holding_hours = (self.expected_holding_secs / 3600.0).max(0.0001);
        (self.net_edge * self.confidence) / tail_risk / self.capital_needed.max(1.0) / holding_hours
    }

    /// Reduz `asset` ao símbolo/token que o identifica. Pump Exhaustion anexa contexto como
    /// "DOGEUSDT (funding 0,15%, 24h +18%)"; corta no primeiro espaço ou
    /// parêntese pra comparar de forma justa entre módulos. Usado tanto pra
    /// confluência (orchestrator.rs) quanto pro Kelly hierárquico por
    /// símbolo (risk.rs).
    pub fn base_symbol(&self) -> &str {
        self.asset
            .split([' ', '('])
            .next()
            .unwrap_or(&self.asset)
            .trim()
    }
}
