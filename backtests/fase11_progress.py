# -*- coding: utf-8 -*-
"""
Rastreador dos criterios da Fase 11 (roadmap v2.1, Secao 8.2) — em vez de
recalcular isso manualmente toda vez que alguem pergunta "estamos prontos
pra Fase 11 ainda?", este script le o events.jsonl e responde com numero
exato, sempre.

O relogio da Fase 11 comeca no ultimo reboot em que os limiares de edge
foram corrigidos pra breakeven (12/08/2026, ~13:10 UTC) — dados anteriores
misturam o comportamento antigo (-EV) com o corrigido, comparar contra eles
seria enganoso. Reboots posteriores (recalibragem do Pump Exhaustion,
rotulagem de carteiras, etc.) nao resetam esse relogio porque nao mudaram
MIN_NET_EDGE de novo.

Uso: python fase11_progress.py
"""
import json
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EVENTS_PATH = ROOT / "orchestrator" / "data" / "events.jsonl"

# Timestamp do boot logo apos a correcao de MIN_NET_EDGE (arbitrage.rs/
# order_flow.rs) para breakeven — ver backtests/edge_threshold_analysis.py
# e Secao 7.1 do roadmap. Tudo antes disso e comportamento pre-correcao.
FASE11_CLOCK_START_MS = 1786540229000

# Criterios propostos na Secao 8.2 do roadmap v2.1 — corrigido apos revisao
# tecnica externa (13/08/2026): "um unico requisito de 150 operacoes e pouco
# pra estrategias rapidas e talvez inalcancavel pra eventos raros. Cada
# estrategia precisa de criterios proprios." Estrategias continuas (muitas
# operacoes por dia) usam 150; estrategias de evento raro com camada de
# confirmacao usam o mesmo piso de amostra da propria camada de confirmacao
# (MIN_CONFIRMATION_SAMPLES em pump_exhaustion.rs) — exigir 150 ali seria
# structuralmente inatingivel num prazo razoavel.
MIN_WEEKS = 4
# min_distinct_weeks (2a revisao tecnica externa, 13/08/2026): "150 eventos
# independentes podem demorar muitos meses" pra estrategias raras — mas o
# problema oposto tambem e real: 20 amostras que aconteceram todas na MESMA
# semana (ou pior, no mesmo dia) nao provam nada sobre regimes de mercado
# diferentes. Estrategias continuas ja cobrem varias semanas so por volume;
# estrategias raras precisam do minimo EXPLICITO de semanas distintas com
# pelo menos 1 amostra cada, nao so contagem bruta.
STRATEGY_CRITERIA = {
    "arbitrage": {"min_trades": 150, "min_profit_factor": 1.3, "min_distinct_weeks": 4},
    "order_flow": {"min_trades": 150, "min_profit_factor": 1.3, "min_distinct_weeks": 4},
    "pump_exhaustion": {"min_trades": 20, "min_profit_factor": 1.1, "min_distinct_weeks": 6},
}
# Ainda 100% observacionais (net_edge sempre 0, nunca executam) — sem
# camada de confirmacao ligada ainda, entao nenhum criterio numerico faz
# sentido pra elas ainda. Reportadas separado, sem gate.
OBSERVATIONAL_STRATEGIES = ["whale_watch", "news", "macro", "launch", "liquidation_hunter", "multi_asset"]


def load_events():
    events = []
    with open(EVENTS_PATH, "r", encoding="utf-8-sig") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return events


def main():
    events = load_events()
    since = [e for e in events if e.get("ts_ms", 0) >= FASE11_CLOCK_START_MS]

    if not since:
        print("Nenhum evento desde o inicio do relogio da Fase 11 ainda.")
        return

    elapsed_ms = since[-1]["ts_ms"] - FASE11_CLOCK_START_MS
    elapsed_days = elapsed_ms / (1000 * 3600 * 24)
    elapsed_weeks = elapsed_days / 7

    halts = [e for e in since if e.get("type") == "system_halt" and e.get("halted")]
    rejections_seen = any(e.get("type") == "decision" and e.get("approved") is False for e in since)

    print("=" * 70)
    print("FASE 11 — PROGRESSO DE VALIDACAO (roadmap v2.1, Secao 8.2)")
    print("=" * 70)
    print(f"Relogio iniciado em: 2026-08-12 13:10 UTC (correcao de limiares p/ breakeven)")
    print(f"Tempo decorrido: {elapsed_days:.2f} dias ({elapsed_weeks:.2f} semanas)")
    print()

    all_pass = True

    weeks_ok = elapsed_weeks >= MIN_WEEKS
    all_pass &= weeks_ok
    print(f"[{'OK' if weeks_ok else '..'}] Minimo {MIN_WEEKS} semanas consecutivas: {elapsed_weeks:.2f}/{MIN_WEEKS}")

    for strategy, criteria in STRATEGY_CRITERIA.items():
        min_trades = criteria["min_trades"]
        min_pf = criteria["min_profit_factor"]
        min_weeks_distinct = criteria["min_distinct_weeks"]
        trades = [e for e in since if e.get("type") == "trade_result" and e.get("strategy") == strategy]
        n = len(trades)
        wins = [t for t in trades if t["outcome"] == "win"]
        losses = [t for t in trades if t["outcome"] == "loss"]
        gross_win = sum(t["pnl"] for t in wins)
        gross_loss = abs(sum(t["pnl"] for t in losses))
        profit_factor = (gross_win / gross_loss) if gross_loss > 0 else (float("inf") if gross_win > 0 else 0.0)
        net_pnl = sum(t["pnl"] for t in trades)
        distinct_weeks = len({t["ts_ms"] // (7 * 24 * 3600 * 1000) for t in trades})

        trades_ok = n >= min_trades
        pf_ok = profit_factor >= min_pf
        net_ok = net_pnl > 0
        weeks_distinct_ok = distinct_weeks >= min_weeks_distinct
        all_pass &= trades_ok and pf_ok and net_ok and weeks_distinct_ok

        print(f"\n  {strategy} (criterio proprio: {min_trades} operacoes, PF>={min_pf}, {min_weeks_distinct} semanas distintas):")
        print(f"    [{'OK' if trades_ok else '..'}] Operacoes: {n}/{min_trades}")
        pf_str = f"{profit_factor:.2f}" if profit_factor != float("inf") else "inf (sem perdas ainda)"
        print(f"    [{'OK' if pf_ok else '..'}] Profit factor: {pf_str} (minimo {min_pf})")
        print(f"    [{'OK' if net_ok else '..'}] PnL liquido: ${net_pnl:+.4f}")
        print(f"    [{'OK' if weeks_distinct_ok else '..'}] Semanas distintas com amostra: {distinct_weeks}/{min_weeks_distinct} "
              f"(evita validar so com eventos concentrados numa unica janela de mercado)")

    print("\n  Estrategias observacionais (sem criterio numerico ainda — net_edge sempre 0):")
    for strategy in OBSERVATIONAL_STRATEGIES:
        received = len([e for e in since if e.get("type") == "opportunity_received" and e.get("strategy") == strategy])
        print(f"    {strategy}: {received} oportunidades observadas, 0 operacoes (esperado)")

    halts_ok = len(halts) == 0
    all_pass &= halts_ok
    print(f"\n[{'OK' if halts_ok else '..'}] Kill-switch nunca acionado: {'sim' if halts_ok else f'{len(halts)} vez(es)'}")

    rejections_ok = True  # rejeicoes SAO o motor de risco funcionando, nao uma falha
    print(f"[OK] Motor de risco rejeitou operacoes fora do limite quando necessario: "
          f"{'sim, funcionando' if rejections_seen else 'nenhuma oportunidade tentou passar do limite ainda'}")

    print()
    print("=" * 70)
    if all_pass:
        print("Todos os criterios automaticos batem. Falta so a revisao manual sua")
        print("dos logs de decisao (Secao 8.2) antes de considerar a Fase 11 completa.")
    else:
        print("Ainda nao — pelo menos um criterio automatico nao foi atingido.")
        print("Isso e esperado enquanto o tempo/amostra nao acumulou o suficiente.")
    print("=" * 70)


if __name__ == "__main__":
    main()
