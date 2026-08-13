use std::collections::HashMap;
use std::time::Instant;

use rand::Rng;
use tokio::sync::mpsc::Receiver;
use tokio::time::Duration;

use crate::events::{DashboardEvent, EventBus};
use crate::risk::{self, PortfolioState, RiskConfig, TradeOutcome};
use crate::types::{Opportunity, Strategy};

/// Estratégias cujo `net_edge` vem de preço/book real medido agora (não de
/// heurística informativa com edge zerado). Confluência entre duas ou mais
/// destas no mesmo ativo é o único caso em que aplicamos um bônus de score
/// — nunca entre módulos informativos, que não carregam número de vantagem
/// nenhum pra somar.
const PRICE_BASED_STRATEGIES: [Strategy; 3] = [Strategy::Arbitrage, Strategy::OrderFlow, Strategy::PumpExhaustion];
const CONFLUENCE_COOLDOWN: Duration = Duration::from_secs(60);
/// Multiplicador de score quando 2+ estratégias de preço real concordam no
/// mesmo ativo na mesma janela. É uma heurística ("sinais independentes
/// concordando pesam mais"), não um número validado por backtest — só
/// prioriza entre oportunidades que já passaram no gate de edge positivo,
/// nunca torna operável algo que não teria vantagem sozinho.
const CONFLUENCE_SCORE_BONUS: f64 = 1.4;

/// Realismo de execução (Seção 3 do roadmap v2) — ainda não cobre fila
/// maker, rate limits, clock drift ou reconciliação pós-desconexão (lista
/// completa no PDF), mas modela os dois efeitos que mais inflavam o
/// resultado do paper trading: preenchimento parcial de ordem e falha da
/// segunda perna do hedge em arbitragem.
const PARTIAL_FILL_PROB: f64 = 0.35;
const PARTIAL_FILL_MIN_FRACTION: f64 = 0.30;
/// Só se aplica a Arbitragem: a perna 1 (na exchange mais barata/cara)
/// executa, mas a perna 2 na outra exchange falha ou chega tarde demais —
/// deixa a posição direcionalmente exposta sem hedge, o oposto de "sem
/// risco" que arbitragem promete no papel.
const SECOND_LEG_FAILURE_PROB: f64 = 0.03;

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

/// Roda o loop central: acumula oportunidades chegando dos módulos, a cada
/// tick escolhe a melhor entre as ainda válidas e aprovadas pelo risk
/// engine, simula o resultado (paper trading) e atualiza o portfólio.
/// Cada passo relevante é publicado no `EventBus` — o dashboard (e qualquer
/// outro consumidor futuro) lê dali, com histórico completo desde o boot.
/// Retorna o estado final para o relatório de resumo.
pub async fn run(
    mut rx: Receiver<Opportunity>,
    cfg: RiskConfig,
    max_cycles: u64,
    tick_every: Duration,
    bus: EventBus,
    state_path: String,
    kill_file_path: String,
    reset_drawdown_file_path: String,
) -> PortfolioState {
    // Recupera equity/ciclos/wins/losses de onde parou — sem isso, todo
    // restart do processo (pra aplicar código novo, por exemplo) voltaria
    // pro equity inicial, o que destruiria qualquer continuidade real do
    // paper trading.
    let mut portfolio = PortfolioState::load_or_new(&cfg, &state_path);
    // Sem isso, os tiles do dashboard ficam em "—" pra sempre até a
    // primeira operação executar — o que pode nunca acontecer se nenhuma
    // oportunidade tiver edge positivo. O estado inicial (equity de boot,
    // perna inicial, zero ciclos) é informação real e deve aparecer desde
    // o primeiro segundo, não só depois do primeiro trade.
    bus.emit(DashboardEvent::portfolio_snapshot(&portfolio));

    let mut pending: Vec<Opportunity> = Vec::new();
    let mut rng = rand::thread_rng();
    let mut confluence_cooldown: HashMap<String, Instant> = HashMap::new();
    let mut was_halted = false;
    let mut was_warned = false;

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
            _ = tick.tick() => {
                pending.retain(|o| !o.is_expired());

                let confluence_bonus = detect_and_emit_confluence(&pending, &bus, &mut confluence_cooldown);

                let kill_status = risk::kill_switch_reason(&mut portfolio, &cfg, &kill_file_path, &reset_drawdown_file_path);
                let is_halted = kill_status.halt.is_some();
                if is_halted != was_halted {
                    was_halted = is_halted;
                    if let Some(reason) = &kill_status.halt {
                        tracing::warn!(reason, "kill-switch acionado: pausando novas execucoes");
                    } else {
                        tracing::info!("kill-switch liberado: retomando execucoes normalmente");
                    }
                    bus.emit(DashboardEvent::system_halt(kill_status.halt.clone()));
                }
                let is_warned = kill_status.warn.is_some();
                if is_warned != was_warned {
                    was_warned = is_warned;
                    if let Some(msg) = &kill_status.warn {
                        tracing::warn!(msg, "aviso preventivo de drawdown");
                    }
                    // Revisão técnica externa (13/08/2026): o aviso preventivo
                    // não bastava só avisar — reduzir o tamanho da perna pela
                    // metade assim que o limiar é cruzado (uma vez, na borda
                    // da transição, não a cada ciclo enquanto o aviso durar)
                    // preserva capacidade de composição sem esperar o bloqueio
                    // rígido do dia seguinte.
                    if is_warned {
                        let scale_events = risk::halve_all_legs(&mut portfolio);
                        if !scale_events.is_empty() {
                            tracing::warn!(count = scale_events.len(), "aviso preventivo: reduzindo perna de todas as estrategias pela metade");
                            for scale in scale_events {
                                bus.emit(DashboardEvent::leg_resized(scale, portfolio.equity));
                            }
                        }
                    }
                    bus.emit(DashboardEvent::risk_warning(kill_status.warn));
                }

                let slots = if is_halted { 0 } else { MAX_TRADES_PER_TICK };
                let mut done_this_tick = 0;
                let mut hit_cycle_limit = false;

                while done_this_tick < slots {
                    // Reavalia a cada iteração: depois de executar uma
                    // oportunidade, a exposição por estratégia/grupo mudou,
                    // então a 2ª/3ª escolha respeita o risco já comprometido
                    // pela 1ª — nunca é "N ordens avaliadas contra o mesmo
                    // estado congelado".
                    let Some(idx) = pick_best(&pending, &portfolio, &cfg, &confluence_bonus) else {
                        break;
                    };
                    let opp = pending.remove(idx);
                    execute(&mut portfolio, &cfg, &opp, &mut rng, &bus);
                    portfolio.save(&state_path);
                    done_this_tick += 1;

                    if portfolio.total_cycles >= max_cycles {
                        tracing::info!(cycles = portfolio.total_cycles, "limite de ciclos atingido");
                        hit_cycle_limit = true;
                        break;
                    }
                }

                if hit_cycle_limit {
                    break;
                }
            }
        }
    }

    portfolio
}

/// Reduz o texto de um ativo ao símbolo/token que o identifica — arbitragem
/// e order flow mandam "DOGEUSDT" puro, mas pump exhaustion anexa contexto
/// como "DOGEUSDT (funding 0.15%, 24h +18%)"; cortamos no primeiro espaço
/// ou parêntese pra comparar de forma justa entre módulos.
fn base_symbol(asset: &str) -> &str {
    asset.split([' ', '(']).next().unwrap_or(asset).trim()
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
        let strategies = by_symbol.entry(base_symbol(&opp.asset)).or_default();
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

        let price_confirmed = strategies.iter().filter(|s| PRICE_BASED_STRATEGIES.contains(s)).count() >= 2;

        let keys: Vec<&'static str> = strategies.iter().map(|s| s.key()).collect();
        let cooldown_key = format!("{symbol}:{}", keys.join(","));
        let should_fire = cooldown
            .get(&cooldown_key)
            .map(|t| t.elapsed() >= CONFLUENCE_COOLDOWN)
            .unwrap_or(true);

        if should_fire {
            cooldown.insert(cooldown_key, Instant::now());
            tracing::info!(symbol, estrategias = ?keys, price_confirmed, "confluência detectada entre estratégias");
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
) -> Option<usize> {
    pending
        .iter()
        .enumerate()
        // score > 0 exige net_edge > 0: nunca executar só porque passou no
        // risk gate — precisa também ter vantagem esperada positiva. O
        // bônus de confluência entra DEPOIS desse filtro, então nunca torna
        // operável algo que não teria vantagem sozinho.
        .filter(|(_, opp)| opp.score() > 0.0 && risk::evaluate(opp, portfolio, cfg).is_ok())
        .max_by(|(_, a), (_, b)| {
            let bonus_a = confluence_bonus.get(base_symbol(&a.asset)).copied().unwrap_or(1.0);
            let bonus_b = confluence_bonus.get(base_symbol(&b.asset)).copied().unwrap_or(1.0);
            (a.score() * bonus_a).partial_cmp(&(b.score() * bonus_b)).unwrap()
        })
        .map(|(idx, _)| idx)
}

/// Aprova formalmente a oportunidade vencedora (recalcula o `Approved` já
/// que `pick_best` só checou viabilidade), simula um resultado ponderado
/// pela confiança do sinal, e registra no portfólio.
///
/// A simulação aqui é só para exercitar a mecânica do orquestrador — não é
/// um backtest e não deve ser lida como previsão de retorno real.
fn execute(
    portfolio: &mut PortfolioState,
    cfg: &RiskConfig,
    opp: &Opportunity,
    rng: &mut impl Rng,
    bus: &EventBus,
) {
    let approved = match risk::evaluate(opp, portfolio, cfg) {
        Ok(a) => a,
        Err(reason) => {
            portfolio.rejections += 1;
            tracing::debug!(?reason, asset = %opp.asset, "oportunidade rejeitada no gate final");
            bus.emit(DashboardEvent::decision_rejected(
                opp.strategy,
                &opp.asset,
                format!("{reason:?}"),
            ));
            bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
            return;
        }
    };

    bus.emit(DashboardEvent::decision_approved(
        opp.strategy,
        &opp.asset,
        approved.order_size,
    ));

    let leg_failed = opp.strategy == Strategy::Arbitrage && rng.gen_bool(SECOND_LEG_FAILURE_PROB);
    let fill_fraction = if rng.gen_bool(PARTIAL_FILL_PROB) {
        rng.gen_range(PARTIAL_FILL_MIN_FRACTION..1.0)
    } else {
        1.0
    };

    let (won, pnl, execution_note) = if leg_failed {
        // Perna 1 sozinha expõe o notional PEDIDO inteiro (não só o
        // preenchido) sem hedge — por isso usa order_size, não
        // filled_size, e o dobro do max_loss_pct modelado.
        (false, -(approved.order_size * opp.max_loss_pct * 2.0), "falha_segunda_perna")
    } else {
        let filled_size = approved.order_size * fill_fraction;
        let won = rng.gen_bool(opp.confidence.clamp(0.0, 1.0));
        let pnl = if won {
            filled_size * opp.net_edge
        } else {
            -(filled_size * opp.max_loss_pct)
        };
        (won, pnl, if fill_fraction < 1.0 { "fill_parcial" } else { "fill_total" })
    };
    let outcome = if won { TradeOutcome::Win } else { TradeOutcome::Loss };

    tracing::info!(
        strategy = opp.strategy.key(),
        asset = %opp.asset,
        outcome = ?outcome,
        order_size = approved.order_size,
        fill_fraction,
        execution_note,
        pnl,
        equity_after = portfolio.equity + pnl,
        leg_size = portfolio.leg_size(opp.strategy),
        "ordem simulada executada"
    );

    let scale_events = risk::record_trade_result(portfolio, cfg, opp, &approved, outcome, pnl);

    bus.emit(DashboardEvent::trade_result(
        opp.strategy,
        &opp.asset,
        won,
        pnl,
        portfolio.equity,
    ));
    for scale in scale_events {
        bus.emit(DashboardEvent::leg_resized(scale, portfolio.equity));
    }
    bus.emit(DashboardEvent::portfolio_snapshot(portfolio));
}
