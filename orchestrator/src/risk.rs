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
    pub leg_size: f64,
    pub total_cycles: u64,
    pub cycles_since_scale: u32,
    pub exposure_by_strategy: HashMap<Strategy, f64>,
    pub exposure_by_group: HashMap<String, f64>,
    pub last_outcome_by_strategy: HashMap<Strategy, TradeOutcome>,
    pub last_order_size_by_strategy: HashMap<Strategy, f64>,
    pub wins: u64,
    pub losses: u64,
    pub rejections: u64,
    /// Janela móvel dos últimos PnLs (todas as estratégias) — usada pelo
    /// gate de escalonamento (revisão técnica externa, 13/08/2026: "exigir
    /// também EV móvel positivo, profit factor mínimo" antes de aumentar a
    /// perna, não só ciclos + drawdown baixo). Não persiste entre restarts
    /// de propósito — é um filtro de qualidade recente, não capital real;
    /// perdê-la só significa esperar a janela reencher, inofensivo.
    pub recent_pnls: VecDeque<f64>,
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
    leg_size: f64,
    total_cycles: u64,
    cycles_since_scale: u32,
    wins: u64,
    losses: u64,
    rejections: u64,
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
            leg_size: cfg.initial_leg_size,
            total_cycles: 0,
            cycles_since_scale: 0,
            exposure_by_strategy: HashMap::new(),
            exposure_by_group: HashMap::new(),
            last_outcome_by_strategy: HashMap::new(),
            last_order_size_by_strategy: HashMap::new(),
            wins: 0,
            losses: 0,
            rejections: 0,
            recent_pnls: VecDeque::new(),
        }
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
            leg_size: persisted.leg_size,
            total_cycles: persisted.total_cycles,
            cycles_since_scale: persisted.cycles_since_scale,
            wins: persisted.wins,
            losses: persisted.losses,
            rejections: persisted.rejections,
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
            leg_size: self.leg_size,
            total_cycles: self.total_cycles,
            cycles_since_scale: self.cycles_since_scale,
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

    let order_size = opp.capital_needed.min(portfolio.leg_size);

    // Nunca aumentar o tamanho da ordem em uma estratégia logo após uma perda.
    if portfolio.last_outcome_by_strategy.get(&opp.strategy) == Some(&TradeOutcome::Loss) {
        if let Some(&last_size) = portfolio.last_order_size_by_strategy.get(&opp.strategy) {
            if order_size > last_size {
                return Err(RejectReason::MartingaleBlocked);
            }
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
) -> Option<ScaleEvent> {
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
    portfolio.cycles_since_scale += 1;

    const RECENT_PNLS_CAP: usize = 200;
    portfolio.recent_pnls.push_back(pnl);
    while portfolio.recent_pnls.len() > RECENT_PNLS_CAP {
        portfolio.recent_pnls.pop_front();
    }

    maybe_scale(portfolio, cfg)
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

/// Ajusta o tamanho da perna operacional: reduz pela metade em drawdown
/// relevante, aumenta gradualmente quando a operação está em nova máxima,
/// estável há tempo suficiente e sem drawdown recente. Retorna o que mudou
/// para que o chamador possa anotar isso no dashboard (marcador no gráfico).
fn maybe_scale(portfolio: &mut PortfolioState, cfg: &RiskConfig) -> Option<ScaleEvent> {
    portfolio.peak_equity = portfolio.peak_equity.max(portfolio.equity);
    let drawdown = portfolio.drawdown_pct();

    if drawdown >= cfg.scaling.drawdown_halve_threshold_pct {
        let old_size = portfolio.leg_size;
        let new_size = (portfolio.leg_size / 2.0).max(1.0);
        portfolio.cycles_since_scale = 0;
        if new_size < old_size {
            tracing::warn!(
                drawdown_pct = drawdown * 100.0,
                old_leg_size = old_size,
                new_leg_size = new_size,
                "drawdown acima do limite: reduzindo perna pela metade"
            );
            portfolio.leg_size = new_size;
            return Some(ScaleEvent {
                old_size,
                new_size,
                direction: ScaleDirection::Decrease,
            });
        }
        return None;
    }

    let at_new_peak = portfolio.equity >= portfolio.peak_equity;
    let enough_cycles = portfolio.cycles_since_scale >= cfg.scaling.min_cycles_between_scale;
    let low_drawdown = drawdown < cfg.scaling.max_drawdown_pct_for_scale;
    // Ciclos + drawdown baixo sozinhos não provam que a estratégia está
    // lucrando de verdade nesse trecho recente — exige também profit
    // factor mínimo na janela móvel (None = amostra pequena demais ainda,
    // trata como "não comprovado", não escala).
    let profit_factor = recent_profit_factor(&portfolio.recent_pnls, cfg.scaling.min_recent_samples_for_scale);
    let profit_factor_ok = profit_factor.map(|pf| pf >= cfg.scaling.min_recent_profit_factor_for_scale).unwrap_or(false);
    // Não escala se o resultado da janela depende de uma única operação
    // excepcional — remove o melhor trade e exige que o saldo continue
    // positivo mesmo assim.
    let robust_without_best = positive_excluding_best_trade(&portfolio.recent_pnls, cfg.scaling.min_recent_samples_for_scale).unwrap_or(false);

    if at_new_peak && enough_cycles && low_drawdown && profit_factor_ok && robust_without_best {
        let old_size = portfolio.leg_size;
        let max_leg = portfolio.equity * cfg.scaling.max_leg_fraction_of_equity;
        let new_size = (portfolio.leg_size * cfg.scaling.scale_growth_factor).min(max_leg);
        portfolio.cycles_since_scale = 0;
        if new_size > old_size {
            tracing::info!(
                old_leg_size = old_size,
                new_leg_size = new_size,
                equity = portfolio.equity,
                "condições de escalonamento atendidas: aumentando perna"
            );
            portfolio.leg_size = new_size;
            return Some(ScaleEvent {
                old_size,
                new_size,
                direction: ScaleDirection::Increase,
            });
        }
    }
    None
}
