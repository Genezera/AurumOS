<div align="center">

<img src="docs/brand/aurumos-mark.svg" width="96" height="96" alt="AurumOS" />

# AurumOS

**Multi-strategy quantitative trading orchestrator — 100% paper trading, real data, no mocks.**

![status](https://img.shields.io/badge/status-paper--trading-f5c453?style=flat-square&labelColor=0c1117)
![language](https://img.shields.io/badge/core-Rust-f5c453?style=flat-square&labelColor=0c1117)
![real capital](https://img.shields.io/badge/real%20capital-never-ff4d67?style=flat-square&labelColor=0c1117)
![dashboard](https://img.shields.io/badge/dashboard-axum%20%2B%20WebSocket-18d7ff?style=flat-square&labelColor=0c1117)
![license](https://img.shields.io/badge/use-personal%20project-aab6c3?style=flat-square&labelColor=0c1117)

</div>

---

## What it is

AurumOS is a trading orchestrator that runs **9 independent strategies in parallel**,
each reading real market data (Bybit, Bitget, Ethereum on-chain, SEC EDGAR, Alpaca/IEX)
and competing for a shared risk budget through a central decision engine. Nothing here
is simulated or synthetic: the prices, spreads, liquidations, filings and on-chain
transactions are real — only execution is paper trading, with no real money connected.

The goal isn't a single "winning" strategy, but a **system that decides on its own where
to allocate risk** across opportunities of completely different natures — from
millisecond arbitrage to macro events that happen once a month — using the same risk
engine, the same sizing and the same kill-switch for all of them.

> Permanent project rule: **it never connects real capital and never creates accounts on
> the user's behalf**, regardless of any autonomy granted. All the trade history below is
> paper trading.

---

## Architecture

```
                        ┌─────────────────────────────────────────┐
                        │            9 SignalSource                │
                        │  (each reads real data, emits            │
                        │   an Opportunity on a shared async chan) │
                        └───────────────────┬───────────────────────┘
                                             │ mpsc::Sender<Opportunity>
                                             ▼
                        ┌─────────────────────────────────────────┐
                        │   Orchestrator (orchestrator.rs)          │
                        │   • confluence engine (fusion.rs)         │
                        │   • scorer by speed/edge                   │
                        │   • contention over a risk budget          │
                        └───────────────────┬───────────────────────┘
                                             │
                        ┌────────────────────┴────────────────────┐
                        ▼                                          ▼
        ┌───────────────────────────┐            ┌───────────────────────────┐
        │  Risk engine (risk.rs)     │            │  EventBus (events.rs)     │
        │  • hierarchical Kelly       │            │  • backlog + live replay   │
        │  • 4-layer kill-switch      │            │    over WebSocket          │
        │  • per strategy/symbol      │            │  • persisted to JSONL      │
        │    scaling                  │            └─────────────┬─────────────┘
        └─────────────────────────────┘                          ▼
                                                    ┌───────────────────────────┐
                                                    │  Dashboard (axum)          │
                                                    │  http://127.0.0.1:7878     │
                                                    └───────────────────────────┘
```

Each module implements the same contract (`trait SignalSource` in
[`sources/mod.rs`](orchestrator/src/sources/mod.rs)): read real data, emit an
`Opportunity`, never decide on its own whether to execute. Every execution, sizing and
risk-cut decision goes through a single central engine — that's what lets a 3-second
arbitrage opportunity be compared against a whale-watch signal that lasts hours using the
same yardstick.

---

## The 9 strategies

Only 3 of them can actually place a trade — the other 6 are structurally informative:
they always emit `net_edge = 0.0`, which makes `score()` always zero, and `pick_best()`
discards any candidate with score ≤ 0 before it even reaches the risk engine. It's not a
convention, it's guaranteed in code.

| Strategy | Path | Data source | What it captures |
|---|---|---|---|
| **Arbitrage** | Executes | Bybit ↔ Bitget book (spot) | Executable price divergence between exchanges |
| **Order Flow** | Executes | Bybit book (spot) | Spread capture as a market maker |
| **Pump Exhaustion** | Executes* | Bybit funding + OI + liquidations (perps) | Move exhaustion (fusion of on-chain whale watch + liquidation hunter) |
| **Whale Watch** | Observation | Ethereum on-chain (USDT/USDC Transfer) | Movement of large labelled wallets |
| **Liquidation Hunter** | Observation | Bybit `allLiquidation` | Liquidation cascades in perps |
| **News Reactor** | Observation | SEC EDGAR (live 8-K filings) + local LLM (Ollama) | Reaction to corporate events classified by a local AI, no key, no cost |
| **Launch Radar** | Observation (extra structural lock) | Bybit `instruments-info` + Uniswap V2 `PairCreated` | New CEX/DEX listings, with a holders/LP checklist |
| **Macro Engine** | Observation | Official FOMC/CPI/NFP calendar | Volatility windows around macro events |
| **Multi-Asset** | Observation | Alpaca (IEX) | Equities via the free feed, idle until the user supplies their own key |

*Pump Exhaustion has the same real-price confirmation layer, discounted execution fee and
bootstrap resampling that Arbitrage/Order Flow already use — but it hasn't yet accumulated
enough samples to leave `net_edge=0.0`: 0 trades executed so far.

Whale Watch and Liquidation Hunter were operationally fused into Pump Exhaustion — three
independent data sources feeding a single exhaustion signal, instead of three strategies
competing for the same risk.

---

## Risk engine

- **Real simultaneous exposure**: positions are genuinely reserved between approval and
  resolution (30s Order Flow, 2s Arbitrage, 20min Pump Exhaustion) — no more instant
  round-trip. Per-strategy/group/portfolio risk limits now block on true SIMULTANEOUS
  excess, not just on a single trade being too big.
- **Hierarchical Kelly** (portfolio → strategy → symbol): the fraction of capital per trade
  is weighted by the PnL history at each level, blending automatically as the per-symbol
  sample grows. The hit rate used in the calculation is the Wilson 95% lower confidence
  bound, not the point estimate — less prone to overconfidence on a small sample. Symbols
  of the same strategy are normalized against each other so they don't each claim 2x at
  once.
- **Profit-factor hierarchy**: PF ≥ 1.3 authorizes growing the leg; PF < 1.2 forces an
  active reduction; between the two, it keeps the current size.
- **Evidence-gated scaling**: growing also requires free risk budget (< 50% of the
  strategy's limit in use) and symbol diversity (profit from at least 2 distinct symbols),
  on top of the time/cycles floor.
- **Global leverage cap**: the sum of the leveraged notional of all open positions,
  divided by equity, never exceeds the configured 1.0x (2.0x is the absolute cap in code,
  not editable).
- **Non-destructive drawdown scaling**: instead of permanently shrinking the leg size on
  every trade in drawdown, a ceiling (`drawdown_ceiling_multiplier`) recomputed each cycle
  applies the reduction as a multiplier — without collapsing sizing to the floor.
- **Partial recovery (80%)**: scaling back up doesn't require recovering 100% of the equity
  peak, only 80% of the drop from the peak.
- **4-layer kill-switch**: preventive warning (1% daily, halves the leg), hard daily block
  (1.5%), weekly block (4%), full block on historical drawdown (7%, requires a manual
  reset). It only blocks NEW openings — already-open positions resolve on their own at
  their natural horizon (at most 30s today).
- **Real per-venue rate limiter**: token bucket (8 requests/s), simulating the limit of a
  common retail account — consumed only when an order is actually opened.
- **Real minimum notional per symbol**: fetched live from the exchange's own public API at
  boot (Bybit and Bitget); an order below the real minimum is rejected.
- **Bootstrap resampling**: the result of each trade (Arbitrage, Order Flow, Pump
  Exhaustion) is drawn from a REAL outcome already confirmed against price, not from a
  `confidence×net_edge` formula.
- **Deduplication of overlapping signals**: overlapping confirmation windows on the same
  symbol don't count as independent samples — only one confirmation in flight per symbol at
  a time.
- **Launch Radar structural lock**: independent of the score gate (which already blocks on
  net_edge=0), a code flag prevents executing token launches — the system's lowest-liquidity
  context.
- **Contention over a risk budget**: no fixed "N simultaneous trades per capital tier"
  table — the correlation and total-risk engine (`risk::evaluate`) decides, re-evaluated
  every cycle.
- **Dynamic symbol universe**: rotation every 15 minutes across the whole available market
  (perps and spot separately), ranked by observed edge, with a display refresh every 5
  seconds.

All risk state (positions, per strategy/symbol PnL, per-symbol edge scores, price
confirmation history) is persisted to disk and reloaded at boot — restarting the process
doesn't reset the accumulated progress.

---

## Dashboard

An axum server embedded in the binary itself, serving a vanilla JS/HTML frontend (no
framework) with the official AurumOS brand kit. WebSocket with full backlog replay on
connect — opening the dashboard never loses history.

```
http://127.0.0.1:7878
```

Panels: per-strategy pulse (with a PnL sparkline for the ones that have traded, and an
honest explanation of why the others are still dormant), risk cockpit (kill-switch,
drawdown, manual stop), real-time exposure per strategy, live symbol universe with ranked
edge, event feed.

---

## How to run

```bash
# build (from the repository root)
cargo build --release

# run (must be run from inside orchestrator/, where config/risk.toml lives)
cd orchestrator
../target/release/orchestrator.exe
```

Optional environment variables (everything has a free/public fallback if omitted):

| Variable | Module | Default without it |
|---|---|---|
| `AURUMOS_ETH_WS_URL` / `AURUMOS_ETH_HTTP_URL` | Whale Watch, Launch Radar DEX | Public RPC node (publicnode.com) |
| `ALPACA_API_KEY_ID` / `ALPACA_API_SECRET_KEY` | Multi-Asset | Module stays idle |

**Local Ollama** (optional, not an environment variable): News Reactor uses `llama3.2:3b`
via `http://localhost:11434` to classify SEC filings. Without Ollama running, it falls back
to a deterministic neutral default — it never invents a direction, it just goes without the
real classification.

**Manual kill-switch**: creating the file `orchestrator/data/KILL` (any content) pauses new
executions; deleting it resumes. It doesn't cancel already-open positions — they resolve on
their own at their natural horizon (at most 30s today).

---

## Repository structure

```
orchestrator/          Rust core — orchestrator, risk engine, dashboard, 9 signal sources
  src/sources/          one module per strategy, all implementing SignalSource
  config/risk.toml       all capital and risk configuration, documented inline
  dashboard/             vanilla JS/HTML frontend embedded in the binary
backtests/              Python analysis scripts over collected real data (events.jsonl, raw logs)
docs/                   technical roadmap generator (generate_roadmap.py, reportlab) + brand kit
```

The full technical roadmap (detailed architecture, audit history, bugs found and fixed,
next steps) is generated locally from `docs/generate_roadmap.py` and is not versioned in
this repository — run the script to get the most up-to-date PDF.

---

## Project state

A personal project, in active solo development. Each strategy evolves from "instrumented"
to "trusted" based on real data accumulated in paper trading — there's no engineering
shortcut for the passage of time that requires. Real bugs already found and fixed include
concurrent event corruption, breakeven calculation errors, symbol universes incompatible
with the connected exchange, destructive risk reduction under drawdown, a frozen Bitget
book being treated as an executable quote (producing a false "100% hit rate" on one
symbol), and a correlation deadlock that froze the whole system the moment simultaneous
exposure became real — each documented in the commit history at the moment it was fixed.

Most recent numeric result (the entire accumulated history, not a clean post-fix window):
Arbitrage 87.9% hit rate / +US$1,002.79, Order Flow 90.9% / +US$2,268.81, Pump Exhaustion
still with no trade (accumulating sample). See the PDF (`docs/generate_roadmap.py`) for the
full breakdown, including the honest note that this number mixes old code (less realistic)
with the most recent.
