# -*- coding: utf-8 -*-
"""
Re-varredura historica das transferencias USDT/USDC >= $250k no periodo em
que o Whale Watch ja esteve rodando, usando a lista AMPLIADA de enderecos
rotulados (whale_watch.rs, revisao de 13/08/2026) — objetivo: descobrir
quantos dos 2.698 eventos ja emitidos teriam batido em algum endereco
rotulado se a lista de hoje ja existisse desde o inicio, e assim saber se
ja da pra backtestar a hipotese de direcao (deposito=short/saque=long)
contra preco real, ou se ainda falta cobertura de endereco.

Usa a MESMA RPC (Alchemy, via orchestrator/.env) e a MESMA logica de
decodificacao que orchestrator/src/sources/whale_watch.rs — nao inventa
dado novo, so reconsulta o que ja aconteceu on-chain.

Uso:
    python whale_watch_rescan.py
"""
from __future__ import annotations

import json
import re
import time
from pathlib import Path

import requests

ROOT = Path(__file__).resolve().parent.parent
ENV_PATH = ROOT / "orchestrator" / ".env"
EVENTS_PATH = ROOT / "orchestrator" / "data" / "events.jsonl"

TRANSFER_TOPIC = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
USDT_CONTRACT = "0xdAC17F958D2ee523a2206206994597C13D831ec"
USDC_CONTRACT = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
MIN_WHALE_USD = 250_000.0
CHUNK_BLOCKS = 10  # limite duro do tier gratuito da Alchemy p/ eth_getLogs
RPC_DELAY_S = 0.12
MAX_RETRIES = 4

# Mesma lista de orchestrator/src/sources/whale_watch.rs (KNOWN_LABELS),
# revisao de 13/08/2026 — mantida em sincronia manualmente.
KNOWN_LABELS = {
    "0x28c6c06298d514db089934071355e5743bf21d60": "Binance",
    "0xf977814e90da44bfa03b6295a0616a897441acec": "Binance",
    "0x21a31ee1afc51d94c2efccaa2092ad1028285549": "Binance",
    "0xdfd5293d8e347dfe59e90efd55b2956a1343963d": "Binance",
    "0x71660c4005ba85c37ccec55d0c4493e66fe775d3": "Coinbase",
    "0x2910543af39aba0cd09dbb2d50200b3e800a63d2": "Kraken",
    "0xf89d7b9c864f589bbf53a82105107622b35eaa40": "Bybit",
    "0x4b4e14a3773ee558b6597070797fd51eb48606e5": "OKX",
    "0x4e7b110335511f662fdbb01bf958a7844118c0d4": "OKX",
    "0x53f78a071d04224b8e254e243fffc6d9f2f3fa23": "KuCoin",
    "0x2b5634c42055806a59e9107ed44d43c426e58258": "KuCoin",
}


def load_rpc_url() -> str:
    content = ENV_PATH.read_text(encoding="utf-8")
    m = re.search(r"AURUMOS_ETH_HTTP_URL=(\S+)", content)
    if not m:
        raise SystemExit("AURUMOS_ETH_HTTP_URL nao encontrado em orchestrator/.env")
    return m.group(1)


def rpc_call(url: str, method: str, params: list) -> dict:
    resp = requests.post(url, json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, timeout=30)
    resp.raise_for_status()
    body = resp.json()
    if "error" in body:
        raise RuntimeError(f"RPC error: {body['error']}")
    return body["result"]


def get_session_start_ms() -> int:
    """Menor ts_ms entre os eventos opportunity_received do whale_watch ja
    persistidos — marca o inicio real da janela que da pra re-varrer."""
    ts_min = None
    with open(EVENTS_PATH, "r", encoding="utf-8-sig") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                e = json.loads(line)
            except json.JSONDecodeError:
                continue
            if e.get("type") == "opportunity_received" and e.get("strategy") == "whale_watch":
                if ts_min is None or e["ts_ms"] < ts_min:
                    ts_min = e["ts_ms"]
    if ts_min is None:
        raise SystemExit("nenhum evento whale_watch encontrado em events.jsonl")
    return ts_min


def block_timestamp(url: str, block_num: int) -> int:
    r = rpc_call(url, "eth_getBlockByNumber", [hex(block_num), False])
    return int(r["timestamp"], 16)


def find_block_at_or_before(url: str, target_ts: int, latest_block: int) -> int:
    """Busca binaria: maior numero de bloco cujo timestamp <= target_ts."""
    lo, hi = 1, latest_block
    while lo < hi:
        mid = (lo + hi + 1) // 2
        ts = block_timestamp(url, mid)
        if ts <= target_ts:
            lo = mid
        else:
            hi = mid - 1
        time.sleep(RPC_DELAY_S)
    return lo


def decode_amount_usd(data_hex: str) -> float:
    raw = data_hex[2:] if data_hex.startswith("0x") else data_hex
    tail = raw[-32:] if len(raw) > 32 else raw
    return int(tail, 16) / 1_000_000.0


def extract_address(topic_hex: str) -> str:
    hex_part = topic_hex[2:] if topic_hex.startswith("0x") else topic_hex
    return "0x" + hex_part[-40:]


def fetch_logs_chunk(url: str, contract: str, from_block: int, to_block: int) -> list:
    # Descoberto na pratica: o endpoint da Alchemy rejeita "address" como
    # lista (erro de deserializacao "Variadic"), apesar de a spec JSON-RPC
    # permitir — uma chamada por contrato, nao combinado.
    return rpc_call(url, "eth_getLogs", [{
        "address": contract,
        "topics": [TRANSFER_TOPIC],
        "fromBlock": hex(from_block),
        "toBlock": hex(to_block),
    }])


def main():
    url = load_rpc_url()
    session_start_ms = get_session_start_ms()
    print(f"inicio da sessao de whale watch: ts_ms={session_start_ms}", flush=True)

    latest_block = int(rpc_call(url, "eth_blockNumber", []), 16)
    print(f"bloco mais recente: {latest_block}", flush=True)

    start_block = find_block_at_or_before(url, session_start_ms // 1000, latest_block)
    total_blocks = latest_block - start_block
    n_chunks = (total_blocks // CHUNK_BLOCKS) + 1
    print(f"bloco inicial estimado (busca binaria): {start_block} "
          f"({total_blocks} blocos, ~{n_chunks} chamadas de {CHUNK_BLOCKS} blocos cada)", flush=True)

    labeled_events = []
    total_logs = 0
    total_whale = 0
    skipped_chunks = []

    block = start_block
    chunk_i = 0
    while block <= latest_block:
        to_block = min(block + CHUNK_BLOCKS - 1, latest_block)
        chunk_i += 1
        for contract, token in [(USDT_CONTRACT, "USDT"), (USDC_CONTRACT, "USDC")]:
            logs = None
            for attempt in range(1, MAX_RETRIES + 1):
                try:
                    logs = fetch_logs_chunk(url, contract, block, to_block)
                    break
                except Exception as e:
                    if attempt == MAX_RETRIES:
                        print(f"  aviso: chunk {block}-{to_block} ({token}) falhou {MAX_RETRIES}x ({e}) — pulando", flush=True)
                        skipped_chunks.append((block, to_block, token))
                    else:
                        time.sleep(1.5)
            if logs is not None:
                total_logs += len(logs)
                for log in logs:
                    topics = log.get("topics", [])
                    if len(topics) < 3:
                        continue
                    amount = decode_amount_usd(log.get("data", "0x0"))
                    if amount < MIN_WHALE_USD:
                        continue
                    total_whale += 1
                    from_addr = extract_address(topics[1])
                    to_addr = extract_address(topics[2])
                    from_label = KNOWN_LABELS.get(from_addr.lower())
                    to_label = KNOWN_LABELS.get(to_addr.lower())
                    if not from_label and not to_label:
                        continue
                    block_num = int(log["blockNumber"], 16)
                    labeled_events.append({
                        "block": block_num,
                        "token": token,
                        "amount_usd": amount,
                        "from": from_addr,
                        "to": to_addr,
                        "from_label": from_label,
                        "to_label": to_label,
                        "hypothesis": "short" if to_label else "long",
                        "tx_hash": log.get("transactionHash"),
                    })
            time.sleep(RPC_DELAY_S)
        if chunk_i % 50 == 0:
            print(f"  progresso: {chunk_i}/{n_chunks} janelas de bloco — bloco {block}/{latest_block} — "
                  f"{total_whale} whale-transfers, {len(labeled_events)} rotulados ate agora", flush=True)
        block = to_block + 1

    print(f"\nvarredura concluida ate bloco {latest_block} ({skipped_chunks and len(skipped_chunks) or 0} chunks pulados apos {MAX_RETRIES} tentativas)", flush=True)
    print()
    print(f"total de logs Transfer inspecionados: {total_logs}")
    print(f"total de transfers >= ${MIN_WHALE_USD:,.0f}: {total_whale}")
    print(f"total com endereco rotulado (from OU to): {len(labeled_events)}")
    print()
    for e in labeled_events:
        label = e["to_label"] or e["from_label"]
        direction = "-> deposito em " if e["to_label"] else "<- saida de "
        print(f"  bloco {e['block']}: {e['token']} ${e['amount_usd']:,.0f} {direction}{label} (hipotese: {e['hypothesis']})")

    out_path = Path(__file__).resolve().parent / "reports" / "whale_watch_labeled_transfers.json"
    out_path.parent.mkdir(exist_ok=True)
    out_path.write_text(json.dumps(labeled_events, indent=2), encoding="utf-8")
    print(f"\nsalvo em {out_path}")


if __name__ == "__main__":
    main()
