"""
Relatorio de performance do paper trading ao vivo (AurumOS).

O que isto NAO e: um backtest historico formal (Fase 10 do roadmap), que
reproduz decisoes contra dado de mercado do passado. Isto le o registro real
de decisoes que o orquestrador ja tomou, ao vivo, com dado de mercado real
(orchestrator/data/events.jsonl) e resume por estrategia. E o insumo direto
da Fase 11 (validacao em paper trading prolongado): revisao numerica do que
de fato aconteceu, nao estimativa.

Uso:
    python live_performance_report.py [--events PATH] [--out PATH]
"""
from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path


def load_events(path: Path) -> list[dict]:
    events = []
    skipped = 0
    with open(path, "r", encoding="utf-8-sig") as f:
        for raw_line in f:
            line = raw_line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                skipped += 1
    if skipped:
        print(f"aviso: {skipped} linha(s) corrompida(s) em {path} foram ignoradas")
    return events


def max_drawdown(curve: list[float]) -> float:
    """Maior queda (em valor absoluto) do pico acumulado ate um vale
    posterior, sobre a curva de pnl acumulado da propria estrategia."""
    peak = 0.0
    worst = 0.0
    running = 0.0
    for pnl in curve:
        running += pnl
        peak = max(peak, running)
        worst = min(worst, running - peak)
    return worst


def build_report(events: list[dict]) -> dict:
    by_strategy_trades: dict[str, list[dict]] = defaultdict(list)
    by_strategy_rejections: dict[str, int] = defaultdict(int)
    by_strategy_received: dict[str, int] = defaultdict(int)

    for e in events:
        t = e.get("type")
        if t == "trade_result":
            by_strategy_trades[e["strategy"]].append(e)
        elif t == "decision" and e.get("approved") is False:
            by_strategy_rejections[e["strategy"]] += 1
        elif t == "opportunity_received":
            by_strategy_received[e["strategy"]] += 1

    all_strategies = sorted(set(by_strategy_trades) | set(by_strategy_rejections) | set(by_strategy_received))

    report = {"strategies": {}, "generated_from_events": len(events)}
    for strat in all_strategies:
        trades = by_strategy_trades.get(strat, [])
        n = len(trades)
        wins = [t for t in trades if t["outcome"] == "win"]
        losses = [t for t in trades if t["outcome"] == "loss"]
        pnl_curve = [t["pnl"] for t in trades]
        net_pnl = sum(pnl_curve)
        win_rate = (len(wins) / n * 100.0) if n else 0.0
        avg_win = (sum(t["pnl"] for t in wins) / len(wins)) if wins else 0.0
        avg_loss = (sum(t["pnl"] for t in losses) / len(losses)) if losses else 0.0
        # razao risco/retorno: quanto se ganha em media por cada unidade
        # perdida em media. avg_loss e negativo, entao dividir por abs().
        risk_reward = (avg_win / abs(avg_loss)) if avg_loss != 0 else None

        report["strategies"][strat] = {
            "operacoes": n,
            "oportunidades_recebidas": by_strategy_received.get(strat, 0),
            "rejeitadas_pelo_risco": by_strategy_rejections.get(strat, 0),
            "vitorias": len(wins),
            "derrotas": len(losses),
            "taxa_de_acerto_pct": round(win_rate, 2),
            "pnl_liquido_acumulado": round(net_pnl, 6),
            "drawdown_maximo_da_estrategia": round(max_drawdown(pnl_curve), 6),
            "ganho_medio": round(avg_win, 6),
            "perda_media": round(avg_loss, 6),
            "razao_risco_retorno": round(risk_reward, 3) if risk_reward is not None else None,
        }
    return report


def render_markdown(report: dict) -> str:
    lines = [
        "# AurumOS — Relatorio de Performance (Paper Trading ao Vivo)",
        "",
        "**Isto NAO e um backtest historico formal (Fase 10).** E o resumo numerico",
        "do que o orquestrador decidiu de fato, ao vivo, com dado de mercado real e",
        "execucao simulada (paper trading) — insumo direto da Fase 11 do roadmap",
        "(validacao em paper trading prolongado).",
        "",
        f"Eventos analisados: **{report['generated_from_events']}**",
        "",
        "| Estrategia | Operacoes | Oport. recebidas | Rejeitadas (risco) | Taxa de acerto | PnL liquido | Drawdown max | Razao risco/retorno |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for strat, s in sorted(report["strategies"].items()):
        rr = f"{s['razao_risco_retorno']:.2f}" if s["razao_risco_retorno"] is not None else "—"
        lines.append(
            f"| {strat} | {s['operacoes']} | {s['oportunidades_recebidas']} | "
            f"{s['rejeitadas_pelo_risco']} | {s['taxa_de_acerto_pct']:.1f}% | "
            f"${s['pnl_liquido_acumulado']:.4f} | ${s['drawdown_maximo_da_estrategia']:.4f} | {rr} |"
        )
    lines += [
        "",
        "_Resultado passado (mesmo que real) nao garante resultado futuro. Estrategias",
        "com `operacoes = 0` nunca dispararam ordem porque nunca tiveram net_edge > 0",
        "(modulos informativos por design — ver roadmap Secao 0)._",
    ]
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    default_events = Path(__file__).resolve().parent.parent / "orchestrator" / "data" / "events.jsonl"
    default_out = Path(__file__).resolve().parent / "reports"
    parser.add_argument("--events", type=Path, default=default_events)
    parser.add_argument("--out", type=Path, default=default_out)
    args = parser.parse_args()

    events = load_events(args.events)
    report = build_report(events)

    args.out.mkdir(parents=True, exist_ok=True)
    json_path = args.out / "live_performance_report.json"
    md_path = args.out / "live_performance_report.md"
    json_path.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    md_path.write_text(render_markdown(report), encoding="utf-8")

    print(render_markdown(report))
    print(f"\nSalvo em {json_path} e {md_path}")


if __name__ == "__main__":
    main()
