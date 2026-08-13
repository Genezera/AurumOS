use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Market {
    Crypto,
    Stocks,
    Forex,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strategy {
    Arbitrage,
    OrderFlow,
    News,
    Launch,
    PumpExhaustion,
    WhaleWatch,
    Macro,
    LiquidationHunter,
    MultiAsset,
}

impl Strategy {
    /// Todas as variantes — usada pra inicializar/reidratar estruturas que
    /// precisam de um estado por estratégia (ex.: `risk::StrategyScaling`)
    /// sem depender de uma crate de enum-iteração externa.
    pub const ALL: [Strategy; 9] = [
        Strategy::Arbitrage,
        Strategy::OrderFlow,
        Strategy::News,
        Strategy::Launch,
        Strategy::PumpExhaustion,
        Strategy::WhaleWatch,
        Strategy::Macro,
        Strategy::LiquidationHunter,
        Strategy::MultiAsset,
    ];

    /// Chave usada para casar com as tabelas do config/risk.toml.
    pub fn key(&self) -> &'static str {
        match self {
            Strategy::Arbitrage => "arbitrage",
            Strategy::OrderFlow => "order_flow",
            Strategy::News => "news",
            Strategy::Launch => "launch",
            Strategy::PumpExhaustion => "pump_exhaustion",
            Strategy::WhaleWatch => "whale_watch",
            Strategy::Macro => "macro",
            Strategy::LiquidationHunter => "liquidation_hunter",
            Strategy::MultiAsset => "multi_asset",
        }
    }

    /// Estratégias "contínuas" (Arbitragem, Order Flow) vivem de captura de
    /// spread pequeno e repetido — o PDF as trata como o motor de base,
    /// reinvestindo 80%/protegendo 20%. As demais são "eventos raros"
    /// (lançamento, baleia, notícia, macro, exaustão de pump, liquidação):
    /// quando dão lucro, o PDF pede uma divisão de 3 vias — 70% reinvestido,
    /// 20% reserva protegida, 10% pra infraestrutura/custos — porque são
    /// oportunidades esporádicas, não um fluxo constante que sustenta custo
    /// operacional sozinho.
    pub fn is_rare_event(&self) -> bool {
        !matches!(self, Strategy::Arbitrage | Strategy::OrderFlow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Long,
    Short,
}

/// Um sinal bruto emitido por um módulo de dados (arbitragem, whale watch,
/// news reactor, etc). O orquestrador nunca confia cegamente nisso — tudo
/// passa pelo motor de risco antes de virar uma ordem simulada.
#[derive(Debug, Clone)]
pub struct Opportunity {
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
    /// oportunidade virasse ordem — arbitragem é ~instantâneo, order flow
    /// espera preenchimento de ordem passiva, pump exhaustion usa a mesma
    /// janela da camada de confirmação (20min). Adicionado após revisão
    /// técnica externa (13/08/2026): "score não considera velocidade...
    /// lucro líquido depende de capital E tempo". Sem isso, uma
    /// oportunidade lenta e uma rápida com o mesmo score pareciam
    /// igualmente atraentes, quando na prática a rápida libera o capital
    /// pra reaproveitar muito mais vezes no mesmo período.
    pub expected_holding_secs: f64,
    /// Capital necessário para executar essa perna, em USD.
    pub capital_needed: f64,
    /// Perda máxima estimada se o sinal falhar, como fração do capital da perna.
    pub max_loss_pct: f64,
    /// Alavancagem solicitada por essa oportunidade (1.0 = sem alavancagem).
    pub leverage: f64,
    /// Agrupa oportunidades que representam essencialmente a mesma aposta de
    /// risco (ex.: "altcoins", "nasdaq_tech", "usd_macro") para impedir que o
    /// orquestrador acumule exposição correlacionada sem perceber.
    pub correlation_group: String,
    /// Reamostragem real (bootstrap) de um desfecho JÁ CONFIRMADO contra
    /// preço real, sorteado no instante da emissão a partir do histórico de
    /// confirmação da estratégia — não uma fórmula (edge×confiança), um
    /// desfecho que realmente aconteceu antes com um sinal parecido.
    /// Achado ao vivo (13/08/2026, auditoria externa): o modelo anterior
    /// decidia ganhou/perdeu por sorteio ponderado por uma probabilidade
    /// agregada — válido (não olha o futuro DESTA operação), mas não é o
    /// mesmo que herdar um resultado real observado. `None` = estratégia
    /// ainda sem confirmação suficiente (ou que não usa este mecanismo) —
    /// nesse caso o orquestrador cai no sorteio antigo.
    pub sampled_return: Option<f64>,
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

    /// Reduz `asset` ao símbolo/token que o identifica — arbitragem e order
    /// flow mandam "DOGEUSDT" puro, mas pump exhaustion anexa contexto como
    /// "DOGEUSDT (funding 0,15%, 24h +18%)"; corta no primeiro espaço ou
    /// parêntese pra comparar de forma justa entre módulos. Usado tanto pra
    /// confluência (orchestrator.rs) quanto pro Kelly hierárquico por
    /// símbolo (risk.rs).
    pub fn base_symbol(&self) -> &str {
        self.asset.split([' ', '(']).next().unwrap_or(&self.asset).trim()
    }
}
