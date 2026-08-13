use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::types::{Opportunity, Strategy};

#[derive(Debug, Deserialize, Clone)]
pub struct LeverageConfig {
    pub launch_max: f64,
    pub liquid_max: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScalingConfig {
    pub min_cycles_between_scale: u32,
    pub max_drawdown_pct_for_scale: f64,
    pub drawdown_halve_threshold_pct: f64,
    pub scale_growth_factor: f64,
    pub max_leg_fraction_of_equity: f64,
    /// Profit factor mínimo (janela móvel recente) exigido pra permitir
    /// aumentar a perna — revisão técnica externa, 13/08/2026: ciclos +
    /// drawdown baixo sozinhos não provam que a estratégia está lucrando de
    /// verdade nesse trecho recente.
    pub min_recent_profit_factor_for_scale: f64,
    pub min_recent_samples_for_scale: usize,
    /// Recuperação parcial (revisão pós-fusão, 12/08/2026): fração do maior
    /// drawdown LOCAL (pico→vale de PnL acumulado) que uma estratégia
    /// precisa recuperar antes de poder escalonar — substitui a exigência
    /// antiga de `equity >= peak_equity` (pico de TODO o portfólio), que
    /// travava uma estratégia individualmente lucrativa só porque outra
    /// ainda não tinha recuperado.
    pub partial_recovery_fraction: f64,
    /// Fração do Kelly cheio realmente aplicada ao tamanho da perna (Kelly
    /// fracionário) — Kelly cheio assume p/b exatos e conhecidos, o que
    /// nunca é o caso com amostra finita e ruidosa; uma fração (ex. 0.3)
    /// mantém a maior parte do crescimento perdendo bem menos robustez.
    pub kelly_safety_fraction: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    pub total_equity_start: f64,
    pub protected_reserve_pct: f64,
    pub reinvest_pct: f64,
    pub rare_event_reserve_pct: f64,
    pub rare_event_infra_pct: f64,
    pub initial_leg_size: f64,
    pub strategy_risk_pct: HashMap<String, f64>,
    pub total_risk_pct: f64,
    pub daily_warn_pct: f64,
    pub daily_halt_pct: f64,
    pub weekly_halt_pct: f64,
    pub total_drawdown_halt_pct: f64,
    pub leverage: LeverageConfig,
    pub scaling: ScalingConfig,
}

impl RiskConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("falha ao ler {path}: {e}"))?;
        let cfg: RiskConfig = toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("falha ao parsear {path}: {e}"))?;
        Ok(cfg)
    }

    fn strategy_limit_pct(&self, strategy: Strategy) -> f64 {
        *self
            .strategy_risk_pct
            .get(strategy.key())
            .unwrap_or(&0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeOutcome {
    Win,
    Loss,
}

#[derive(Debug, Clone)]
pub struct PortfolioState {
    pub equity: f64,
    pub protected_reserve: f64,
    /// Fatia de 10% do lucro de eventos raros reservada pro conceito de
    /// "infraestrutura e custos" do PDF. Em paper trading não existe custo
    /// real pra pagar, então isso só fica separado e visível — não é
    /// gasto automaticamente em nada.
    pub infra_reserve: f64,
    pub peak_equity: f64,
    /// Equity no início do dia/semana UTC corrente — base dos dois
    /// primeiros níveis do kill-switch em camadas, separado do
    /// `peak_equity` histórico (esse é all-time, não reseta periodicamente).
    pub day_start_equity: f64,
    pub day_start_epoch_day: i64,
    pub week_start_equity: f64,
    pub week_start_epoch_week: i64,
    /// Trava do nível mais grave do kill-switch (drawdown total desde o
    /// pico histórico) — diferente dos outros 3 níveis, este NÃO libera
    /// sozinho quando o equity recupera; fica travado até confirmação
    /// manual explícita (ver `data/RESET_DRAWDOWN_HALT` em
    /// `kill_switch_reason`). Persiste entre restarts de propósito — um
    /// restart não deve ser uma forma acidental de destravar o nível mais
    /// grave de proteção.
    pub total_drawdown_latched: bool,
    pub total_cycles: u64,
    pub exposure_by_strategy: HashMap<Strategy, f64>,
    pub exposure_by_group: HashMap<String, f64>,
    pub last_outcome_by_strategy: HashMap<Strategy, TradeOutcome>,
    pub last_order_size_by_strategy: HashMap<Strategy, f64>,
    pub wins: u64,
    pub losses: u64,
    pub rejections: u64,
    /// Escalonamento por estratégia (pedido do usuário, 12/08/2026: "cada
    /// estratégia rastreia seu próprio PnL acumulado, pico e leg_size —
    /// ganha o direito de crescer pelo próprio histórico, não pelo pico do
    /// portfólio inteiro"). Substitui o antigo `leg_size`/`recent_pnls`/
    /// `cycles_since_scale` globais únicos. Sempre populado com todas as
    /// `Strategy::ALL` — `leg_size()` nunca deveria precisar de fallback.
    pub strategy_scaling: HashMap<Strategy, StrategyScaling>,
    /// Kelly hierárquico (auditoria externa, 12/08/2026): PnL recente POR
    /// SÍMBOLO dentro de cada estratégia — "Order Flow recebe X% de
    /// orçamento; QTUM recebe 55% desse orçamento; GRT recebe 25%". Só o
    /// suficiente pra calcular um Kelly por símbolo e comparar contra o da
    /// estratégia inteira (ver `symbol_allocation_fraction`); não duplica
    /// leg_size/pico/vale por símbolo — essa continua sendo uma decisão de
    /// estratégia (Seção 9.1 do roadmap). Não persiste entre restarts de
    /// propósito, mesmo raciocínio do `recent_pnls` de `StrategyScaling`
    /// abaixo — refila rápido com o volume de trades por símbolo.
    pub symbol_scaling: HashMap<(Strategy, String), SymbolScaling>,
}

/// PnL recente de UM símbolo dentro de UMA estratégia — a metade
/// "símbolo" do Kelly hierárquico (portfólio→estratégia→SÍMBOLO). Sem
/// leg_size/pico/vale próprios de propósito: o tamanho continua vindo da
/// estratégia (`StrategyScaling::leg_size`), isto só ajusta QUANTO desse
/// tamanho vai pra este símbolo específico.
#[derive(Debug, Clone, Default)]
pub struct SymbolScaling {
    pub recent_pnls: VecDeque<f64>,
}

/// Estado de escalonamento de UMA estratégia — perna operacional, PnL
/// acumulado (não equity, que é do portfólio inteiro) e a janela móvel de
/// resultados recentes usada pelos gates de profit factor/robustez.
#[derive(Debug, Clone)]
pub struct StrategyScaling {
    pub leg_size: f64,
    pub cumulative_pnl: f64,
    pub peak_cumulative_pnl: f64,
    /// Menor `cumulative_pnl` visto desde o último novo pico — junto com
    /// `peak_cumulative_pnl`, define o tamanho do drawdown local usado pela
    /// recuperação parcial (`ScalingConfig::partial_recovery_fraction`).
    pub trough_since_peak: f64,
    pub cycles_since_scale: u32,
    /// Janela móvel dos últimos PnLs DESSA estratégia — não persiste entre
    /// restarts de propósito (mesmo raciocínio do campo global antigo:
    /// filtro de qualidade recente, não capital; perdê-la só significa
    /// esperar a janela reencher).
    pub recent_pnls: VecDeque<f64>,
}

impl StrategyScaling {
    fn new(initial_leg_size: f64) -> Self {
        Self {
            leg_size: initial_leg_size,
            cumulative_pnl: 0.0,
            peak_cumulative_pnl: 0.0,
            trough_since_peak: 0.0,
            cycles_since_scale: 0,
            recent_pnls: VecDeque::new(),
        }
    }
}

/// Só os campos que fazem sentido sobreviver a um restart do processo —
/// exposição aberta e "último resultado por estratégia" são artefatos do
/// ciclo em andamento no momento em que o processo caiu, não capital de
/// verdade; perdê-los é inofensivo (e recarregar exposição "presa" de uma
/// ordem que nunca vai fechar seria pior que zerar).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedState {
    equity: f64,
    protected_reserve: f64,
    #[serde(default)]
    infra_reserve: f64,
    peak_equity: f64,
    #[serde(default)]
    day_start_equity: f64,
    #[serde(default)]
    day_start_epoch_day: i64,
    #[serde(default)]
    week_start_equity: f64,
    #[serde(default)]
    week_start_epoch_week: i64,
    #[serde(default)]
    total_drawdown_latched: bool,
    /// Campo do formato antigo (leg_size global único) — mantido só como
    /// semente pra estratégias sem entrada própria em `strategy_scaling`
    /// num arquivo salvo antes desta revisão (12/08/2026). Novos saves
    /// sempre populam `strategy_scaling` e este campo vira só um eco dele.
    #[serde(default)]
    leg_size: f64,
    total_cycles: u64,
    #[serde(default)]
    strategy_scaling: HashMap<String, StrategyScalingPersisted>,
    wins: u64,
    losses: u64,
    rejections: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StrategyScalingPersisted {
    leg_size: f64,
    cumulative_pnl: f64,
    peak_cumulative_pnl: f64,
    trough_since_peak: f64,
    cycles_since_scale: u32,
}

impl PortfolioState {
    pub fn new(cfg: &RiskConfig) -> Self {
        Self {
            equity: cfg.total_equity_start,
            protected_reserve: 0.0,
            infra_reserve: 0.0,
            peak_equity: cfg.total_equity_start,
            day_start_equity: cfg.total_equity_start,
            day_start_epoch_day: today_epoch_day(),
            week_start_equity: cfg.total_equity_start,
            week_start_epoch_week: today_epoch_week(),
            total_drawdown_latched: false,
            total_cycles: 0,
            exposure_by_strategy: HashMap::new(),
            exposure_by_group: HashMap::new(),
            last_outcome_by_strategy: HashMap::new(),
            last_order_size_by_strategy: HashMap::new(),
            wins: 0,
            losses: 0,
            rejections: 0,
            strategy_scaling: Strategy::ALL
                .iter()
                .map(|&s| (s, StrategyScaling::new(cfg.initial_leg_size)))
                .collect(),
            symbol_scaling: HashMap::new(),
        }
    }

    /// Tamanho da perna operacional da estratégia — 0.0 nunca deveria
    /// acontecer de verdade (`strategy_scaling` é sempre populado com todas
    /// as `Strategy::ALL`), mas o fallback evita pânico se um novo valor de
    /// `Strategy` for adicionado sem passar por `new()`/`load_or_new()`.
    pub fn leg_size(&self, strategy: Strategy) -> f64 {
        self.strategy_scaling
            .get(&strategy)
            .map(|s| s.leg_size)
            .unwrap_or(0.0)
    }

    /// Tenta recuperar o estado salvo em `path`; se não existir ou estiver
    /// corrompido, começa do zero normalmente (não é um erro fatal — é
    /// exatamente o que aconteceria na primeiríssima vez que o processo
    /// roda). Isso é o que evita o equity voltar pra US$200 toda vez que o
    /// processo reinicia (pra aplicar código novo, por exemplo).
    pub fn load_or_new(cfg: &RiskConfig, path: &str) -> Self {
        let fresh = Self::new(cfg);
        let Ok(raw) = std::fs::read_to_string(path) else {
            tracing::info!(path, "nenhum estado salvo encontrado, começando do zero");
            return fresh;
        };
        // Alguns editores/ferramentas no Windows salvam JSON com um BOM
        // UTF-8 na frente, que não é whitespace válido pra um parser JSON
        // estrito — tira isso antes de tentar, em vez de descartar um
        // estado bom só por causa de 3 bytes invisíveis.
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        let Ok(persisted) = serde_json::from_str::<PersistedState>(raw) else {
            tracing::warn!(path, "estado salvo corrompido ou de formato antigo, começando do zero");
            return fresh;
        };
        tracing::info!(
            equity = persisted.equity,
            total_equity = persisted.equity + persisted.protected_reserve + persisted.infra_reserve,
            ciclos = persisted.total_cycles,
            "estado recuperado do disco — continuando de onde parou"
        );
        // day_start_equity/day_start_epoch_day de um arquivo salvo antes
        // desta revisão vêm como 0 (default do serde) — trata como "sem dado
        // do dia ainda" e recomeça o dia a partir do equity restaurado, em
        // vez de herdar um valor 0 que faria o drawdown diário parecer de
        // -100% no primeiro tick.
        let (day_start_equity, day_start_epoch_day) = if persisted.day_start_epoch_day == 0 {
            (persisted.equity, today_epoch_day())
        } else {
            (persisted.day_start_equity, persisted.day_start_epoch_day)
        };
        let (week_start_equity, week_start_epoch_week) = if persisted.week_start_epoch_week == 0 {
            (persisted.equity, today_epoch_week())
        } else {
            (persisted.week_start_equity, persisted.week_start_epoch_week)
        };
        Self {
            equity: persisted.equity,
            protected_reserve: persisted.protected_reserve,
            infra_reserve: persisted.infra_reserve,
            peak_equity: persisted.peak_equity,
            day_start_equity,
            day_start_epoch_day,
            week_start_equity,
            week_start_epoch_week,
            total_drawdown_latched: persisted.total_drawdown_latched,
            total_cycles: persisted.total_cycles,
            wins: persisted.wins,
            losses: persisted.losses,
            rejections: persisted.rejections,
            strategy_scaling: Strategy::ALL
                .iter()
                .map(|&s| {
                    let scaling = match persisted.strategy_scaling.get(s.key()) {
                        Some(sc) => StrategyScaling {
                            leg_size: sc.leg_size,
                            cumulative_pnl: sc.cumulative_pnl,
                            peak_cumulative_pnl: sc.peak_cumulative_pnl,
                            trough_since_peak: sc.trough_since_peak,
                            cycles_since_scale: sc.cycles_since_scale,
                            recent_pnls: VecDeque::new(),
                        },
                        // Save de antes desta revisão (12/08/2026): não tem
                        // strategy_scaling nenhum ainda — usa o leg_size
                        // global antigo como semente pra todas, em vez de
                        // reiniciar em cfg.initial_leg_size (perderia
                        // escalonamento já conquistado).
                        None if persisted.leg_size > 0.0 => {
                            StrategyScaling::new(persisted.leg_size)
                        }
                        None => StrategyScaling::new(cfg.initial_leg_size),
                    };
                    (s, scaling)
                })
                .collect(),
            ..fresh
        }
    }

    /// Chamado depois de cada trade — o custo de um `write` a cada operação
    /// é desprezível no volume que este sistema opera, e garante que nunca
    /// perdemos mais que a última operação em caso de queda abrupta.
    pub fn save(&self, path: &str) {
        let persisted = PersistedState {
            equity: self.equity,
            protected_reserve: self.protected_reserve,
            infra_reserve: self.infra_reserve,
            peak_equity: self.peak_equity,
            day_start_equity: self.day_start_equity,
            day_start_epoch_day: self.day_start_epoch_day,
            week_start_equity: self.week_start_equity,
            week_start_epoch_week: self.week_start_epoch_week,
            total_drawdown_latched: self.total_drawdown_latched,
            // Eco do maior leg_size entre estratégias — só pra servir de
            // semente em `load_or_new` se um save mais antigo (ver `None`
            // acima) precisar dele; a fonte da verdade é sempre
            // `strategy_scaling` abaixo.
            leg_size: self
                .strategy_scaling
                .values()
                .map(|s| s.leg_size)
                .fold(0.0, f64::max),
            total_cycles: self.total_cycles,
            strategy_scaling: self
                .strategy_scaling
                .iter()
                .map(|(s, sc)| {
                    (
                        s.key().to_string(),
                        StrategyScalingPersisted {
                            leg_size: sc.leg_size,
                            cumulative_pnl: sc.cumulative_pnl,
                            peak_cumulative_pnl: sc.peak_cumulative_pnl,
                            trough_since_peak: sc.trough_since_peak,
                            cycles_since_scale: sc.cycles_since_scale,
                        },
                    )
                })
                .collect(),
            wins: self.wins,
            losses: self.losses,
            rejections: self.rejections,
        };
        let Ok(json) = serde_json::to_string_pretty(&persisted) else { return };
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(path, json) {
            tracing::warn!(path, error = %e, "falha ao salvar estado do portfólio em disco");
        }
    }

    fn total_exposure(&self) -> f64 {
        self.exposure_by_strategy.values().sum()
    }

    pub fn drawdown_pct(&self) -> f64 {
        if self.peak_equity <= 0.0 {
            return 0.0;
        }
        ((self.peak_equity - self.equity) / self.peak_equity).max(0.0)
    }

    pub fn daily_drawdown_pct(&self) -> f64 {
        if self.day_start_equity <= 0.0 {
            return 0.0;
        }
        ((self.day_start_equity - self.equity) / self.day_start_equity).max(0.0)
    }

    pub fn weekly_drawdown_pct(&self) -> f64 {
        if self.week_start_equity <= 0.0 {
            return 0.0;
        }
        ((self.week_start_equity - self.equity) / self.week_start_equity).max(0.0)
    }
}

fn today_epoch_day() -> i64 {
    use chrono::Datelike;
    let today = chrono::Utc::now().date_naive();
    today.year() as i64 * 1000 + today.ordinal() as i64
}

/// Semana ISO (year*100+semana) — janela de 7 dias corridos alinhada ao
/// calendário, o suficiente pra pegar drawdown que se acumula devagar ao
/// longo de vários dias sem nenhum isoladamente bater o limite diário.
fn today_epoch_week() -> i64 {
    use chrono::Datelike;
    let today = chrono::Utc::now().date_naive();
    let iso = today.iso_week();
    iso.year() as i64 * 100 + iso.week() as i64
}

/// Resultado do kill-switch em camadas: `halt` (se `Some`, novas execuções
/// param) e `warn` (aviso não-bloqueante — dashboard mostra, sistema
/// continua operando normalmente).
pub struct KillSwitchStatus {
    pub halt: Option<String>,
    pub warn: Option<String>,
}

/// Kill-switch em camadas (Fase 12 do roadmap; revisado após auditoria
/// técnica externa de 13/08/2026 — 5% diário único era alto demais pra
/// microcapital). Checado a cada tick, ANTES de escolher/executar qualquer
/// oportunidade nova:
/// 1. Manual — existência do arquivo em `kill_file_path`.
/// 2. Preventivo (`daily_warn_pct`) — só aviso, não para o sistema.
/// 3. Diário rígido (`daily_halt_pct`) — para até o dia seguinte (UTC) ou
///    destravamento manual.
/// 4. Semanal (`weekly_halt_pct`) — mesma lógica, janela de 7 dias.
/// 5. Drawdown total desde o pico histórico (`total_drawdown_halt_pct`) —
///    rede de segurança final, independente de dia/semana.
///
/// Nunca desfaz posições já abertas nem mexe em equity — só impede NOVAS
/// ordens. Isso é suficiente porque o modelo de execução atual é 100%
/// round-trip instantâneo (nunca há posição aberta entre um ciclo e outro
/// — ver Seção 3 do roadmap); se/quando o sistema passar a manter posições
/// reais abertas, este kill-switch precisa ganhar lógica de
/// neutralizar/fechar posição, não só bloquear ordem nova.
pub fn kill_switch_reason(portfolio: &mut PortfolioState, cfg: &RiskConfig, kill_file_path: &str, reset_drawdown_file_path: &str) -> KillSwitchStatus {
    let today = today_epoch_day();
    if portfolio.day_start_epoch_day != today {
        portfolio.day_start_epoch_day = today;
        portfolio.day_start_equity = portfolio.equity;
    }
    let this_week = today_epoch_week();
    if portfolio.week_start_epoch_week != this_week {
        portfolio.week_start_epoch_week = this_week;
        portfolio.week_start_equity = portfolio.equity;
    }

    if std::path::Path::new(kill_file_path).exists() {
        return KillSwitchStatus { halt: Some(format!("kill-switch manual ativo ({kill_file_path} existe)")), warn: None };
    }

    // Nível mais grave (drawdown total desde o pico) é "trava", não
    // "termômetro" — uma vez acionado, fica bloqueado mesmo que o equity
    // suba de volta, até confirmação manual explícita. A presença do
    // arquivo de reset é consumida (removida) na hora — é uma confirmação
    // de uma vez, não um interruptor permanente.
    if portfolio.total_drawdown_latched {
        if std::path::Path::new(reset_drawdown_file_path).exists() {
            let _ = std::fs::remove_file(reset_drawdown_file_path);
            portfolio.total_drawdown_latched = false;
            tracing::warn!("drawdown total: trava removida manualmente, retomando avaliação normal");
        } else {
            return KillSwitchStatus {
                halt: Some(format!(
                    "drawdown total travado (nível mais grave) — crie {reset_drawdown_file_path} pra confirmar e destravar manualmente"
                )),
                warn: None,
            };
        }
    }

    let total_dd = portfolio.drawdown_pct();
    if total_dd >= cfg.total_drawdown_halt_pct {
        portfolio.total_drawdown_latched = true;
        return KillSwitchStatus {
            halt: Some(format!("drawdown total de {:.2}% (desde o pico historico) >= limite de {:.2}% — travado ate reset manual", total_dd * 100.0, cfg.total_drawdown_halt_pct * 100.0)),
            warn: None,
        };
    }

    let weekly_dd = portfolio.weekly_drawdown_pct();
    if weekly_dd >= cfg.weekly_halt_pct {
        return KillSwitchStatus {
            halt: Some(format!("drawdown semanal de {:.2}% >= limite de {:.2}%", weekly_dd * 100.0, cfg.weekly_halt_pct * 100.0)),
            warn: None,
        };
    }

    let daily_dd = portfolio.daily_drawdown_pct();
    if daily_dd >= cfg.daily_halt_pct {
        return KillSwitchStatus {
            halt: Some(format!("drawdown diario de {:.2}% >= limite de {:.2}%", daily_dd * 100.0, cfg.daily_halt_pct * 100.0)),
            warn: None,
        };
    }
    if daily_dd >= cfg.daily_warn_pct {
        return KillSwitchStatus {
            halt: None,
            warn: Some(format!("drawdown diario de {:.2}% acima do aviso preventivo de {:.2}% — sistema continua operando", daily_dd * 100.0, cfg.daily_warn_pct * 100.0)),
        };
    }

    KillSwitchStatus { halt: None, warn: None }
}

#[derive(Debug, Clone)]
pub struct Approved {
    pub order_size: f64,
    pub capital_at_risk: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    Expired,
    StrategyLimitExceeded,
    TotalLimitExceeded,
    CorrelationGroupBusy,
    NoLeverageOnLaunch,
    LeverageTooHigh,
    MartingaleBlocked,
}

/// Avalia uma oportunidade contra o estado atual do portfólio e os limites
/// de risco configurados. Não decide execução por si só — apenas diz se a
/// oportunidade PODE ser executada e com que tamanho.
pub fn evaluate(
    opp: &Opportunity,
    portfolio: &PortfolioState,
    cfg: &RiskConfig,
) -> Result<Approved, RejectReason> {
    if opp.is_expired() {
        return Err(RejectReason::Expired);
    }

    if opp.strategy == Strategy::Launch && opp.leverage > cfg.leverage.launch_max {
        return Err(RejectReason::NoLeverageOnLaunch);
    }
    if opp.leverage > cfg.leverage.liquid_max {
        return Err(RejectReason::LeverageTooHigh);
    }

    // Teto de drawdown (auditoria externa, 12/08/2026 — ver
    // drawdown_ceiling_multiplier): aplicado aqui, no tamanho EFETIVO da
    // ordem, nunca mutando o leg_size armazenado — a estrategia continua
    // livre pra escalonar via Kelly/recuperacao parcial (Secao 9.1) mesmo
    // com o portfolio em drawdown; so o que ela pode USAR agora fica
    // temporariamente menor.
    //
    // Kelly hierárquico (auditoria externa, 12/08/2026): dentro do
    // orçamento que a estratégia já ganhou, o símbolo específico desta
    // oportunidade recebe mais ou menos conforme seu próprio Kelly medido
    // — ver symbol_allocation_fraction. Símbolo novo/pouco visto herda
    // 100% do orçamento da estratégia (fração 1.0), nunca começa zerado.
    let symbol_fraction = symbol_allocation_fraction(
        portfolio.symbol_scaling.get(&(opp.strategy, opp.base_symbol().to_string())),
        portfolio.strategy_scaling.get(&opp.strategy),
    );
    let leg_size = portfolio.leg_size(opp.strategy)
        * drawdown_ceiling_multiplier(portfolio.drawdown_pct())
        * symbol_fraction;
    let order_size = opp.capital_needed.min(leg_size);

    // Nunca aumentar o tamanho da ordem em uma estratégia logo após uma
    // perda — mas comparado contra a perna BASE da estratégia
    // (`portfolio.leg_size`), não contra o tamanho exato do último trade
    // individual.
    //
    // Bug real encontrado ao vivo (13/08/2026, achado pelo usuário: "parou
    // de ter trades desde 23 horas"): comparar contra o último order_size
    // realizado travava a estratégia PARA SEMPRE. `order_size` varia por
    // símbolo (Kelly hierárquico, task #101 — `symbol_fraction` vai de
    // 0.10 a 2.00), então era pura sorte se o próximo candidato calhava de
    // ter fração menor que o símbolo do trade perdido. Order Flow, com
    // ~200+ símbolos ativos, quase sempre tirava um símbolo com fração
    // maior logo em seguida — bloqueado. E como só um trade BEM-SUCEDIDO
    // atualiza `last_outcome_by_strategy`/`last_order_size_by_strategy`,
    // e nenhum trade conseguia passar, virava um deadlock permanente sem
    // nenhum erro, sem nenhum log (a rejeição em si só era logada — e só
    // em debug! — se o candidato já tivesse passado por AQUI; nada
    // indicava no nível info que zero trades estavam acontecendo).
    // Comparar contra a perna base (estável, só muda por escalonamento já
    // gated por profit factor) preserva a intenção original — não deixar
    // a estratégia se auto-escalar pra cima logo após perder — sem travar
    // em ruído de variação por símbolo.
    if portfolio.last_outcome_by_strategy.get(&opp.strategy) == Some(&TradeOutcome::Loss) {
        let base_leg_size = portfolio.leg_size(opp.strategy);
        if order_size > base_leg_size {
            return Err(RejectReason::MartingaleBlocked);
        }
    }

    let capital_at_risk = order_size * opp.max_loss_pct;

    let strategy_limit = cfg.strategy_limit_pct(opp.strategy) * portfolio.equity;
    let current_strategy_exposure = *portfolio
        .exposure_by_strategy
        .get(&opp.strategy)
        .unwrap_or(&0.0);
    if current_strategy_exposure + capital_at_risk > strategy_limit {
        return Err(RejectReason::StrategyLimitExceeded);
    }

    let total_limit = cfg.total_risk_pct * portfolio.equity;
    if portfolio.total_exposure() + capital_at_risk > total_limit {
        return Err(RejectReason::TotalLimitExceeded);
    }

    let group_exposure = *portfolio
        .exposure_by_group
        .get(&opp.correlation_group)
        .unwrap_or(&0.0);
    if group_exposure > 0.0 {
        return Err(RejectReason::CorrelationGroupBusy);
    }

    Ok(Approved {
        order_size,
        capital_at_risk,
    })
}

/// Registra o resultado (simulado) de uma ordem aprovada e atualiza o estado
/// do portfólio: equity, reserva protegida, exposição liberada e contadores
/// usados pela lógica de escalonamento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleDirection {
    Increase,
    Decrease,
}

#[derive(Debug, Clone, Copy)]
pub struct ScaleEvent {
    /// `None` = ajuste portfolio-wide (redução de segurança por drawdown
    /// total), aplicado a todas as estratégias de uma vez — ver
    /// `halve_all_legs`. `Some` = ajuste de uma estratégia específica
    /// (aumento por Kelly fracionário, task #99).
    pub strategy: Option<Strategy>,
    pub old_size: f64,
    pub new_size: f64,
    pub direction: ScaleDirection,
}

pub fn record_trade_result(
    portfolio: &mut PortfolioState,
    cfg: &RiskConfig,
    opp: &Opportunity,
    approved: &Approved,
    outcome: TradeOutcome,
    pnl: f64,
) -> Vec<ScaleEvent> {
    portfolio.equity += pnl;
    if pnl > 0.0 {
        // Divisão de lucro depende do tipo de estratégia — ver
        // Strategy::is_rare_event. Em ambos os casos a reserva é
        // efetivamente MOVIDA pra fora do equity operacional (por isso o
        // -= aqui), não só somada num contador paralelo — senão o mesmo
        // dólar contaria duas vezes: uma como equity, outra como reserva,
        // inflando patrimônio total e os limites de risco (que são
        // calculados em cima de `equity`).
        let (reserve_pct, infra_pct) = if opp.strategy.is_rare_event() {
            (cfg.rare_event_reserve_pct, cfg.rare_event_infra_pct)
        } else {
            (cfg.protected_reserve_pct, 0.0)
        };
        let to_reserve = pnl * reserve_pct;
        let to_infra = pnl * infra_pct;
        portfolio.protected_reserve += to_reserve;
        portfolio.infra_reserve += to_infra;
        portfolio.equity -= to_reserve + to_infra;
        portfolio.wins += 1;
    } else {
        portfolio.losses += 1;
    }

    // Posições neste modelo de paper trading são "round-trip" imediatas
    // (o módulo real manteria a posição aberta até o fechamento real);
    // aqui liberamos a exposição assim que o resultado é conhecido.
    portfolio
        .exposure_by_strategy
        .insert(opp.strategy, 0.0);
    portfolio
        .exposure_by_group
        .insert(opp.correlation_group.clone(), 0.0);

    portfolio
        .last_outcome_by_strategy
        .insert(opp.strategy, outcome);
    portfolio
        .last_order_size_by_strategy
        .insert(opp.strategy, approved.order_size);

    portfolio.total_cycles += 1;

    // Bookkeeping por estratégia (task #97): PnL acumulado, pico e vale
    // desde o pico (usados pela recuperação parcial, task #98) e a janela
    // móvel de PnLs recentes DESSA estratégia (profit factor/robustez já
    // não são mais compartilhados entre estratégias diferentes — uma
    // estratégia ruim não trava mais o escalonamento de uma boa, e
    // vice-versa).
    let scaling = portfolio
        .strategy_scaling
        .entry(opp.strategy)
        .or_insert_with(|| StrategyScaling::new(cfg.initial_leg_size));
    scaling.cumulative_pnl += pnl;
    if scaling.cumulative_pnl > scaling.peak_cumulative_pnl {
        scaling.peak_cumulative_pnl = scaling.cumulative_pnl;
        scaling.trough_since_peak = scaling.cumulative_pnl;
    } else {
        scaling.trough_since_peak = scaling.trough_since_peak.min(scaling.cumulative_pnl);
    }
    scaling.cycles_since_scale += 1;

    const RECENT_PNLS_CAP: usize = 200;
    scaling.recent_pnls.push_back(pnl);
    while scaling.recent_pnls.len() > RECENT_PNLS_CAP {
        scaling.recent_pnls.pop_front();
    }

    // Kelly hierárquico (auditoria externa, 12/08/2026): mesmo bookkeeping,
    // agora também por (estratégia, símbolo) — alimenta
    // `symbol_allocation_fraction`. Janela mais curta que a da estratégia
    // (100 vs. 200): amostra por símbolo é sempre menor, não faz sentido
    // guardar uma janela do mesmo tamanho pra algo que refila mais devagar.
    const SYMBOL_RECENT_PNLS_CAP: usize = 100;
    let symbol_scaling = portfolio
        .symbol_scaling
        .entry((opp.strategy, opp.base_symbol().to_string()))
        .or_default();
    symbol_scaling.recent_pnls.push_back(pnl);
    while symbol_scaling.recent_pnls.len() > SYMBOL_RECENT_PNLS_CAP {
        symbol_scaling.recent_pnls.pop_front();
    }

    maybe_scale(portfolio, cfg, opp.strategy)
}

/// Profit factor (soma dos ganhos / soma das perdas) da janela móvel
/// recente — `None` se não há perdas registradas ainda (profit factor
/// indefinido) ou a amostra é pequena demais pra dizer algo.
fn recent_profit_factor(recent_pnls: &VecDeque<f64>, min_samples: usize) -> Option<f64> {
    if recent_pnls.len() < min_samples {
        return None;
    }
    let gains: f64 = recent_pnls.iter().filter(|&&p| p > 0.0).sum();
    let losses: f64 = recent_pnls.iter().filter(|&&p| p < 0.0).map(|p| p.abs()).sum();
    if losses <= 0.0 {
        return if gains > 0.0 { Some(f64::INFINITY) } else { None };
    }
    Some(gains / losses)
}

/// Robustez a outlier (revisão técnica externa, 13/08/2026): profit factor
/// sozinho pode ser carregado por uma única operação excepcional — remove
/// o melhor resultado da janela e confere se o saldo ainda seria positivo.
/// `None` se a amostra ainda é pequena demais pra dizer algo (mesmo piso
/// de `recent_profit_factor`).
fn positive_excluding_best_trade(recent_pnls: &VecDeque<f64>, min_samples: usize) -> Option<bool> {
    if recent_pnls.len() < min_samples {
        return None;
    }
    let best = recent_pnls.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let total: f64 = recent_pnls.iter().sum();
    Some(total - best.max(0.0) > 0.0)
}

/// Reduz a perna de TODAS as estratégias pela metade — usado só pelo aviso
/// preventivo de drawdown DIÁRIO (transição única, guardada em
/// orchestrator.rs por `is_warned != was_warned`, nunca chamada a cada
/// trade). O gatilho por drawdown desde o pico histórico usava isto
/// também até 12/08/2026, mas era chamado a cada trade enquanto o
/// drawdown ficasse acima do limiar — reduzindo repetidas vezes até o
/// piso; substituído por `drawdown_ceiling_multiplier` (não-destrutivo,
/// função pura do drawdown atual, ver `evaluate()`).
pub fn halve_all_legs(portfolio: &mut PortfolioState) -> Vec<ScaleEvent> {
    let mut events = Vec::new();
    for (&strategy, scaling) in portfolio.strategy_scaling.iter_mut() {
        let old_size = scaling.leg_size;
        let new_size = (old_size / 2.0).max(1.0);
        scaling.cycles_since_scale = 0;
        if new_size < old_size {
            scaling.leg_size = new_size;
            events.push(ScaleEvent {
                strategy: Some(strategy),
                old_size,
                new_size,
                direction: ScaleDirection::Decrease,
            });
        }
    }
    events
}

// Escada de recuperação de drawdown (auditoria externa, 12/08/2026).
// Valores fixos por enquanto (não expostos em risk.toml ainda) — degraus
// absolutos de drawdown desde o pico do portfólio, não relativos a
// nenhum outro parâmetro.
const DRAWDOWN_CEILING_TIER_1: f64 = 0.0175; // acima disto: metade
const DRAWDOWN_CEILING_TIER_2: f64 = 0.015; // abaixo de 1,75%: 65%
const DRAWDOWN_CEILING_TIER_3: f64 = 0.01; // abaixo de 1,50%: 80%
// abaixo de 1%: sem teto (100%)

/// Teto NÃO-DESTRUTIVO sobre o tamanho efetivo da ordem, em função do
/// drawdown ATUAL do portfólio — substitui a antiga mutação repetida de
/// `leg_size` via `halve_all_legs` a cada trade. Por ser uma função pura
/// recalculada do zero a cada chamada (nunca acumula, nunca precisa de
/// "já reduzi essa vez?"), relaxa sozinha assim que o portfólio recupera,
/// sem exigir lógica de recuperação separada nem estado extra.
fn drawdown_ceiling_multiplier(drawdown: f64) -> f64 {
    if drawdown >= DRAWDOWN_CEILING_TIER_1 {
        0.50
    } else if drawdown >= DRAWDOWN_CEILING_TIER_2 {
        0.65
    } else if drawdown >= DRAWDOWN_CEILING_TIER_3 {
        0.80
    } else {
        1.0
    }
}

/// Fração de Kelly cheio (`p - (1-p)/b`, `p`=taxa de acerto, `b`=ganho
/// médio/perda média) medida numa janela de PnLs — usada tanto pro
/// tamanho da perna por estratégia (`kelly_target_size`) quanto pro
/// Kelly hierárquico por símbolo (`symbol_allocation_fraction`). `None`
/// quando não há amostra dos dois lados (só vitórias ou só perdas na
/// janela) ou quando o Kelly medido é <= 0 — a própria matemática dizendo
/// "sem vantagem aqui ainda", não um caso de fallback silencioso.
fn kelly_fraction_from_pnls(recent_pnls: &VecDeque<f64>) -> Option<f64> {
    let wins: Vec<f64> = recent_pnls.iter().copied().filter(|&p| p > 0.0).collect();
    let losses: Vec<f64> = recent_pnls.iter().copied().filter(|&p| p < 0.0).map(f64::abs).collect();
    if wins.is_empty() || losses.is_empty() {
        return None;
    }
    let total = recent_pnls.len() as f64;
    let win_rate = wins.len() as f64 / total;
    let avg_win = wins.iter().sum::<f64>() / wins.len() as f64;
    let avg_loss = losses.iter().sum::<f64>() / losses.len() as f64;
    if avg_loss <= 0.0 {
        return None;
    }
    let payoff_ratio = avg_win / avg_loss;
    let kelly_fraction = win_rate - (1.0 - win_rate) / payoff_ratio;
    (kelly_fraction > 0.0).then_some(kelly_fraction)
}

/// Sizing contínuo estilo Kelly fracionário (task #99): em vez do degrau
/// fixo antigo (`leg_size * scale_growth_factor`), o tamanho-alvo da perna é
/// recalculado a cada aumento a partir da fração de Kelly medida na janela
/// recente de trades DESSA estratégia. Aplica só uma fração de segurança do
/// Kelly cheio (`kelly_safety_fraction`) porque o Kelly cheio maximiza
/// crescimento assumindo p/b exatos e conhecidos — nunca o caso com amostra
/// finita e ruidosa; Kelly fracionário é a forma padrão de manter a maior
/// parte do crescimento perdendo bem menos robustez.
fn kelly_target_size(scaling: &StrategyScaling, cfg: &RiskConfig, equity: f64) -> Option<f64> {
    let kelly_fraction = kelly_fraction_from_pnls(&scaling.recent_pnls)?;
    Some(equity * kelly_fraction * cfg.scaling.kelly_safety_fraction)
}

// Kelly hierárquico (auditoria externa, 12/08/2026): quantas amostras
// PRÓPRIAS de um símbolo até confiar 100% no Kelly dele em vez do prior da
// estratégia inteira — pesa linearmente entre os dois até lá.
const MIN_SYMBOL_SAMPLES_FOR_FULL_TRUST: usize = 50;
// Nunca zera nem infla um símbolo além disso, mesmo com Kelly bem
// destoante da estratégia — amostra por símbolo é sempre menor e mais
// ruidosa que a da estratégia inteira, então o intervalo é mais apertado
// que os 0-100% que o próprio Kelly fracionário já aplica em cima.
const SYMBOL_ALLOCATION_MIN: f64 = 0.10;
const SYMBOL_ALLOCATION_MAX: f64 = 2.00;

/// Fração do orçamento de risco da ESTRATÉGIA que vai pra ESTE símbolo
/// específico — a camada "símbolo" do Kelly hierárquico
/// (portfólio→estratégia→símbolo). Pedido do usuário via auditoria
/// externa: "Order Flow recebe X% de orçamento de risco; QTUM recebe 55%
/// desse orçamento; GRT recebe 25%". Símbolo nunca visto ou com pouca
/// amostra herda o prior da estratégia (fração 1.0 = "trata igual à
/// média da estratégia até provar diferente"); conforme acumula PnL
/// próprio, a fração passa a refletir o Kelly DESSE símbolo relativo ao
/// da estratégia — símbolos melhores que a média recebem mais, piores
/// recebem menos, nunca zero (mantém alguma amostra fluindo pra eles
/// continuarem sendo avaliados).
fn symbol_allocation_fraction(
    symbol_scaling: Option<&SymbolScaling>,
    strategy_scaling: Option<&StrategyScaling>,
) -> f64 {
    let Some(sym) = symbol_scaling else { return 1.0 };
    let n = sym.recent_pnls.len();
    if n == 0 {
        return 1.0;
    }
    let Some(symbol_kelly) = kelly_fraction_from_pnls(&sym.recent_pnls) else {
        return 1.0;
    };
    let Some(strategy_kelly) = strategy_scaling.and_then(|s| kelly_fraction_from_pnls(&s.recent_pnls)) else {
        return 1.0;
    };
    if strategy_kelly <= 0.0 {
        return 1.0; // sem baseline confiável da estratégia — não penaliza nem infla
    }
    let weight = (n as f64 / MIN_SYMBOL_SAMPLES_FOR_FULL_TRUST as f64).min(1.0);
    let blended_kelly = weight * symbol_kelly + (1.0 - weight) * strategy_kelly;
    (blended_kelly / strategy_kelly).clamp(SYMBOL_ALLOCATION_MIN, SYMBOL_ALLOCATION_MAX)
}

/// Decide se a perna operacional de UMA estratégia pode crescer. Gate de
/// segurança portfolio-wide primeiro (drawdown desde o pico do equity
/// inteiro — se estourou, reduz TUDO e nem avalia aumento). Senão, avalia
/// só o histórico dessa estratégia: recuperação parcial do próprio
/// drawdown local (task #98, substitui o antigo `at_new_peak` de equity
/// inteiro), ciclos mínimos, profit factor e robustez-sem-melhor-trade na
/// própria janela recente — e, se tudo passar, tamanha pelo Kelly
/// fracionário medido (task #99), com fallback pro degrau fixo antigo só
/// quando a amostra ainda não dá pra estimar Kelly (ex.: só vitórias até
/// agora).
fn maybe_scale(portfolio: &mut PortfolioState, cfg: &RiskConfig, strategy: Strategy) -> Vec<ScaleEvent> {
    portfolio.peak_equity = portfolio.peak_equity.max(portfolio.equity);
    let drawdown = portfolio.drawdown_pct();

    // Redução destrutiva por drawdown removida daqui (auditoria externa,
    // 12/08/2026): antes, chamava halve_all_legs a cada trade enquanto
    // drawdown >= limiar, o que reduzia a perna pela metade REPETIDAMENTE —
    // não uma vez, a cada ciclo — até colapsar no piso de US$1 e nunca
    // recuperar sozinho (uma única redução real levaria US$25 pra
    // US$12,50, não pra US$1; a repetição é que colapsava tudo). A
    // proteção contra drawdown agora é um teto não-destrutivo aplicado no
    // tamanho EFETIVO da ordem (ver drawdown_ceiling_multiplier, usado em
    // evaluate()) — o leg_size armazenado nunca é mutado por drawdown, só
    // pelo escalonamento por Kelly/recuperação parcial abaixo. O gate
    // `low_drawdown` logo adiante já impede escalonar em cima de um
    // drawdown elevado, então nenhuma proteção foi perdida.

    let equity = portfolio.equity;
    let Some(scaling) = portfolio.strategy_scaling.get_mut(&strategy) else {
        return Vec::new();
    };

    let enough_cycles = scaling.cycles_since_scale >= cfg.scaling.min_cycles_between_scale;
    let low_drawdown = drawdown < cfg.scaling.max_drawdown_pct_for_scale;

    // Recuperação parcial (task #98): em vez de exigir `cumulative_pnl` no
    // pico exato — o antigo `at_new_peak` comparava o EQUITY DO PORTFÓLIO
    // inteiro, travando uma estratégia individualmente lucrativa só porque
    // outra ainda não tinha recuperado — basta recuperar
    // `partial_recovery_fraction` do maior drawdown LOCAL (pico→vale de
    // PnL acumulado) já sofrido por essa estratégia especificamente.
    let peak_to_trough = scaling.peak_cumulative_pnl - scaling.trough_since_peak;
    let recovered = if peak_to_trough <= 0.0 {
        true
    } else {
        let recovered_fraction = 1.0 - (scaling.peak_cumulative_pnl - scaling.cumulative_pnl) / peak_to_trough;
        recovered_fraction >= cfg.scaling.partial_recovery_fraction
    };

    let profit_factor = recent_profit_factor(&scaling.recent_pnls, cfg.scaling.min_recent_samples_for_scale);
    let profit_factor_ok = profit_factor.map(|pf| pf >= cfg.scaling.min_recent_profit_factor_for_scale).unwrap_or(false);
    let robust_without_best = positive_excluding_best_trade(&scaling.recent_pnls, cfg.scaling.min_recent_samples_for_scale).unwrap_or(false);

    if !(recovered && enough_cycles && low_drawdown && profit_factor_ok && robust_without_best) {
        return Vec::new();
    }

    let old_size = scaling.leg_size;
    let max_leg = equity * cfg.scaling.max_leg_fraction_of_equity;
    let target_size = kelly_target_size(scaling, cfg, equity).unwrap_or(old_size * cfg.scaling.scale_growth_factor);
    let new_size = target_size.min(max_leg).max(1.0);
    scaling.cycles_since_scale = 0;

    if (new_size - old_size).abs() < 0.01 {
        return Vec::new();
    }

    tracing::info!(
        strategy = strategy.key(),
        old_leg_size = old_size,
        new_leg_size = new_size,
        equity,
        "condições de escalonamento atendidas: ajustando perna (Kelly fracionário)"
    );
    scaling.leg_size = new_size;
    vec![ScaleEvent {
        strategy: Some(strategy),
        old_size,
        new_size,
        direction: if new_size > old_size { ScaleDirection::Increase } else { ScaleDirection::Decrease },
    }]
}
