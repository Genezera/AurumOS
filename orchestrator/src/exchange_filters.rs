//! Filtros de ordem dos perpétuos USDT da única venue executora.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::risk::SymbolFilter;

const BYBIT_LINEAR_INSTRUMENTS_URL: &str =
    "https://api.bybit.com/v5/market/instruments-info?category=linear&status=Trading&limit=1000";
const BYBIT_FEE_GROUPS_URL: &str =
    "https://api.bybit.com/v5/market/fee-group-info?productType=contract";
const STANDARD_CRYPTO_TAKER_FEE: f64 = 0.00055;
const PRE_LISTING_TAKER_FEE: f64 = 0.00100;
const INNOVATION_TAKER_FEE: f64 = 0.00110;

#[derive(Debug, Serialize)]
struct FeeClassificationReport<'a> {
    schema: &'static str,
    generated_at_ms: u64,
    source: &'static str,
    note: &'static str,
    symbols: &'a HashMap<String, f64>,
}

fn parse_crypto_taker_fees(value: &Value) -> HashMap<String, f64> {
    let mut rates = HashMap::new();
    let Some(groups) = value.pointer("/result/list").and_then(Value::as_array) else {
        return rates;
    };
    for group in groups {
        let Some(group_name) = group.get("groupName").and_then(Value::as_str) else {
            continue;
        };
        let rate = match group_name {
            "Major Coins" | "Altcoin" => STANDARD_CRYPTO_TAKER_FEE,
            "Innovation-Zone" => INNOVATION_TAKER_FEE,
            "Pre-listing" => PRE_LISTING_TAKER_FEE,
            // TradFi tem microestrutura, horários e custos próprios. Ele não
            // pertence ao universo cripto do AurumOS e fica de fora do mapa.
            _ => continue,
        };
        let Some(symbols) = group.get("symbols").and_then(Value::as_array) else {
            continue;
        };
        for symbol in symbols.iter().filter_map(Value::as_str) {
            if symbol.ends_with("USDT") {
                rates.insert(symbol.to_string(), rate);
            }
        }
    }
    rates
}

pub async fn fetch_bybit_crypto_taker_fees() -> HashMap<String, f64> {
    let response = match reqwest::get(BYBIT_FEE_GROUPS_URL).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "falha ao buscar classificação pública de taxas da Bybit");
            return HashMap::new();
        }
    };
    let value: Value = match response.json().await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "resposta inválida da classificação de taxas da Bybit");
            return HashMap::new();
        }
    };
    let rates = parse_crypto_taker_fees(&value);
    if rates.is_empty() {
        tracing::warn!("classificação pública de taxas da Bybit veio vazia");
        return rates;
    }
    let report = FeeClassificationReport {
        schema: "aurumos.bybit.public_crypto_fee_classification.v1",
        generated_at_ms: crate::raw_log::now_ms(),
        source: BYBIT_FEE_GROUPS_URL,
        note: "VIP0 público: Major/Altcoin 0,055%; Pre-listing 0,10%; Innovation 0,11%; TradFi excluído",
        symbols: &rates,
    };
    let path = format!(
        "{}/data/bybit_public_crypto_fee_classification_v1.json",
        env!("CARGO_MANIFEST_DIR")
    );
    if let Ok(json) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::create_dir_all(format!("{}/data", env!("CARGO_MANIFEST_DIR")));
        let _ = std::fs::write(path, json);
    }
    tracing::info!(
        symbols = rates.len(),
        "classificação pública de taxas cripto da Bybit carregada"
    );
    rates
}

pub async fn fetch_bybit_linear_filters() -> HashMap<String, SymbolFilter> {
    let mut out = HashMap::new();
    let resp = match reqwest::get(BYBIT_LINEAR_INSTRUMENTS_URL).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "falha ao buscar filtros dos perpétuos Bybit");
            return out;
        }
    };
    let value: serde_json::Value = match resp.json().await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "resposta inválida dos filtros lineares Bybit");
            return out;
        }
    };
    let Some(list) = value
        .pointer("/result/list")
        .and_then(|items| items.as_array())
    else {
        tracing::warn!("resposta dos filtros lineares Bybit não contém result.list");
        return out;
    };
    for item in list {
        let Some(symbol) = item.get("symbol").and_then(|item| item.as_str()) else {
            continue;
        };
        let Some(filter) = item.get("lotSizeFilter") else {
            continue;
        };
        // `unwrap_or(0.0)` é o fallback certo pro chamador (0.0 já faz o
        // risk engine falhar fechado — ver `evaluate()`), mas se a Bybit
        // um dia remover/renomear um destes campos isso ficaria silencioso
        // sem o log abaixo, escondendo que a trava passou a valer pra
        // TODOS os símbolos, não só pra um caso raro.
        let parse_lot = |key: &str| {
            let value = filter
                .get(key)
                .and_then(|item| item.as_str())
                .and_then(|item| item.parse::<f64>().ok());
            if value.is_none() {
                tracing::warn!(
                    symbol,
                    key,
                    "campo de lotSizeFilter ausente ou não numérico na Bybit — filtro deste símbolo cai pra 0.0"
                );
            }
            value.unwrap_or(0.0)
        };
        let price_tick = item
            .pointer("/priceFilter/tickSize")
            .and_then(|item| item.as_str())
            .and_then(|item| item.parse::<f64>().ok())
            .unwrap_or_else(|| {
                tracing::warn!(
                    symbol,
                    "priceFilter.tickSize ausente ou não numérico na Bybit — filtro deste símbolo cai pra 0.0"
                );
                0.0
            });
        out.insert(
            symbol.to_string(),
            SymbolFilter {
                min_qty: parse_lot("minOrderQty"),
                qty_step: parse_lot("qtyStep"),
                min_notional: parse_lot("minNotionalValue"),
                price_tick,
            },
        );
    }
    tracing::info!(symbols = out.len(), "filtros de perpétuos Bybit carregados");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_groups_apply_special_crypto_rates_and_exclude_tradfi() {
        let value = serde_json::json!({"result":{"list":[
            {"groupName":"Major Coins","symbols":["BTCUSDT"]},
            {"groupName":"Altcoin","symbols":["ARBUSDT","XPLPERP"]},
            {"groupName":"Innovation-Zone","symbols":["NEWUSDT"]},
            {"groupName":"Pre-listing","symbols":["SOONUSDT"]},
            {"groupName":"TradFi","symbols":["SPYUSDT"]}
        ]}});
        let rates = parse_crypto_taker_fees(&value);
        assert_eq!(rates.get("BTCUSDT"), Some(&STANDARD_CRYPTO_TAKER_FEE));
        assert_eq!(rates.get("ARBUSDT"), Some(&STANDARD_CRYPTO_TAKER_FEE));
        assert_eq!(rates.get("NEWUSDT"), Some(&INNOVATION_TAKER_FEE));
        assert_eq!(rates.get("SOONUSDT"), Some(&PRE_LISTING_TAKER_FEE));
        assert!(!rates.contains_key("SPYUSDT"));
        assert!(!rates.contains_key("XPLPERP"));
    }
}
