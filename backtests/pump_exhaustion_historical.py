"""
Backtest historico formal — Pump Exhaustion (Fase 10 / criterio de saida da
Fase 6 do roadmap).

Reproduz EXATAMENTE a heuristica de orchestrator/src/sources/pump_exhaustion.rs
(funding rate > 0.10%/8h em valor absoluto E variacao de 24h > +15%, ambos ao
mesmo tempo) contra dado historico real da Bybit (klines + funding rate
publicos, sem API key), para os mesmos simbolos monitorados em producao
(WIDE_SYMBOLS em orchestrator/src/main.rs).

Para cada disparo do gatilho, mede se o preco caiu (o padrao de "exaustao"
esperado) numa janela de horizonte fixa apos o sinal, e compara a taxa de
acerto contra um marcador aleatorio de mesma frequencia por simbolo — exigido
explicitamente pelo criterio de saida da Fase 6/10 ("documentado com numero
exato, nao estimativa").

Uso:
    python pump_exhaustion_historical.py [--days 60] [--horizon-hours 24]
"""
from __future__ import annotations

import argparse
import json
import random
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

BASE_URL = "https://api.bybit.com"

# Mesma lista de orchestrator/src/main.rs::WIDE_SYMBOLS — mesmo universo
# monitorado ao vivo, pra o backtest ser comparavel com o paper trading real.
WIDE_SYMBOLS = [
    "BTCUSDT", "ETHUSDT", "SOLUSDT", "XRPUSDT", "DOGEUSDT", "ADAUSDT", "TRXUSDT", "LTCUSDT",
    "AVAXUSDT", "DOTUSDT", "LINKUSDT", "INJUSDT", "ATOMUSDT", "NEARUSDT", "UNIUSDT", "APTUSDT",
    "ARBUSDT", "OPUSDT", "SUIUSDT", "FILUSDT", "BCHUSDT", "ETCUSDT", "XLMUSDT", "ALGOUSDT",
    "AAVEUSDT", "MKRUSDT", "LDOUSDT", "GRTUSDT", "SANDUSDT", "IMXUSDT",
]

# Mesmos limiares exatos de orchestrator/src/sources/pump_exhaustion.rs —
# se estes numeros divergirem do .rs, o backtest deixa de valer.
FUNDING_EXTREME = 0.001  # 0.10%/8h
PUMP_24H_PCNT = 0.15  # +15% em 24h

RETRIES = 3


def http_get_json(url: str) -> dict:
    last_err = None
    for attempt in range(RETRIES):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "AurumOS-backtest/1.0"})
            with urllib.request.urlopen(req, timeout=20) as r:
                return json.load(r)
        except (urllib.error.URLError, TimeoutError) as e:
            last_err = e
            time.sleep(1.5 * (attempt + 1))
    raise RuntimeError(f"falha ao buscar {url}: {last_err}")


def fetch_klines(symbol: str, days: int) -> list[dict]:
    """1h candles, mais recentes primeiro na resposta da Bybit; devolvemos em
    ordem cronologica (mais antigo primeiro)."""
    end_ms = int(time.time() * 1000)
    start_ms = end_ms - days * 24 * 3600 * 1000
    candles: list[dict] = []
    cursor_end = end_ms
    # 1000 candles por pagina, 1h cada => ~41 dias por pagina. Pagina pra tras
    # ate cobrir a janela pedida.
    while cursor_end > start_ms:
        url = (
            f"{BASE_URL}/v5/market/kline?category=linear&symbol={symbol}"
            f"&interval=60&start={start_ms}&end={cursor_end}&limit=1000"
        )
        data = http_get_json(url)
        rows = data.get("result", {}).get("list", [])
        if not rows:
            break
        for row in rows:
            candles.append({
                "ts": int(row[0]),
                "open": float(row[1]),
                "high": float(row[2]),
                "low": float(row[3]),
                "close": float(row[4]),
            })
        oldest = min(int(r[0]) for r in rows)
        if oldest <= start_ms or oldest >= cursor_end:
            break
        cursor_end = oldest
        time.sleep(0.1)
    candles.sort(key=lambda c: c["ts"])
    # dedup (paginas podem se sobrepor na borda)
    seen = set()
    unique = []
    for c in candles:
        if c["ts"] in seen:
            continue
        seen.add(c["ts"])
        unique.append(c)
    return unique


def fetch_funding_history(symbol: str, days: int) -> list[dict]:
    end_ms = int(time.time() * 1000)
    start_ms = end_ms - days * 24 * 3600 * 1000
    events: list[dict] = []
    cursor_end = end_ms
    while cursor_end > start_ms:
        url = (
            f"{BASE_URL}/v5/market/funding/history?category=linear&symbol={symbol}"
            f"&startTime={start_ms}&endTime={cursor_end}&limit=200"
        )
        data = http_get_json(url)
        rows = data.get("result", {}).get("list", [])
        if not rows:
            break
        for row in rows:
            events.append({
                "ts": int(row["fundingRateTimestamp"]),
                "rate": float(row["fundingRate"]),
            })
        oldest = min(int(r["fundingRateTimestamp"]) for r in rows)
        if oldest <= start_ms or oldest >= cursor_end:
            break
        cursor_end = oldest
        time.sleep(0.1)
    events.sort(key=lambda e: e["ts"])
    return events


def funding_rate_at(funding_events: list[dict], ts: int) -> float:
    """Funding mais recente com timestamp <= ts (busca linear com cursor
    decrescente ja que ambas listas estao ordenadas)."""
    best = 0.0
    for ev in funding_events:
        if ev["ts"] > ts:
            break
        best = ev["rate"]
    return best


@dataclass
class Signal:
    symbol: str
    ts: int
    entry_price: float
    funding_rate: float
    price_24h_pcnt: float
    outcome_pcnt: float | None = None  # variacao do preco no horizonte, negativo = sucesso (caiu)


def backtest_symbol(symbol: str, candles: list[dict], funding_events: list[dict], horizon_hours: int) -> list[Signal]:
    signals: list[Signal] = []
    last_signal_ts = -1
    cooldown_ms = 45_000  # mesmo EMIT_COOLDOWN do .rs, mas em escala horaria isso so evita duplicar no mesmo candle
    for i, c in enumerate(candles):
        if i < 24:
            continue  # precisa de 24h de historico pra calcular variacao 24h
        price_24h_ago = candles[i - 24]["close"]
        if price_24h_ago <= 0:
            continue
        pcnt_24h = (c["close"] - price_24h_ago) / price_24h_ago
        funding = funding_rate_at(funding_events, c["ts"])

        funding_extreme = abs(funding) > FUNDING_EXTREME
        big_pump = pcnt_24h > PUMP_24H_PCNT
        if not (funding_extreme and big_pump):
            continue
        if c["ts"] - last_signal_ts < cooldown_ms:
            continue
        last_signal_ts = c["ts"]

        horizon_idx = i + horizon_hours
        outcome_pcnt = None
        if horizon_idx < len(candles):
            future_price = candles[horizon_idx]["close"]
            outcome_pcnt = (future_price - c["close"]) / c["close"]

        signals.append(Signal(
            symbol=symbol,
            ts=c["ts"],
            entry_price=c["close"],
            funding_rate=funding,
            price_24h_pcnt=pcnt_24h,
            outcome_pcnt=outcome_pcnt,
        ))
    return signals


def random_baseline(symbol: str, candles: list[dict], n_signals: int, horizon_hours: int, rng: random.Random) -> list[float]:
    """N timestamps aleatorios (com >=24h de historico e horizonte completo
    disponivel), mesma contagem que os sinais reais desse simbolo — pra
    comparar taxa de acerto contra a MESMA frequencia, nao so uma media
    global."""
    outcomes = []
    if n_signals == 0:
        return outcomes
    valid_range = list(range(24, len(candles) - horizon_hours))
    if not valid_range:
        return outcomes
    picks = rng.choices(valid_range, k=n_signals)
    for i in picks:
        entry = candles[i]["close"]
        future = candles[i + horizon_hours]["close"]
        outcomes.append((future - entry) / entry)
    return outcomes


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--days", type=int, default=60)
    parser.add_argument("--horizon-hours", type=int, default=24)
    parser.add_argument("--seed", type=int, default=1337)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parent / "reports")
    args = parser.parse_args()

    rng = random.Random(args.seed)
    all_signals: list[Signal] = []
    per_symbol_baseline: list[float] = []
    fetch_errors: list[str] = []

    for idx, symbol in enumerate(WIDE_SYMBOLS, start=1):
        print(f"[{idx}/{len(WIDE_SYMBOLS)}] buscando {symbol}...")
        try:
            candles = fetch_klines(symbol, args.days)
            funding_events = fetch_funding_history(symbol, args.days)
        except RuntimeError as e:
            fetch_errors.append(f"{symbol}: {e}")
            continue
        if len(candles) < 48:
            fetch_errors.append(f"{symbol}: dado historico insuficiente ({len(candles)} candles)")
            continue

        signals = backtest_symbol(symbol, candles, funding_events, args.horizon_hours)
        all_signals.extend(signals)
        baseline = random_baseline(symbol, candles, len(signals), args.horizon_hours, rng)
        per_symbol_baseline.extend(baseline)
        time.sleep(0.1)

    resolved = [s for s in all_signals if s.outcome_pcnt is not None]
    successes = [s for s in resolved if s.outcome_pcnt < 0]  # sucesso = preco caiu apos o sinal
    success_rate = (len(successes) / len(resolved) * 100.0) if resolved else 0.0
    avg_outcome = (sum(s.outcome_pcnt for s in resolved) / len(resolved)) if resolved else 0.0

    baseline_successes = [o for o in per_symbol_baseline if o < 0]
    baseline_rate = (len(baseline_successes) / len(per_symbol_baseline) * 100.0) if per_symbol_baseline else 0.0
    baseline_avg = (sum(per_symbol_baseline) / len(per_symbol_baseline)) if per_symbol_baseline else 0.0

    report = {
        "descricao": "Backtest historico Pump Exhaustion — heuristica exata de pump_exhaustion.rs contra dado real Bybit",
        "janela_dias": args.days,
        "horizonte_horas": args.horizon_hours,
        "limiares": {"funding_extreme": FUNDING_EXTREME, "pump_24h_pcnt": PUMP_24H_PCNT},
        "simbolos_com_erro_de_busca": fetch_errors,
        "sinais_disparados": len(all_signals),
        "sinais_com_horizonte_completo": len(resolved),
        "detector": {
            "taxa_de_acerto_pct": round(success_rate, 2),
            "variacao_media_no_horizonte_pct": round(avg_outcome * 100.0, 3),
        },
        "marcador_aleatorio_mesma_frequencia": {
            "amostras": len(per_symbol_baseline),
            "taxa_de_acerto_pct": round(baseline_rate, 2),
            "variacao_media_no_horizonte_pct": round(baseline_avg * 100.0, 3),
        },
        "detector_supera_aleatorio": success_rate > baseline_rate,
        "sinais": [
            {
                "symbol": s.symbol,
                "ts": s.ts,
                "funding_rate_pct": round(s.funding_rate * 100, 4),
                "price_24h_pcnt": round(s.price_24h_pcnt * 100, 2),
                "outcome_pcnt": round(s.outcome_pcnt * 100, 3) if s.outcome_pcnt is not None else None,
            }
            for s in all_signals
        ],
    }

    args.out.mkdir(parents=True, exist_ok=True)
    out_path = args.out / "pump_exhaustion_historical_report.json"
    out_path.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")

    print()
    print(f"Sinais disparados: {len(all_signals)} ({len(resolved)} com horizonte de {args.horizon_hours}h completo)")
    print(f"Detector  — taxa de acerto: {success_rate:.2f}%  |  variacao media: {avg_outcome*100:.3f}%")
    print(f"Aleatorio — taxa de acerto: {baseline_rate:.2f}%  |  variacao media: {baseline_avg*100:.3f}%  (n={len(per_symbol_baseline)})")
    print(f"Detector supera o marcador aleatorio: {report['detector_supera_aleatorio']}")
    if fetch_errors:
        print(f"Simbolos com erro de busca: {fetch_errors}")
    print(f"\nSalvo em {out_path}")


if __name__ == "__main__":
    main()
