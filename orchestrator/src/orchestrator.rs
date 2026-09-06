use std::collections::{HashMap, HashSet};
use std::time::Instant;

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Duration;

use crate::events::{DashboardEvent, EventBus};
use crate::risk::{self, PortfolioState, RiskConfig, TradeOutcome};
use crate::types::{
    ExecutionBackend, ExecutionMode, ExecutionReport, ExecutionRequest, ExecutionStatus,
    Opportunity, Strategy,
};

const CONFLUENCE_COOLDOWN: Duration = Duration::from_secs(60);
/// Multiplicador de score quando 2+ estratégias de preço real concordam no
/// mesmo ativo na mesma janela. É uma heurística ("sinais independentes
/// concordando pesam mais"), não um número validado por backtest — só
/// prioriza entre oportunidades que já passaram no gate de edge positivo,
/// nunca torna operável algo que não teria vantagem sozinho.
const CONFLUENCE_SCORE_BONUS: f64 = 1.4;

/// Folga para receber a confirmação depois da latência prevista. Se ela não
/// chegar, a reserva é cancelada como unfilled e não vira um trade fictício.
const EXECUTION_REPORT_GRACE: Duration = Duration::from_secs(5);
const DEMO_EXECUTION_REPORT_GRACE: Duration = Duration::from_secs(20);
const OPEN_RISK_RECHECK: Duration = Duration::from_secs(60 * 60 * 24);

/// Concorrência por orçamento de risco (auditoria externa, 12/08/2026):
/// a tabela fixa antiga (1 operação até US$499, 2 de US$500-999, 3 de
/// US$1.000-2.499, 5 acima disso) limitava artificialmente o sistema
/// mesmo quando várias oportunidades pequenas e NÃO correlacionadas
/// cabiam dentro do orçamento de risco disponível — com US$200 e
/// universo de 220+220 símbolos, isso significava recusar a 2ª/3ª
/// oportunidade boa do mesmo ciclo só por causa de um número fixo, não
/// por falta de risco disponível.
///
/// O motor de risco já é o orçamento de risco real: `risk::evaluate`
/// (chamado a cada iteração dentro de `pick_best`, contra o estado JÁ
/// ATUALIZADO pela oportunidade anterior no mesmo ciclo) rejeita por
/// limite de estratégia, grupo de correlação e total simultâneo de
/// 0,50% — exatamente os três critérios que a auditoria pediu. Não há
/// mais teto artificial: o loop abaixo executa até esse orçamento se
/// esgotar sozinho (`pick_best` não acha mais nada aprovado) ou até este
/// teto de sanidade por tick, o que vier primeiro — ele existe só pra
/// nunca travar um único tick indefinidamente, não como limite de
/// negócio.
const MAX_TRADES_PER_TICK: usize = 50;

/// Uma posição aprovada, ainda não resolvida — vive entre `open_position`
/// (aprovação real, exposição contabilizada) e `resolve_position` (fim da
/// janela real de `expected_holding_secs`, resultado simulado, exposição
/// liberada). Achado ao vivo (13/08/2026, pergunta do usuário: "o que
/// falta pra ser 100% real?"): antes, essas duas coisas aconteciam no
/// mesmo instante — nenhuma posição jamais ficava genuinamente "aberta",
/// então `exposure_by_strategy`/`exposure_by_group` nunca acumulavam nada
/// pra valer, e os gates de limite de risco simultâneo em `risk::evaluate`
/// nunca tinham o que checar de verdade.
struct OpenPosition {
    opp: Opportunity,
    approved: risk::Approved,
    report_deadline: Instant,
}

/// Roda o loop central: acumula oportunidades chegando dos módulos, a cada
/// tick escolhe a melhor entre as ainda válidas e aprovadas pelo risk
/// engine, aguarda o resultado do backend selecionado e atualiza o portfólio.
/// Cada passo relevante é publicado no `EventBus` — o dashboard (e qualquer
/// outro consumidor futuro) lê dali, com histórico completo desde o boot.
/// Retorna o estado final para o relatório de resumo.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    mut rx: Receiver<Opportunity>,
    mut execution_rx: Receiver<ExecutionReport>,
    cfg: RiskConfig,
    max_cycles: u64,
    tick_every: Duration,
    bus: EventBus,
    state_path: String,
    kill_file_path: String,
    reset_drawdown_file_path: String,
    execution_backend: ExecutionBackend,
    execution_request_tx: Option<Sender<ExecutionRequest>>,
) -> PortfolioState {
    // Recupera equity/ciclos/wins/losses de onde parou — sem isso, todo
    // restart do processo (pra aplicar código novo, por exemplo) voltaria
    // pro equity inicial, o que destruiria qualquer continuidade real do
    // backend de execução.
    let mut portfolio = PortfolioState::load_or_new(&cfg, &state_path);

    // Filtros reais de lote/notional mínimo por símbolo, buscados uma vez
    // no boot. Símbolos ausentes falham fechado no risk engine.
    for (symbol, filter) in crate::exchange_filters::fetch_bybit_linear_filters().await {
        portfolio
            .symbol_filters
            .insert(format!("bybit:{symbol}"), filter);
    }
    // Materializa a sessão mesmo antes do primeiro trade. Assim um restart
    // durante a calibração não apaga o histórico de eventos por falta do
    // arquivo de estado inicial.
    portfolio.save(&state_path);

    // Sem isso, os tiles do dashboard ficam em "—" pra sempre até a
    // primeira operação executar — o que pode nunca acontecer se nenhuma
    // oportunidade tiver edge positivo. O estado inicial (equity de boot,
    // perna inicial, zero ciclos) é informação real e deve aparecer desde
    // o primeiro segundo, não só depois do primeiro trade.
    bus.emit(DashboardEvent::portfolio_snapshot(&portfolio));

    let mut pending: Vec<Opportunity> = Vec::new();
    let mut open_positions: Vec<OpenPosition> = Vec::new();
    let mut confluence_cooldown: HashMap<String, Instant> = HashMap::new();
    let mut reported_rejections: HashSet<u64> = HashSet::new();
    let mut execution_faults: HashSet<u64> = HashSet::new();
    let mut last_halt_reason: Option<String> = None;
    let mut was_warned = false;
    let mut execution_channel_open = true;

    let mut tick = tokio::time::interval(tick_every);

    loop {
        tokio::select! {
            maybe_opp = rx.recv() => {
                match maybe_opp {
                    Some(opp) => {
                        bus.emit(DashboardEvent::opportunity_received(&opp));
                        pending.push(opp);
                    }
                    None => {
                        tracing::info!("todas as fontes de sinal encerraram, finalizando");
                        break;
                    }
                }
            }
            maybe_report = execution_rx.recv(), if execution_channel_open => {
                match maybe_report {
                    Some(report) => {
                        settle_execution(
                            &mut portfolio,
                            &cfg,
                            report,
                            &bus,
                            &mut open_positions,
                            &mut execution_faults,
                        );
                        portfolio.save(&state_path);
                        if portfolio.total_cycles >= max_cycles {
                            tracing::info!(cycles = portfolio.total_cycles, "limite de ciclos atingido");
                            break;
                        }
                    }
                    None => execution_channel_open = false,
                }
            }
            _ = tick.tick() => {
                pending.retain(|o| !o.is_expired());
                // `reported_rejections` só precisa lembrar de sinais ainda
                // pendentes — sem podar junto com `pending` ele cresce um
                // `u64` por sinal rejeitado pelo resto da vida do processo,
                // nunca liberando memória mesmo depois do sinal expirar.
                if !pending.is_empty() {
                    let still_pending: HashSet<u64> =
                        pending.iter().map(|opp| opp.signal_id).collect();
                    reported_rejections.retain(|signal_id| still_pending.contains(signal_id));
                } else {
                    reported_rejections.clear();
                }

                // Sem relatório ligado ao signal_id não existe resultado.
                // Uma confirmação perdida apenas libera a reserva como
                // unfilled; não incrementa ciclos, wins, losses ou Kelly.
                let now = Instant::now();
                let mut still_open = Vec::with_capacity(open_positions.len());
                for mut pos in open_positions.drain(..) {
                    if pos.report_deadline > now {
                        still_open.push(pos);
                        continue;
                    }
                    if execution_backend == ExecutionBackend::BybitDemo {
                        let first_fault = execution_faults.insert(pos.opp.signal_id);
                        pos.report_deadline = now + OPEN_RISK_RECHECK;
                        if first_fault {
                            tracing::error!(signal_id = pos.opp.signal_id, asset = %pos.opp.asset, "execução Demo sem relatório; exposição preservada e novas ordens bloqueadas");
                            bus.emit(DashboardEvent::execution_open_risk(
                                pos.opp.signal_id,
                                pos.opp.strategy,
                                &pos.opp.asset,
                                pos.approved.order_size,
                                0,
                                "timeout_demo_requer_reconciliacao",
                            ));
                        }
                        still_open.push(pos);
                    } else {
                        risk::cancel_exposure(&mut portfolio, &pos.opp, &pos.approved);
                        tracing::warn!(signal_id = pos.opp.signal_id, asset = %pos.opp.asset, "execução shadow expirou sem relatório; posição cancelada como unfilled");
                        bus.emit(DashboardEvent::execution_unfilled(
                            pos.opp.signal_id,
                            pos.opp.strategy,
                            &pos.opp.asset,
                            "timeout_sem_relatorio",
                        ));
                    }
                    portfolio.save(&state_path);
                }
                open_positions = still_open;

                let confluence_bonus = detect_and_emit_confluence(&pending, &bus, &mut confluence_cooldown);

                let kill_status = risk::kill_switch_reason(&mut portfolio, &cfg, &kill_file_path, &reset_drawdown_file_path);
                let effective_halt = if execution_faults.is_empty() {
                    kill_status.halt.clone()
                } else {
                    Some(format!(
                        "risco de execução Demo aberto em {} operação(ões); reconciliação obrigatória",
                        execution_faults.len()
                    ))
                };
                let is_halted = effective_halt.is_some();
                if effective_halt != last_halt_reason {
                    last_halt_reason = effective_halt.clone();
                    if let Some(reason) = &effective_halt {
                        tracing::warn!(reason, "kill-switch acionado: pausando novas execucoes");
                    } else {
                        tracing::info!("kill-switch liberado: retomando execucoes normalmente");
                    }
                    bus.emit(DashboardEvent::system_halt(effective_halt));
                }
                let is_warned = kill_status.warn.is_some();
                if is_warned != was_warned {
                    was_warned = is_warned;
                    if let Some(msg) = &kill_status.warn {
                        tracing::warn!(msg, "aviso preventivo de drawdown");
                    }
                    bus.emit(DashboardEvent::risk_warning(kill_status.warn));
                }

                let slots = if is_halted { 0 } else { MAX_TRADES_PER_TICK };
                let mut done_this_tick = 0;

                // Um candidato pode ficar pendente por vários ticks porque
                // um limite transitório ainda está ocupado. Registra a
                // primeira rejeição de cada signal_id sem inflar o contador
                // a cada reavaliação.
                if !is_halted {
                    for opp in &pending {
                        if opp.execution_mode != ExecutionMode::ExecutableQuoted || opp.score() <= 0.0 {
                            continue;
                        }
                        if let Err(reason) = risk::evaluate(opp, &portfolio, &cfg) {
                            if reported_rejections.insert(opp.signal_id) {
                                portfolio.rejections += 1;
                                bus.emit(DashboardEvent::decision_rejected(
                                    opp.signal_id,
                                    opp.strategy,
                                    &opp.asset,
                                    format!("{reason:?}"),
                                ));
                            }
                        }
                    }
                }

                while done_this_tick < slots {
                    // Reavalia a cada iteração: depois de ABRIR uma posição,
                    // a exposição por estratégia/grupo mudou de verdade
                    // (fica contabilizada até a posição resolver, não mais
                    // liberada no mesmo instante), então a 2ª/3ª escolha
                    // respeita o risco já comprometido pela 1ª — nunca é "N
                    // ordens avaliadas contra o mesmo estado congelado".
                    let Some(idx) = pick_best(
                        &pending,
                        &portfolio,
                        &cfg,
                        &confluence_bonus,
                        &open_positions,
                    ) else {
                        break;
                    };
                    let opp = pending.remove(idx);
                    open_position(
                        &mut portfolio,
                        &cfg,
                        opp,
                        &bus,
                        &mut open_positions,
                        execution_backend,
                        execution_request_tx.as_ref(),
                    )
                    .await;
                    portfolio.save(&state_path);
                    done_this_tick += 1;
                }
            }
        }
    }

    portfolio
}

/// Agrupa as oportunidades ainda válidas por símbolo-base e, quando duas ou
/// mais estratégias diferentes concordam no mesmo ativo dentro da mesma
/// janela, emite um evento de confluência pro dashboard — a parte central
/// do orquestrador "se comunicando" que o roadmap descreve. Retorna um mapa
/// símbolo → multiplicador de score, usado só para desempatar prioridade
/// entre candidatos que já têm vantagem positiva sozinhos.
fn detect_and_emit_confluence(
    pending: &[Opportunity],
    bus: &EventBus,
    cooldown: &mut HashMap<String, Instant>,
) -> HashMap<String, f64> {
    let mut by_symbol: HashMap<&str, Vec<Strategy>> = HashMap::new();
    for opp in pending {
        let strategies = by_symbol.entry(opp.base_symbol()).or_default();
        if !strategies.contains(&opp.strategy) {
            strategies.push(opp.strategy);
        }
    }

    let mut bonuses = HashMap::new();
    for (symbol, mut strategies) in by_symbol {
        if strategies.len() < 2 {
            continue;
        }
        strategies.sort_by_key(|s| s.key());

        // Um sensor observacional pode aparecer na confluência do painel,
        // mas só estratégias com executor cotado podem alterar prioridade.
        let quoted_strategies: HashSet<Strategy> = pending
            .iter()
            .filter(|opp| {
                opp.base_symbol() == symbol && opp.execution_mode == ExecutionMode::ExecutableQuoted
            })
            .map(|opp| opp.strategy)
            .collect();
        let price_confirmed = quoted_strategies.len() >= 2;

        let keys: Vec<&'static str> = strategies.iter().map(|s| s.key()).collect();
        let cooldown_key = format!("{symbol}:{}", keys.join(","));
        let should_fire = cooldown
            .get(&cooldown_key)
            .map(|t| t.elapsed() >= CONFLUENCE_COOLDOWN)
            .unwrap_or(true);

        if should_fire {
            cooldown.insert(cooldown_key, Instant::now());
            if price_confirmed {
                tracing::info!(symbol, estrategias = ?keys, "confluência executável detectada");
            } else {
                tracing::debug!(symbol, estrategias = ?keys, "confluência apenas observacional");
            }
            bus.emit(DashboardEvent::confluence(symbol, keys, price_confirmed));
        }

        if price_confirmed {
            bonuses.insert(symbol.to_string(), CONFLUENCE_SCORE_BONUS);
        }
    }
    bonuses
}

/// Entre as oportunidades pendentes, aprova cada uma pelo motor de risco e
/// escolhe a de maior score (com bônus de confluência aplicado só pra
/// desempate de prioridade) dentre as aprovadas. Retorna o índice em
/// `pending`, ou `None` se nenhuma passar no risk gate.
fn pick_best(
    pending: &[Opportunity],
    portfolio: &PortfolioState,
    cfg: &RiskConfig,
    confluence_bonus: &HashMap<String, f64>,
    open_positions: &[OpenPosition],
) -> Option<usize> {
    pending
        .iter()
        .enumerate()
        // score > 0 exige net_edge > 0: nunca executar só porque passou no
        // risk gate — precisa também ter vantagem esperada positiva. O
        // bônus de confluência entra DEPOIS desse filtro, então nunca torna
        // operável algo que não teria vantagem sozinho.
        //
        // Log em debug! (silencioso em produção, RUST_LOG=orchestrator=info
        // por padrão) — sem isso, um candidato que nunca passa nesse filtro
        // não deixa rastro nenhum: `execute()` só loga rejeição pra quem já
        // passou aqui, então "tudo rejeitado silenciosamente" era invisível
        // (achado 13/08/2026 investigando trades parados por 5h+ sem erro).
        .filter(|(_, opp)| {
            if open_positions
                .iter()
                .any(|position| position.opp.base_symbol() == opp.base_symbol())
            {
                return false;
            }
            if opp.execution_mode != ExecutionMode::ExecutableQuoted || opp.score() <= 0.0 {
                return false;
            }
            match risk::evaluate(opp, portfolio, cfg) {
                Ok(_) => true,
                Err(reason) => {
                    tracing::debug!(?reason, strategy = opp.strategy.key(), asset = %opp.asset, "candidato excluído no pick_best");
                    false
                }
            }
        })
        .max_by(|(_, a), (_, b)| {
            let bonus_a = confluence_bonus.get(a.base_symbol()).copied().unwrap_or(1.0);
            let bonus_b = confluence_bonus.get(b.base_symbol()).copied().unwrap_or(1.0);
            (a.score() * bonus_a).partial_cmp(&(b.score() * bonus_b)).unwrap()
        })
        .map(|(idx, _)| idx)
}

/// Aprova formalmente a oportunidade vencedora (recalcula o `Approved` já
/// que `pick_best` só checou viabilidade) e ABRE a posição — diferente de
/// antes, não resolve o resultado agora. A exposição fica genuinamente
/// contabilizada (`risk::open_exposure`) e uma ficha do rate limiter da(s)
/// venue(s) é consumida (`risk::consume_rate_limit`) até `resolve_position`
/// fechar, quando a janela real de `opp.expected_holding_secs` terminar.
async fn open_position(
    portfolio: &mut PortfolioState,
    cfg: &RiskConfig,
    opp: Opportunity,
    bus: &EventBus,
    open_positions: &mut Vec<OpenPosition>,
    execution_backend: ExecutionBackend,
    execution_request_tx: Option<&Sender<ExecutionRequest>>,
) {
    let approved = match risk::evaluate(&opp, portfolio, cfg) {
        Ok(a) => a,
        Err(reason) => {
            portfolio.rejections += 1;
            tracing::debug!(?reason, asset = %opp.asset, "oportunidade rejeitada no gate final");
            bus.emit(DashboardEvent::decision_rejected(
                opp.signal_id,
                opp.strategy,
                &opp.asset,
                format!("{reason:?}"),
            ));
            bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
            return;
        }
    };

    bus.emit(DashboardEvent::decision_approved(
        opp.signal_id,
        opp.strategy,
        &opp.asset,
        approved.order_size,
    ));

    risk::open_exposure(portfolio, &opp, &approved);
    risk::consume_rate_limit(portfolio, opp.strategy);

    if execution_backend == ExecutionBackend::BybitDemo {
        let Some(sender) = execution_request_tx else {
            risk::cancel_exposure(portfolio, &opp, &approved);
            bus.emit(DashboardEvent::execution_unfilled(
                opp.signal_id,
                opp.strategy,
                &opp.asset,
                "executor_demo_indisponivel",
            ));
            return;
        };
        let request = ExecutionRequest {
            signal_id: opp.signal_id,
            symbol: opp.base_symbol().to_string(),
            direction: opp.direction,
            quantity: approved.order_qty,
            price_tick: approved.price_tick,
            stop_loss_pct: opp.max_loss_pct,
            expected_holding_secs: opp.expected_holding_secs,
        };
        if sender.send(request).await.is_err() {
            risk::cancel_exposure(portfolio, &opp, &approved);
            bus.emit(DashboardEvent::execution_unfilled(
                opp.signal_id,
                opp.strategy,
                &opp.asset,
                "canal_executor_demo_fechado",
            ));
            return;
        }
    }

    let report_grace = match execution_backend {
        ExecutionBackend::Shadow => EXECUTION_REPORT_GRACE,
        ExecutionBackend::BybitDemo => DEMO_EXECUTION_REPORT_GRACE,
    };
    let report_deadline = Instant::now()
        + Duration::from_secs_f64(opp.expected_holding_secs.max(0.05))
        + report_grace;
    open_positions.push(OpenPosition {
        opp,
        approved,
        report_deadline,
    });
}

/// Liquida exclusivamente pelo relatório que carrega o mesmo `signal_id`.
/// O notional preenchido vem da menor profundidade observada entre as duas
/// pernas; nenhum sorteio de win rate ou retorno histórico entra no PnL.
fn settle_execution(
    portfolio: &mut PortfolioState,
    cfg: &RiskConfig,
    report: ExecutionReport,
    bus: &EventBus,
    open_positions: &mut Vec<OpenPosition>,
    execution_faults: &mut HashSet<u64>,
) {
    let Some(idx) = open_positions
        .iter()
        .position(|p| p.opp.signal_id == report.signal_id)
    else {
        tracing::debug!(
            signal_id = report.signal_id,
            "relatório sem posição aprovada correspondente; ignorando"
        );
        return;
    };
    if report.status == ExecutionStatus::OpenRisk {
        let pos = &mut open_positions[idx];
        pos.report_deadline = Instant::now() + OPEN_RISK_RECHECK;
        execution_faults.insert(report.signal_id);
        tracing::error!(
            signal_id = report.signal_id,
            asset = %pos.opp.asset,
            execution_note = %report.note,
            "risco de execução aberto; exposição preservada e novas ordens bloqueadas"
        );
        bus.emit(DashboardEvent::execution_open_risk(
            pos.opp.signal_id,
            pos.opp.strategy,
            &pos.opp.asset,
            report.max_executable_notional,
            report.observed_latency_ms,
            &report.note,
        ));
        bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
        return;
    }

    let OpenPosition { opp, approved, .. } = open_positions.swap_remove(idx);
    execution_faults.remove(&report.signal_id);

    if report.status == ExecutionStatus::Unfilled {
        risk::cancel_exposure(portfolio, &opp, &approved);
        bus.emit(DashboardEvent::execution_unfilled(
            opp.signal_id,
            opp.strategy,
            &opp.asset,
            &report.note,
        ));
        bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
        return;
    }

    if !report.return_pct.is_finite()
        || report
            .realized_pnl
            .map(|pnl| !pnl.is_finite())
            .unwrap_or(false)
        || !report.max_executable_notional.is_finite()
        || report.max_executable_notional < 0.0
    {
        risk::cancel_exposure(portfolio, &opp, &approved);
        tracing::warn!(
            signal_id = opp.signal_id,
            "relatório de execução contém valores inválidos; cancelando como unfilled"
        );
        bus.emit(DashboardEvent::execution_unfilled(
            opp.signal_id,
            opp.strategy,
            &opp.asset,
            "relatorio_invalido",
        ));
        bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
        return;
    }

    let filled_size = approved
        .order_size
        .min(report.max_executable_notional.max(0.0));
    if filled_size <= 0.0 {
        risk::cancel_exposure(portfolio, &opp, &approved);
        bus.emit(DashboardEvent::execution_unfilled(
            opp.signal_id,
            opp.strategy,
            &opp.asset,
            &report.note,
        ));
        bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
        return;
    }

    let fill_fraction = (filled_size / approved.order_size).clamp(0.0, 1.0);
    let pnl = report
        .realized_pnl
        .unwrap_or(filled_size * report.return_pct);
    let won = pnl > 0.0;
    let outcome = if won {
        TradeOutcome::Win
    } else {
        TradeOutcome::Loss
    };

    if -report.return_pct > opp.max_loss_pct {
        tracing::warn!(
            signal_id = opp.signal_id,
            realized_loss_pct = -report.return_pct * 100.0,
            stress_loss_pct = opp.max_loss_pct * 100.0,
            "perda executada excedeu o cenário de stress usado no sizing"
        );
    }

    tracing::info!(
        signal_id = opp.signal_id,
        strategy = opp.strategy.key(),
        asset = %opp.asset,
        outcome = ?outcome,
        order_size = approved.order_size,
        filled_size,
        fill_fraction,
        observed_latency_ms = report.observed_latency_ms,
        execution_note = %report.note,
        pnl,
        equity_after = portfolio.equity + pnl,
        leg_size = portfolio.leg_size(opp.strategy),
        "execução liquidada"
    );

    let scale_events = risk::record_trade_result(portfolio, cfg, &opp, &approved, outcome, pnl);

    bus.emit(DashboardEvent::trade_result(
        opp.signal_id,
        opp.strategy,
        &opp.asset,
        won,
        pnl,
        portfolio.equity,
    ));
    bus.emit(DashboardEvent::execution_filled(
        opp.signal_id,
        opp.strategy,
        &opp.asset,
        filled_size,
        report.return_pct,
        report.observed_latency_ms,
        &report.note,
    ));
    for scale in scale_events {
        bus.emit(DashboardEvent::leg_resized(scale, portfolio.equity));
    }
    bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Direction, Market};

    fn config() -> RiskConfig {
        RiskConfig::load(concat!(env!("CARGO_MANIFEST_DIR"), "/config/risk.toml")).unwrap()
    }

    fn opportunity(signal_id: u64) -> Opportunity {
        Opportunity {
            signal_id,
            market: Market::Crypto,
            strategy: Strategy::OrderFlow,
            asset: "BTCUSDT".to_string(),
            direction: Direction::Long,
            net_edge: 0.001,
            confidence: 0.7,
            valid_for_ms: 10_000,
            expected_holding_secs: 5.0,
            capital_needed: 25.0,
            reference_price: Some(100.0),
            max_loss_pct: 0.005,
            leverage: 1.0,
            correlation_group: "BTCUSDT".to_string(),
            execution_mode: ExecutionMode::ExecutableQuoted,
            capital_multiplier: 1.0,
            emitted_at: Instant::now(),
        }
    }

    fn portfolio_with_btc_filter(cfg: &RiskConfig) -> PortfolioState {
        let mut portfolio = PortfolioState::new(cfg);
        portfolio.symbol_filters.insert(
            "bybit:BTCUSDT".to_string(),
            risk::SymbolFilter {
                min_qty: 0.001,
                qty_step: 0.001,
                min_notional: 5.0,
                price_tick: 0.1,
            },
        );
        portfolio
    }

    #[test]
    fn pnl_comes_only_from_report_with_matching_signal_id() {
        let cfg = config();
        let mut portfolio = portfolio_with_btc_filter(&cfg);
        let opp = opportunity(7);
        let approved = risk::evaluate(&opp, &portfolio, &cfg).unwrap();
        risk::open_exposure(&mut portfolio, &opp, &approved);
        let mut open = vec![OpenPosition {
            opp,
            approved,
            report_deadline: Instant::now() + Duration::from_secs(10),
        }];
        let bus = EventBus::new(32);
        let mut faults = HashSet::new();

        settle_execution(
            &mut portfolio,
            &cfg,
            ExecutionReport {
                signal_id: 999,
                status: ExecutionStatus::Filled,
                return_pct: 0.50,
                realized_pnl: None,
                max_executable_notional: 25.0,
                observed_latency_ms: 5_000,
                note: "mismatch".to_string(),
            },
            &bus,
            &mut open,
            &mut faults,
        );
        assert_eq!(portfolio.total_cycles, 0);
        assert_eq!(portfolio.equity, 200.0);

        settle_execution(
            &mut portfolio,
            &cfg,
            ExecutionReport {
                signal_id: 7,
                status: ExecutionStatus::Filled,
                return_pct: 0.001,
                realized_pnl: None,
                max_executable_notional: 25.0,
                observed_latency_ms: 5_000,
                note: "quote_real".to_string(),
            },
            &bus,
            &mut open,
            &mut faults,
        );
        assert_eq!(portfolio.total_cycles, 1);
        assert!((portfolio.equity + portfolio.protected_reserve - 200.025).abs() < 1e-9);
        assert_eq!(portfolio.total_exposure(), 0.0);
    }

    #[test]
    fn invalid_execution_report_is_unfilled_without_pnl() {
        let cfg = config();
        let mut portfolio = portfolio_with_btc_filter(&cfg);
        let opp = opportunity(8);
        let approved = risk::evaluate(&opp, &portfolio, &cfg).unwrap();
        risk::open_exposure(&mut portfolio, &opp, &approved);
        let mut open = vec![OpenPosition {
            opp,
            approved,
            report_deadline: Instant::now() + Duration::from_secs(10),
        }];
        let bus = EventBus::new(32);
        let mut faults = HashSet::new();

        settle_execution(
            &mut portfolio,
            &cfg,
            ExecutionReport {
                signal_id: 8,
                status: ExecutionStatus::Filled,
                return_pct: f64::NAN,
                realized_pnl: None,
                max_executable_notional: 25.0,
                observed_latency_ms: 5_000,
                note: "corrompido".to_string(),
            },
            &bus,
            &mut open,
            &mut faults,
        );

        assert_eq!(portfolio.total_cycles, 0);
        assert_eq!(portfolio.equity, 200.0);
        assert_eq!(portfolio.total_exposure(), 0.0);
        assert!(open.is_empty());
    }

    #[test]
    fn venue_realized_pnl_has_priority_over_reconstructed_return() {
        let cfg = config();
        let mut portfolio = portfolio_with_btc_filter(&cfg);
        let opp = opportunity(11);
        let approved = risk::evaluate(&opp, &portfolio, &cfg).unwrap();
        risk::open_exposure(&mut portfolio, &opp, &approved);
        let mut open = vec![OpenPosition {
            opp,
            approved,
            report_deadline: Instant::now() + Duration::from_secs(10),
        }];
        let bus = EventBus::new(32);
        let mut faults = HashSet::new();

        settle_execution(
            &mut portfolio,
            &cfg,
            ExecutionReport {
                signal_id: 11,
                status: ExecutionStatus::Filled,
                return_pct: 0.0,
                realized_pnl: Some(0.123),
                max_executable_notional: 25.1,
                observed_latency_ms: 20,
                note: "fills_demo".to_string(),
            },
            &bus,
            &mut open,
            &mut faults,
        );

        assert!((portfolio.equity + portfolio.protected_reserve - 200.123).abs() < 1e-9);
    }

    #[test]
    fn open_risk_preserves_exposure_and_position() {
        let cfg = config();
        let mut portfolio = portfolio_with_btc_filter(&cfg);
        let opp = opportunity(9);
        let approved = risk::evaluate(&opp, &portfolio, &cfg).unwrap();
        risk::open_exposure(&mut portfolio, &opp, &approved);
        let exposure = portfolio.total_exposure();
        let mut open = vec![OpenPosition {
            opp,
            approved,
            report_deadline: Instant::now() + Duration::from_secs(10),
        }];
        let bus = EventBus::new(32);
        let mut faults = HashSet::new();

        settle_execution(
            &mut portfolio,
            &cfg,
            ExecutionReport {
                signal_id: 9,
                status: ExecutionStatus::OpenRisk,
                return_pct: 0.0,
                realized_pnl: None,
                max_executable_notional: 25.0,
                observed_latency_ms: 3_000,
                note: "residual".to_string(),
            },
            &bus,
            &mut open,
            &mut faults,
        );

        assert_eq!(portfolio.total_cycles, 0);
        assert_eq!(portfolio.total_exposure(), exposure);
        assert_eq!(open.len(), 1);
        assert!(faults.contains(&9));
    }

    #[tokio::test]
    async fn demo_request_uses_exchange_symbol_without_dashboard_annotation() {
        let cfg = config();
        let mut portfolio = portfolio_with_btc_filter(&cfg);
        let mut opp = opportunity(10);
        opp.asset = "BTCUSDT [Bybit linear; evidência]".to_string();
        let bus = EventBus::new(32);
        let mut open = Vec::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        open_position(
            &mut portfolio,
            &cfg,
            opp,
            &bus,
            &mut open,
            ExecutionBackend::BybitDemo,
            Some(&tx),
        )
        .await;

        let request = rx.recv().await.unwrap();
        assert_eq!(request.symbol, "BTCUSDT");
        assert!(request.quantity > 0.0);
        assert_eq!(open.len(), 1);
    }
}
