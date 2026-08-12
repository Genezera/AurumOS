# -*- coding: utf-8 -*-
"""
Varredura de calibracao pro Pump Exhaustion — pump_exhaustion_historical.py
com os limiares de producao (funding>0.10%/8h E pump 24h>15%) achou ZERO
sinais em 60 dias/30 simbolos. Este script busca o mesmo dado uma vez e
testa uma grade de limiares mais frouxos, so pra achar a regiao onde
ha amostra suficiente pra um backtest fazer sentido — NAO e uma
recomendacao de mudar os limiares de producao sem validar o resultado
depois (Secao 3.1 do roadmap: numero minimo de operacoes, fora da amostra,
etc.). E puramente exploratorio.

Uso: python pump_exhaustion_calibration.py [--days 60]
"""
import argparse
import json
import time
from pathlib import Path

from pump_exhaustion_historical import (
    WIDE_SYMBOLS, fetch_klines, fetch_funding_history, funding_rate_at,
)

GRID_FUNDING = [0.0010, 0.0007, 0.0005, 0.0003, 0.0002]  # 0.10% -> 0.02%
GRID_PUMP = [0.15, 0.10, 0.07, 0.05, 0.03]  # 15% -> 3%


def count_signals(candles_by_symbol, funding_by_symbol, funding_extreme, pump_pct):
    total = 0
    per_symbol = {}
    for symbol, candles in candles_by_symbol.items():
        funding_events = funding_by_symbol.get(symbol, [])
        count = 0
        last_ts = -1
        for i, c in enumerate(candles):
            if i < 24:
                continue
            price_24h_ago = candles[i - 24]["close"]
            if price_24h_ago <= 0:
                continue
            pcnt = (c["close"] - price_24h_ago) / price_24h_ago
            funding = funding_rate_at(funding_events, c["ts"])
            if abs(funding) > funding_extreme and pcnt > pump_pct:
                if c["ts"] - last_ts >= 45_000:
                    count += 1
                    last_ts = c["ts"]
        if count:
            per_symbol[symbol] = count
        total += count
    return total, per_symbol


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--days", type=int, default=60)
    args = parser.parse_args()

    candles_by_symbol = {}
    funding_by_symbol = {}
    for idx, symbol in enumerate(WIDE_SYMBOLS, start=1):
        print(f"[{idx}/{len(WIDE_SYMBOLS)}] buscando {symbol}...")
        try:
            candles_by_symbol[symbol] = fetch_klines(symbol, args.days)
            funding_by_symbol[symbol] = fetch_funding_history(symbol, args.days)
        except RuntimeError as e:
            print(f"  erro: {e}")
        time.sleep(0.1)

    print("\n=== grade de calibracao (numero de sinais por combinacao) ===")
    print(f"{'funding>':>10} {'pump>':>8} {'sinais':>8}  simbolos com pelo menos 1 sinal")
    results = []
    for funding_extreme in GRID_FUNDING:
        for pump_pct in GRID_PUMP:
            total, per_symbol = count_signals(candles_by_symbol, funding_by_symbol, funding_extreme, pump_pct)
            results.append({
                "funding_extreme_pct": funding_extreme * 100,
                "pump_24h_pct": pump_pct * 100,
                "total_signals": total,
                "symbols_with_signal": list(per_symbol.keys()),
            })
            print(f"{funding_extreme*100:>9.2f}% {pump_pct*100:>7.1f}% {total:>8d}  {sorted(per_symbol.keys())}")

    out_path = Path(__file__).resolve().parent / "reports" / "pump_exhaustion_calibration.json"
    out_path.write_text(json.dumps(results, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"\nSalvo em {out_path}")
    print(
        "\nLembrete: isso e so pra achar onde ha amostra. Qualquer limiar novo"
        " precisa passar pelos criterios da Secao 3.1 do roadmap (numero minimo"
        " de operacoes, expectativa positiva, fora da amostra, Monte Carlo)"
        " antes de virar producao — nao e uma recomendacao de mudanca direta."
    )


if __name__ == "__main__":
    main()
