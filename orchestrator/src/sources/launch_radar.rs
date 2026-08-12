use std::collections::HashSet;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::{sleep, Duration};

use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

const INSTRUMENTS_URL: &str = "https://api.bybit.com/v5/market/instruments-info?category=spot&limit=1000";
const POLL_INTERVAL: Duration = Duration::from_secs(90);

/// Fase 5 do roadmap, com escopo reduzido de propósito: detecta pares novos
/// aparecendo no spot da Bybit via polling do endpoint público
/// `instruments-info` (sem chave de API). O PDF descreve uma análise bem
/// mais profunda — distribuição de holders, permissões de mint/freeze do
/// contrato, carteira do deployer — que exige um provedor de block explorer
/// (Etherscan/BscScan/etc., decisão pendente do roadmap por causa de chave
/// de API e custo). Sem isso, fazer essa análise seria fingir uma
/// verificação que não está acontecendo de verdade. Por enquanto: apenas
/// visibilidade real de "isso acabou de ser listado" (`net_edge = 0.0`,
/// mesmo padrão do Whale Watch/News/Macro) — o checklist completo de
/// segurança do token fica para quando essa decisão for tomada.
pub struct LaunchRadarSource;

#[async_trait::async_trait]
impl SignalSource for LaunchRadarSource {
    fn name(&self) -> &'static str {
        "launch_radar_bybit"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .user_agent("AurumOS-ResearchBot/0.1")
            .timeout(Duration::from_secs(15))
            .build()?;

        let mut known: HashSet<String> = HashSet::new();
        let mut first_poll = true;

        loop {
            match poll_symbols(&client).await {
                Ok(symbols) => {
                    let mut new_symbols = Vec::new();
                    for s in &symbols {
                        if !known.contains(s) {
                            new_symbols.push(s.clone());
                        }
                    }
                    known = symbols;

                    if first_poll {
                        tracing::info!(baseline = known.len(), "launch radar: linha de base de pares spot da Bybit estabelecida");
                        first_poll = false;
                    } else {
                        for symbol in new_symbols {
                            emit(&tx, &symbol).await;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "falha ao consultar instruments-info da Bybit, tentando de novo");
                }
            }
            sleep(POLL_INTERVAL).await;
        }
    }
}

async fn poll_symbols(client: &reqwest::Client) -> anyhow::Result<HashSet<String>> {
    let resp = client.get(INSTRUMENTS_URL).send().await?.error_for_status()?;
    let body: Value = resp.json().await?;
    let list = body
        .get("result")
        .and_then(|r| r.get("list"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let symbols = list
        .iter()
        .filter_map(|item| item.get("symbol").and_then(Value::as_str))
        .map(String::from)
        .collect();
    Ok(symbols)
}

async fn emit(tx: &Sender<Opportunity>, symbol: &str) {
    tracing::info!(symbol, "novo par spot detectado na Bybit");

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::Launch,
        asset: symbol.to_string(),
        direction: Direction::Long,
        net_edge: 0.0,
        confidence: 0.2,
        valid_for_ms: 300_000,
        // Informativo (net_edge=0) — placeholder consistente com valid_for_ms.
        expected_holding_secs: 300.0,
        capital_needed: 10.0,
        max_loss_pct: 0.02,
        // Nunca alavancagem em lançamento — livro/liquidez ainda não são
        // confiáveis logo na abertura (mesma regra do risk.toml).
        leverage: 1.0,
        correlation_group: "new_listings".to_string(),
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}
