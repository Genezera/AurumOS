//! Busca real de filtros de lote/notional mínimo por símbolo, direto da
//! API pública de cada exchange — achado ao vivo (13/08/2026, pergunta do
//! usuário: "o que falta pra ser 100% real em regras/validações?"): até
//! aqui nenhuma ordem simulada verificava se a exchange de verdade
//! aceitaria aquele tamanho. Buscado uma vez no boot (filtros de lote
//! praticamente não mudam durante uma sessão) e usado por
//! `risk::evaluate` pra rejeitar (`RejectReason::BelowExchangeMinimum`)
//! ordens abaixo do notional mínimo real.
//!
//! Formatos confirmados ao vivo consultando as duas APIs (13/08/2026):
//! Bybit `GET /v5/market/instruments-info?category=spot` →
//! `result.list[].lotSizeFilter.{minOrderQty,minOrderAmt}`; Bitget
//! `GET /api/v2/spot/public/symbols` → `data[].{minTradeAmount,minTradeUSDT}`.

use std::collections::HashMap;

use crate::risk::SymbolFilter;

const BYBIT_INSTRUMENTS_URL: &str = "https://api.bybit.com/v5/market/instruments-info?category=spot";
const BITGET_SYMBOLS_URL: &str = "https://api.bitget.com/api/v2/spot/public/symbols";

pub async fn fetch_bybit_spot_filters() -> HashMap<String, SymbolFilter> {
    let mut out = HashMap::new();
    let resp = match reqwest::get(BYBIT_INSTRUMENTS_URL).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "falha ao buscar filtros de lote da Bybit — validação de notional mínimo fica inativa pra spot Bybit até o próximo boot");
            return out;
        }
    };
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "resposta inválida da Bybit ao buscar filtros de lote");
            return out;
        }
    };
    let Some(list) = v.pointer("/result/list").and_then(|l| l.as_array()) else {
        tracing::warn!("resposta da Bybit sem result.list ao buscar filtros de lote");
        return out;
    };
    for item in list {
        let Some(symbol) = item.get("symbol").and_then(|s| s.as_str()) else { continue };
        let Some(filter) = item.get("lotSizeFilter") else { continue };
        let min_qty = filter.get("minOrderQty").and_then(|s| s.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let min_notional = filter.get("minOrderAmt").and_then(|s| s.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        out.insert(symbol.to_string(), SymbolFilter { min_qty, qty_step: min_qty, min_notional });
    }
    tracing::info!(simbolos = out.len(), "filtros de lote/notional mínimo da Bybit spot carregados (dado real)");
    out
}

pub async fn fetch_bitget_spot_filters() -> HashMap<String, SymbolFilter> {
    let mut out = HashMap::new();
    let resp = match reqwest::get(BITGET_SYMBOLS_URL).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "falha ao buscar filtros de lote da Bitget — validação de notional mínimo fica inativa pra spot Bitget até o próximo boot");
            return out;
        }
    };
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "resposta inválida da Bitget ao buscar filtros de lote");
            return out;
        }
    };
    let Some(list) = v.get("data").and_then(|l| l.as_array()) else {
        tracing::warn!("resposta da Bitget sem data ao buscar filtros de lote");
        return out;
    };
    for item in list {
        let Some(symbol) = item.get("symbol").and_then(|s| s.as_str()) else { continue };
        let min_qty = item.get("minTradeAmount").and_then(|s| s.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let min_notional = item.get("minTradeUSDT").and_then(|s| s.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        out.insert(symbol.to_string(), SymbolFilter { min_qty, qty_step: min_qty, min_notional });
    }
    tracing::info!(simbolos = out.len(), "filtros de lote/notional mínimo da Bitget spot carregados (dado real)");
    out
}
