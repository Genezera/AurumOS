use std::time::Instant;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

// PublicNode oferece WSS público para Ethereum mainnet sem exigir cadastro
// ou chave de API — ponto de partida razoável, usado por padrão. Pra
// escalar (mais confiabilidade, rate limit maior), crie uma conta gratuita
// na Alchemy (https://www.alchemy.com — free tier generoso, sem cartão) e
// exporte AURUMOS_ETH_WS_URL com a URL wss:// que ela te dá; nenhuma
// mudança de código é necessária. Isso não pode ser feito por mim — criar
// contas é uma ação exclusiva do usuário.
fn eth_ws_url() -> String {
    std::env::var("AURUMOS_ETH_WS_URL").unwrap_or_else(|_| "wss://ethereum-rpc.publicnode.com".to_string())
}

// keccak256("Transfer(address,address,uint256)") — assinatura padrão do
// evento ERC-20 Transfer, igual em qualquer token compatível.
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const USDT_CONTRACT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec";
const USDC_CONTRACT: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
// Abaixo disso não tratamos como "movimento de baleia".
const MIN_WHALE_USD: f64 = 250_000.0;

// Rotulagem de carteiras conhecidas (Fase 4 do roadmap: "construída a
// partir de dados públicos, sem inventar identidade"). Cada endereço abaixo
// foi verificado individualmente contra o rótulo público do Etherscan antes
// de entrar aqui — não é uma lista completa (a Etherscan tem centenas de
// endereços rotulados por exchange), é um ponto de partida pequeno e
// conferido. Minúsculas de propósito — comparação sempre via
// eq_ignore_ascii_case, então o case do dado recebido não importa.
//
// Ampliada em 13/08/2026: achado real ao tentar backtestar a hipótese de
// direção (depósito=short/saque=long) — com só 6 endereços, ZERO dos 2.698
// eventos de whale watch em ~18,5h bateram em algum deles (nenhuma exchange
// usa só uma carteira quente; usam centenas). A hipótese não estava "não
// validada", estava impossível de testar por falta de amostra. +5 endereços
// abaixo, cada um conferido individualmente contra o rótulo público do
// Etherscan (endereço bate exatamente, sem ambiguidade) — dois candidatos
// (dois endereços diferentes ambos rotulados "Coinbase 4" em fontes
// distintas) ficaram de fora por não dar pra desambiguar com confiança.
const KNOWN_LABELS: &[(&str, &str)] = &[
    ("0x28c6c06298d514db089934071355e5743bf21d60", "Binance"),
    ("0xf977814e90da44bfa03b6295a0616a897441acec", "Binance"),
    ("0x21a31ee1afc51d94c2efccaa2092ad1028285549", "Binance"),
    ("0xdfd5293d8e347dfe59e90efd55b2956a1343963d", "Binance"),
    ("0x71660c4005ba85c37ccec55d0c4493e66fe775d3", "Coinbase"),
    ("0x2910543af39aba0cd09dbb2d50200b3e800a63d2", "Kraken"),
    ("0xf89d7b9c864f589bbf53a82105107622b35eaa40", "Bybit"),
    ("0x4b4e14a3773ee558b6597070797fd51eb48606e5", "OKX"),
    ("0x4e7b110335511f662fdbb01bf958a7844118c0d4", "OKX"),
    ("0x53f78a071d04224b8e254e243fffc6d9f2f3fa23", "KuCoin"),
    ("0x2b5634c42055806a59e9107ed44d43c426e58258", "KuCoin"),
];

fn known_label(address: &str) -> Option<&'static str> {
    KNOWN_LABELS
        .iter()
        .find(|(addr, _)| addr.eq_ignore_ascii_case(address))
        .map(|(_, label)| *label)
}

/// Fase 4 do roadmap: escuta transferências grandes de USDT/USDC on-chain
/// via subscription JSON-RPC (tempo real, sem polling), com rotulagem de um
/// pequeno conjunto de carteiras de exchange verificadas individualmente
/// contra o Etherscan (`KNOWN_LABELS`) — não uma base completa, um começo
/// conferido. Um depósito grande sozinho não é um sinal de preço confiável
/// — por isso toda `Opportunity` daqui sai com `net_edge = 0.0` de
/// propósito: o score nunca fica positivo e o orquestrador nunca executa
/// isso automaticamente. O valor agora é visibilidade em tempo real no
/// dashboard; virar sinal acionável exige a camada de confirmação por
/// preço/livro que essa fase ainda não tem.
pub struct WhaleWatchSource;

#[async_trait::async_trait]
impl SignalSource for WhaleWatchSource {
    fn name(&self) -> &'static str {
        "whale_watch_eth"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        loop {
            if let Err(e) = run_once(&tx).await {
                tracing::warn!(error = %e, "conexão com nó Ethereum caiu, reconectando em 5s");
            }
            sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn run_once(tx: &Sender<Opportunity>) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(eth_ws_url()).await?;
    let (mut sink, mut stream) = ws.split();

    // Duas assinaturas separadas (uma por contrato) em vez de um único
    // filtro com `address` como array — formato mais conservador/compatível
    // entre implementações de nó, testado depois que o filtro combinado
    // ficou mudo (nenhuma notificação, mesmo com o ack de inscrição ok).
    for (id, contract) in [(1, USDT_CONTRACT), (2, USDC_CONTRACT)] {
        let sub = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "eth_subscribe",
            "params": ["logs", { "address": contract, "topics": [TRANSFER_TOPIC] }]
        });
        futures_util::SinkExt::send(&mut sink, WsMessage::Text(sub.to_string())).await?;
    }
    tracing::info!(chain = "ethereum_mainnet", limiar_usd = MIN_WHALE_USD, "whale watch: assinaturas de Transfer USDT e USDC enviadas");

    // Provedores públicos/anônimos às vezes derrubam a conexão sem enviar
    // um Close frame — sem isso, `stream.next().await` ficaria pendurado
    // para sempre e o loop de reconexão nunca seria acionado. Um timeout
    // por mensagem resolve: mainnet gera transfers de USDT/USDC o tempo
    // todo, então silêncio total por mais de ~25s já é sinal de conexão
    // morta, não de mercado parado.
    const READ_TIMEOUT: Duration = Duration::from_secs(25);

    let mut first_messages_logged = 0;
    loop {
        let next = tokio::time::timeout(READ_TIMEOUT, stream.next()).await;
        let msg = match next {
            Ok(Some(m)) => m,
            Ok(None) => anyhow::bail!("stream do nó encerrou"),
            Err(_) => anyhow::bail!("nenhuma mensagem do nó em {}s — tratando como conexão morta", READ_TIMEOUT.as_secs()),
        };
        match msg? {
            WsMessage::Text(text) => {
                if first_messages_logged < 5 {
                    first_messages_logged += 1;
                    tracing::debug!(raw = %text.chars().take(400).collect::<String>(), "mensagem bruta do nó (diagnóstico)");
                }
                handle_message(&text, tx).await;
            }
            WsMessage::Close(_) => anyhow::bail!("conexão fechada pelo servidor"),
            _ => {}
        }
    }
}

async fn handle_message(text: &str, tx: &Sender<Opportunity>) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(log) = v.get("params").and_then(|p| p.get("result")) else {
        return;
    };
    let Some(address) = log.get("address").and_then(Value::as_str) else {
        return;
    };
    let Some(topics) = log.get("topics").and_then(Value::as_array) else {
        return;
    };
    let Some(data) = log.get("data").and_then(Value::as_str) else {
        return;
    };
    if topics.len() < 3 {
        return;
    }

    let token = if address.eq_ignore_ascii_case(USDT_CONTRACT) {
        "USDT"
    } else if address.eq_ignore_ascii_case(USDC_CONTRACT) {
        "USDC"
    } else {
        return;
    };

    let Some(amount_usd) = decode_amount_usd(data) else {
        return;
    };
    tracing::debug!(token, amount_usd, "transfer recebido (abaixo ou acima do limiar)");
    if amount_usd < MIN_WHALE_USD {
        return;
    }

    let from = extract_address(topics.get(1));
    let to = extract_address(topics.get(2));
    let from_label = known_label(&from);
    let to_label = known_label(&to);

    tracing::info!(token, amount_usd, from = %from, to = %to, ?from_label, ?to_label, "movimento de baleia detectado");

    // Heurística de intenção (Fase 4 do roadmap): dinheiro indo PRA uma
    // exchange rotulada costuma preceder venda; saindo DE uma exchange
    // costuma ser acumulação/retirada. É uma leitura direcional plausível,
    // não validada por backtest — por isso confidence continua igual
    // (0.3) e net_edge continua 0.0, independente do rótulo.
    let (label_note, direction) = match (from_label, to_label) {
        (_, Some(to_ex)) => (format!(" → deposito na {to_ex}"), Direction::Short),
        (Some(from_ex), _) => (format!(" ← saida da {from_ex}"), Direction::Long),
        _ => (String::new(), Direction::Long),
    };

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::WhaleWatch,
        asset: format!("{token} {amount_usd:.0}{label_note}"),
        direction,
        net_edge: 0.0,
        confidence: 0.3,
        valid_for_ms: 60_000,
        // Informativo (net_edge=0) — score não é usado pra decisão real
        // ainda, valor é só placeholder consistente com valid_for_ms.
        expected_holding_secs: 60.0,
        capital_needed: 10.0,
        max_loss_pct: 0.01,
        leverage: 1.0,
        correlation_group: "whale_signal".to_string(),
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}

/// O campo `data` do log é o valor transferido em uint256 (32 bytes hex).
/// USDT e USDC têm 6 casas decimais, então dividimos por 10^6 direto.
fn decode_amount_usd(data: &str) -> Option<f64> {
    let raw = data.strip_prefix("0x")?;
    let hex_tail = if raw.len() > 32 { &raw[raw.len() - 32..] } else { raw };
    let units = u128::from_str_radix(hex_tail, 16).ok()?;
    Some(units as f64 / 1_000_000.0)
}

/// Endereço indexado num topic vem como 32 bytes com zeros à esquerda — os
/// últimos 20 bytes (40 chars hex) são o endereço de verdade.
fn extract_address(topic: Option<&Value>) -> String {
    let Some(t) = topic.and_then(Value::as_str) else {
        return "?".to_string();
    };
    let hex = t.trim_start_matches("0x");
    let addr = if hex.len() >= 40 { &hex[hex.len() - 40..] } else { hex };
    format!("0x{addr}")
}
