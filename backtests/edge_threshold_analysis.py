# -*- coding: utf-8 -*-
"""
Analise de breakeven dos limiares de edge (MIN_NET_EDGE) de Arbitragem e
Order Flow, usando o log bruto continuo acumulado (data/raw_arbitrage.jsonl,
data/raw_order_flow.jsonl) — nao o events.jsonl (que so tem o que ja cruzou
o limiar).

Pergunta: por que arbitragem acerta 56%+ das operacoes e ainda perde
dinheiro no liquido? Hipotese testada aqui: o PROPRIO modelo de simulacao
(confianca em funcao do edge, perda fixa em max_loss_pct) ja implica um
ponto de breakeven — se MIN_NET_EDGE (o limiar que decide se uma
oportunidade vira ordem) estiver abaixo desse breakeven, o sistema está
aceitando, pelo seu proprio calculo interno, operacoes com valor esperado
negativo.

EV(edge) = confianca(edge) * edge - (1 - confianca(edge)) * max_loss_pct

Reproduz exatamente as formulas de orchestrator/src/sources/{arbitrage,
order_flow}.rs.
"""
import json
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "orchestrator" / "data"

# --- espelha arbitrage.rs (valores pos-correcao de breakeven, 12/08/2026) ---
ARB_MAX_LOSS_PCT = 0.004
ARB_MIN_NET_EDGE = 0.0065


def arb_confidence(edge: float) -> float:
    return min(max(0.5 + edge * 25.0, 0.4), 0.9)


def arb_ev(edge: float) -> float:
    c = arb_confidence(edge)
    return c * edge - (1 - c) * ARB_MAX_LOSS_PCT


# --- espelha order_flow.rs ---
OF_MAX_LOSS_PCT = 0.003
OF_MIN_NET_EDGE = 0.0045
OF_CONFIDENCE = 0.45  # fixa, nao escala com o edge


def of_ev(edge: float) -> float:
    return OF_CONFIDENCE * edge - (1 - OF_CONFIDENCE) * OF_MAX_LOSS_PCT


def solve_breakeven_quadratic():
    """EV(e)=0 pra arbitragem, resolvendo a quadratica no trecho onde a
    confianca nao esta saturada (0.4 < c < 0.9).

    BUG CORRIGIDO (revisao tecnica externa, 13/08/2026): o termo
    constante da equacao expandida e -0.5*max_loss_pct, nao
    -max_loss_pct. Derivacao completa:
        EV(e) = c(e)*e - (1-c(e))*L,  c(e) = 0.5 + 25e
              = c(e)*(e+L) - L
              = (0.5+25e)*(e+L) - L
              = 25e^2 + (0.5 + 25L)e + 0.5L - L
              = 25e^2 + (0.5 + 25L)e - 0.5L
    Com L=max_loss_pct=0.004: 25e^2 + 0.6e - 0.002 = 0, raiz ~0.297%
    (nao 0.544% como a v2.1 do roadmap dizia — esse valor vinha de usar
    -L em vez de -0.5L no termo constante). O limiar de producao
    (MIN_NET_EDGE=0.65%) continua valido — fica ainda mais acima do
    breakeven real do que se pensava, nao precisa mudar.
    """
    a = 25.0
    b = 0.5 + 25.0 * ARB_MAX_LOSS_PCT
    c = -0.5 * ARB_MAX_LOSS_PCT
    disc = b * b - 4 * a * c
    root = (-b + disc**0.5) / (2 * a)
    return root


def load_jsonl(path: Path):
    rows = []
    if not path.exists():
        return rows
    with open(path, "r", encoding="utf-8-sig") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return rows


def analyze_arbitrage():
    rows = load_jsonl(DATA / "raw_arbitrage.jsonl")
    breakeven = solve_breakeven_quadratic()

    per_symbol = defaultdict(list)
    taken_negative_ev = 0
    taken_positive_ev = 0
    would_be_taken_at_breakeven = 0
    total_ticks = len(rows)

    for r in rows:
        edge = max(r["edge_1"], r["edge_2"])
        per_symbol[r["symbol"]].append(edge)
        if edge >= ARB_MIN_NET_EDGE:
            ev = arb_ev(edge)
            if ev < 0:
                taken_negative_ev += 1
            else:
                taken_positive_ev += 1
        if edge >= breakeven:
            would_be_taken_at_breakeven += 1

    print("=== ARBITRAGEM ===")
    print(f"breakeven teorico (EV=0 dado o modelo de confianca atual): {breakeven*100:.3f}%")
    print(f"limiar atual (MIN_NET_EDGE): {ARB_MIN_NET_EDGE*100:.3f}%")
    print(f"ticks brutos analisados: {total_ticks}")
    print(f"ticks que cruzariam o limiar atual e teriam EV<0 (negativo, mas seriam operados hoje): {taken_negative_ev}")
    print(f"ticks que cruzariam o limiar atual com EV>=0: {taken_positive_ev}")
    if taken_negative_ev + taken_positive_ev > 0:
        pct = 100 * taken_negative_ev / (taken_negative_ev + taken_positive_ev)
        print(f"-> {pct:.1f}% das oportunidades que HOJE viram ordem tem EV negativo pelo proprio modelo")
    print(f"ticks que cruzariam um limiar no breakeven ({breakeven*100:.3f}%): {would_be_taken_at_breakeven} ({100*would_be_taken_at_breakeven/total_ticks:.2f}% do total de ticks)")

    print("\ntop 10 simbolos por edge medio (so ticks com dado valido em ambas exchanges):")
    ranked = sorted(per_symbol.items(), key=lambda kv: sum(kv[1]) / len(kv[1]), reverse=True)
    for symbol, edges in ranked[:10]:
        avg = sum(edges) / len(edges)
        above_breakeven = sum(1 for e in edges if e >= breakeven)
        print(f"  {symbol:10s} edge medio {avg*100:+.4f}%  n={len(edges):5d}  acima do breakeven: {above_breakeven} ({100*above_breakeven/len(edges):.2f}%)")

    print("\nbottom 5 simbolos por edge medio:")
    for symbol, edges in ranked[-5:]:
        avg = sum(edges) / len(edges)
        print(f"  {symbol:10s} edge medio {avg*100:+.4f}%  n={len(edges):5d}")

    return {
        "breakeven_pct": breakeven * 100,
        "current_threshold_pct": ARB_MIN_NET_EDGE * 100,
        "pct_current_trades_negative_ev": pct if (taken_negative_ev + taken_positive_ev) else None,
        "ranked_symbols": [(s, sum(e) / len(e), len(e)) for s, e in ranked],
    }


def analyze_order_flow():
    rows = load_jsonl(DATA / "raw_order_flow.jsonl")
    # breakeven pra order_flow: confianca fixa, entao e so quando c*e = (1-c)*L
    breakeven = (1 - OF_CONFIDENCE) * OF_MAX_LOSS_PCT / OF_CONFIDENCE

    per_symbol = defaultdict(list)
    taken_negative_ev = 0
    taken_positive_ev = 0
    would_be_taken_at_breakeven = 0
    total_ticks = len(rows)

    for r in rows:
        edge = r["net_edge"]
        per_symbol[r["symbol"]].append(edge)
        if edge >= OF_MIN_NET_EDGE:
            ev = of_ev(edge)
            if ev < 0:
                taken_negative_ev += 1
            else:
                taken_positive_ev += 1
        if edge >= breakeven:
            would_be_taken_at_breakeven += 1

    print("\n=== ORDER FLOW ===")
    print(f"breakeven teorico (confianca fixa em {OF_CONFIDENCE}): {breakeven*100:.3f}%")
    print(f"limiar atual (MIN_NET_EDGE): {OF_MIN_NET_EDGE*100:.3f}%")
    print(f"ticks brutos analisados: {total_ticks}")
    print(f"ticks que cruzariam o limiar atual e teriam EV<0: {taken_negative_ev}")
    print(f"ticks que cruzariam o limiar atual com EV>=0: {taken_positive_ev}")
    if taken_negative_ev + taken_positive_ev > 0:
        pct = 100 * taken_negative_ev / (taken_negative_ev + taken_positive_ev)
        print(f"-> {pct:.1f}% das oportunidades que HOJE viram ordem tem EV negativo pelo proprio modelo")
    print(f"ticks que cruzariam um limiar no breakeven ({breakeven*100:.3f}%): {would_be_taken_at_breakeven} ({100*would_be_taken_at_breakeven/total_ticks:.2f}% do total de ticks)")

    print("\ntop 10 simbolos por edge medio:")
    ranked = sorted(per_symbol.items(), key=lambda kv: sum(kv[1]) / len(kv[1]), reverse=True)
    for symbol, edges in ranked[:10]:
        avg = sum(edges) / len(edges)
        above_breakeven = sum(1 for e in edges if e >= breakeven)
        print(f"  {symbol:10s} edge medio {avg*100:+.4f}%  n={len(edges):5d}  acima do breakeven: {above_breakeven} ({100*above_breakeven/len(edges):.2f}%)")

    return {
        "breakeven_pct": breakeven * 100,
        "current_threshold_pct": OF_MIN_NET_EDGE * 100,
        "ranked_symbols": [(s, sum(e) / len(e), len(e)) for s, e in ranked],
    }


if __name__ == "__main__":
    arb_result = analyze_arbitrage()
    of_result = analyze_order_flow()

    out = {"arbitrage": arb_result, "order_flow": of_result}
    out_path = Path(__file__).resolve().parent / "reports" / "edge_threshold_analysis.json"
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"\nSalvo em {out_path}")
