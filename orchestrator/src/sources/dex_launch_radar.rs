use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

// Mesma convenção de whale_watch.rs: gratuito por padrão, sem cadastro.
// Exporte AURUMOS_ETH_WS_URL/AURUMOS_ETH_HTTP_URL (ex.: Alchemy) pra
// escalar sem mudar código — criar a conta é passo exclusivo do usuário.
fn eth_ws_url() -> String {
    std::env::var("AURUMOS_ETH_WS_URL").unwrap_or_else(|_| "wss://ethereum-rpc.publicnode.com".to_string())
}
fn eth_http_url() -> String {
    std::env::var("AURUMOS_ETH_HTTP_URL").unwrap_or_else(|_| "https://ethereum-rpc.publicnode.com".to_string())
}

const UNISWAP_V2_FACTORY: &str = "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f";
// keccak256("PairCreated(address,address,address,uint256)")
const PAIR_CREATED_TOPIC: &str = "0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0f";

// keccak256("Transfer(address,address,uint256)") — mesma assinatura padrão
// ERC-20 usada em whale_watch.rs, duplicada aqui (constante, não vale a
// pena acoplar os dois módulos por causa disso).
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const WETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";
const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec";

// Assinaturas de 4 bytes (seletor de função) usadas na checagem heurística.
const SELECTOR_OWNER: &str = "8da5cb5b"; // owner()
const SELECTOR_GET_RESERVES: &str = "0902f1ac"; // getReserves()
const SELECTOR_MINT_HEURISTIC: &str = "40c10f19"; // mint(address,uint256) — padrão OpenZeppelin
const SELECTOR_BALANCE_OF: &str = "70a08231"; // balanceOf(address)
const SELECTOR_TOTAL_SUPPLY: &str = "18160ddd"; // totalSupply()

// Endereços de "queima" — LP token mandado pra cá não pode mais ser
// resgatado por ninguém, é o jeito on-chain verificável de "liquidez
// travada pra sempre" sem depender de confiar num locker de terceiros.
const BURN_ADDRESS_DEAD: &str = "0x000000000000000000000000000000000000dead";
const BURN_ADDRESS_ZERO: &str = "0x0000000000000000000000000000000000000000";

/// Parte do checklist de contrato do roadmap (Fase 5) que o `LaunchRadarSource`
/// original não cobria: em vez de só olhar listagens em exchange centralizada
/// (que já são tokens estabelecidos), este módulo escuta a criação de pares
/// novos na Uniswap V2 em tempo real — o ponto onde um token literalmente
/// "nasce" e pode subir/despencar em minutos, exatamente o padrão que você
/// descreveu.
///
/// Faz 3 checagens reais via chamada direta ao contrato (sem precisar de
/// Etherscan/block explorer pago):
/// 1. `eth_getCode` no token — presença do seletor de `mint()` (heurística:
///    outros padrões de mint não batem essa assinatura exata, então isso
///    reduz falso negativo mas não elimina).
/// 2. `owner()` — se existe e não é endereço zero, alguém ainda tem
///    controle privilegiado do contrato.
/// 3. `getReserves()` do par — liquidez inicial real, não estimada.
///
/// Concentração de holders (top1/top5 %) e % de LP QUEIMADO (nunca
/// "travado" — só detecta envio a endereço de queima, irrecuperável pra
/// sempre; LP depositado num contrato locker de terceiros como Unicrypt ou
/// Team Finance, recuperável após prazo, é uma verificação diferente e não
/// implementada — revisão técnica externa de 13/08/2026 apontou os dois
/// sendo tratados como sinônimo aqui antes, o que estava errado) e candidato a
/// distribuição coordenada/sybil (mesma origem mandando quantias muito
/// parecidas pra 5+ carteiras) também são calculados agora, via
/// eth_getLogs/eth_call no mesmo RPC gratuito — viável sem indexador pago
/// porque o token é recém-lançado (pouco histórico de blocos pra varrer).
/// Deliberadamente NÃO implementado: honeypot e imposto de compra/venda
/// exigiriam simular uma compra+venda de verdade (ou confiar numa
/// heurística de bytecode não confiável o suficiente pra um checklist de
/// segurança — arriscado inventar confiança onde não há); detecção de
/// volume artificial exigiria o topic hash do evento Swap, que não estava
/// disponível pra verificação nesta revisão — melhor não ter o item do que
/// arriscar um hash errado retornando silenciosamente zero resultados. O
/// que ainda falta (carteira do deployer, autoridade de freeze, Solana)
/// segue exigindo um indexador de eventos históricos maior ou um provedor
/// pago — por isso, como os outros módulos de detecção, isso sai com
/// `net_edge = 0.0`: visibilidade real, nunca uma ordem sozinha.
pub struct DexLaunchRadarSource;

#[async_trait::async_trait]
impl SignalSource for DexLaunchRadarSource {
    fn name(&self) -> &'static str {
        "dex_launch_radar_uniswap"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        loop {
            if let Err(e) = run_once(&tx).await {
                tracing::warn!(error = %e, "conexão dex launch radar (nó Ethereum) caiu, reconectando em 5s");
            }
            sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn run_once(tx: &Sender<Opportunity>) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(eth_ws_url()).await?;
    let (mut sink, mut stream) = ws.split();

    let sub = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_subscribe",
        "params": ["logs", { "address": UNISWAP_V2_FACTORY, "topics": [PAIR_CREATED_TOPIC] }]
    });
    futures_util::SinkExt::send(&mut sink, WsMessage::Text(sub.to_string())).await?;
    tracing::info!(chain = "ethereum_mainnet", factory = UNISWAP_V2_FACTORY, "dex launch radar: assinatura de PairCreated enviada");

    let client = reqwest::Client::builder()
        .user_agent("AurumOS-ResearchBot/0.1")
        .timeout(Duration::from_secs(10))
        .build()?;

    const READ_TIMEOUT: Duration = Duration::from_secs(120);
    loop {
        let next = tokio::time::timeout(READ_TIMEOUT, stream.next()).await;
        let msg = match next {
            Ok(Some(m)) => m,
            Ok(None) => anyhow::bail!("stream do nó encerrou"),
            Err(_) => anyhow::bail!("nenhuma mensagem do nó em {}s — tratando como conexão morta", READ_TIMEOUT.as_secs()),
        };
        match msg? {
            WsMessage::Text(text) => handle_message(&text, &client, tx).await,
            WsMessage::Close(_) => anyhow::bail!("conexão fechada pelo servidor"),
            _ => {}
        }
    }
}

async fn handle_message(text: &str, client: &reqwest::Client, tx: &Sender<Opportunity>) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(log) = v.get("params").and_then(|p| p.get("result")) else {
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

    let token0 = extract_address(topics.get(1));
    let token1 = extract_address(topics.get(2));
    // O campo `data` (não indexado) traz o endereço do par recém-criado nos
    // primeiros 32 bytes.
    let Some(pair) = decode_address_from_data(data, 0) else { return };

    let known = [WETH, USDC, USDT];
    let (paired_with, new_token, new_token_is_token0) = if known.iter().any(|k| k.eq_ignore_ascii_case(&token0)) {
        (token0.clone(), token1.clone(), false)
    } else if known.iter().any(|k| k.eq_ignore_ascii_case(&token1)) {
        (token1.clone(), token0.clone(), true)
    } else {
        // Par não envolve WETH/USDC/USDT — ignoramos (par entre dois
        // tokens obscuros raramente é onde o volume de verdade aparece
        // primeiro).
        return;
    };

    let creation_block = log.get("blockNumber").and_then(Value::as_str).and_then(|s| parse_hex_u64(s));
    tracing::info!(new_token, pair, paired_with, ?creation_block, "novo par Uniswap V2 detectado — rodando checklist");

    let checklist = run_checklist(client, &new_token, &pair, new_token_is_token0, &paired_with, creation_block).await;

    let opp = Opportunity {
        market: Market::Crypto,
        strategy: Strategy::Launch,
        asset: format!("{new_token} — {}", checklist.summary()),
        direction: Direction::Long,
        net_edge: 0.0,
        confidence: 0.2,
        valid_for_ms: 300_000,
        // Informativo (net_edge=0) — placeholder consistente com valid_for_ms.
        expected_holding_secs: 300.0,
        capital_needed: 10.0,
        max_loss_pct: 0.02,
        leverage: 1.0,
        correlation_group: "new_listings".to_string(),
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}

struct Checklist {
    has_mint_selector: bool,
    owner: Option<String>,
    liquidity_native: Option<f64>,
    liquidity_symbol: &'static str,
    /// % da supply movimentada (visível via Transfer desde a criação do
    /// par) que está concentrada no maior holder e nos 5 maiores — checklist
    /// item que a v1 do roadmap listava como faltante (exigia indexador
    /// pago). Calculado direto via eth_getLogs no mesmo RPC gratuito, sem
    /// indexador: viável porque um token recém-lançado tem poucos blocos
    /// de histórico. `None` se a consulta falhar ou não houver transfers.
    top_holder_pct: Option<f64>,
    top5_holder_pct: Option<f64>,
    /// % do LP token (o próprio par Uniswap V2 é um ERC-20) que está em
    /// endereço de queima (dEaD ou zero) — a única forma verificável
    /// on-chain de "liquidez travada pra sempre" sem precisar confiar num
    /// contrato locker de terceiros. `None` se a consulta falhar.
    lp_burned_pct: Option<f64>,
    /// Ver `TransferAnalysis::sybil_candidate`.
    sybil_candidate: Option<(String, usize)>,
}

impl Checklist {
    fn summary(&self) -> String {
        let mint = if self.has_mint_selector { "mint: presente" } else { "mint: não detectado" };
        let owner = match &self.owner {
            Some(o) if o == "0x0000000000000000000000000000000000000000" => "owner: renunciado".to_string(),
            Some(o) => format!("owner: ativo ({}...)", &o[..10.min(o.len())]),
            None => "owner: n/d".to_string(),
        };
        let liq = match self.liquidity_native {
            Some(v) => format!("liq inicial: {v:.4} {}", self.liquidity_symbol),
            None => "liq inicial: n/d".to_string(),
        };
        let holders = match (self.top_holder_pct, self.top5_holder_pct) {
            (Some(top1), Some(top5)) => format!("holders: top1 {top1:.1}%, top5 {top5:.1}%"),
            _ => "holders: n/d".to_string(),
        };
        let lp = match self.lp_burned_pct {
            Some(pct) if pct >= 90.0 => format!("LP: {pct:.0}% queimado"),
            Some(pct) => format!("LP: só {pct:.0}% queimado (removível)"),
            None => "LP: n/d".to_string(),
        };
        let sybil = match &self.sybil_candidate {
            Some((source, count)) => format!(", ⚠ possível distribuição coordenada ({count} carteiras via {}...)", &source[..10.min(source.len())]),
            None => String::new(),
        };
        format!("{mint}, {owner}, {liq}, {holders}, {lp}{sybil}")
    }
}

async fn run_checklist(
    client: &reqwest::Client,
    token: &str,
    pair: &str,
    token_is_token0: bool,
    paired_with: &str,
    creation_block: Option<u64>,
) -> Checklist {
    let code = eth_call_raw(client, "eth_getCode", serde_json::json!([token, "latest"])).await;
    let has_mint_selector = code
        .map(|c| c.to_lowercase().contains(SELECTOR_MINT_HEURISTIC))
        .unwrap_or(false);

    let owner_raw = eth_call(client, token, SELECTOR_OWNER).await;
    let owner = owner_raw.map(|r| decode_address_from_data(&r, 0).unwrap_or_default());

    let reserves_raw = eth_call(client, pair, SELECTOR_GET_RESERVES).await;
    let (liquidity_native, liquidity_symbol) = match reserves_raw {
        Some(raw) => {
            let hex = raw.strip_prefix("0x").unwrap_or(&raw);
            // reserve0 ocupa os primeiros 64 chars hex, reserve1 os próximos 64.
            let reserve0 = hex.get(0..64).and_then(parse_hex_u128).unwrap_or(0);
            let reserve1 = hex.get(64..128).and_then(parse_hex_u128).unwrap_or(0);
            let raw_amount = if token_is_token0 { reserve1 } else { reserve0 };
            let (decimals, symbol): (u32, &'static str) = if paired_with.eq_ignore_ascii_case(WETH) {
                (18, "ETH")
            } else {
                (6, if paired_with.eq_ignore_ascii_case(USDT) { "USDT" } else { "USDC" })
            };
            let amount = raw_amount as f64 / 10f64.powi(decimals as i32);
            (Some(amount), symbol)
        }
        None => (None, "?"),
    };

    let transfer_analysis = match creation_block {
        Some(block) => analyze_transfers(client, token, block).await,
        None => None,
    };
    let (top_holder_pct, top5_holder_pct, sybil_candidate) = match transfer_analysis {
        Some(a) => (a.top_holder_pct, a.top5_holder_pct, a.sybil_candidate),
        None => (None, None, None),
    };

    let lp_burned_pct = lp_burned_percentage(client, pair).await;

    Checklist { has_mint_selector, owner, liquidity_native, liquidity_symbol, top_holder_pct, top5_holder_pct, lp_burned_pct, sybil_candidate }
}

/// % do LP token do par (o próprio contrato do par, que segue ERC-20) que
/// está em endereço de queima. `None` se alguma das 3 chamadas falhar —
/// nunca mostra um número inventado.
async fn lp_burned_percentage(client: &reqwest::Client, pair: &str) -> Option<f64> {
    let total_supply_raw = eth_call(client, pair, SELECTOR_TOTAL_SUPPLY).await?;
    let total_supply = parse_hex_u128(total_supply_raw.strip_prefix("0x").unwrap_or(&total_supply_raw))?;
    if total_supply == 0 {
        return None;
    }

    let dead_balance_data = format!("{SELECTOR_BALANCE_OF}000000000000000000000000{}", &BURN_ADDRESS_DEAD[2..]);
    let zero_balance_data = format!("{SELECTOR_BALANCE_OF}000000000000000000000000{}", &BURN_ADDRESS_ZERO[2..]);
    let dead_raw = eth_call_raw(client, "eth_call", serde_json::json!([{ "to": pair, "data": format!("0x{dead_balance_data}") }, "latest"])).await?;
    let zero_raw = eth_call_raw(client, "eth_call", serde_json::json!([{ "to": pair, "data": format!("0x{zero_balance_data}") }, "latest"])).await?;
    let dead_balance = parse_hex_u128(dead_raw.strip_prefix("0x").unwrap_or(&dead_raw)).unwrap_or(0);
    let zero_balance = parse_hex_u128(zero_raw.strip_prefix("0x").unwrap_or(&zero_raw)).unwrap_or(0);

    Some(100.0 * (dead_balance + zero_balance) as f64 / total_supply as f64)
}

struct TransferAnalysis {
    top_holder_pct: Option<f64>,
    top5_holder_pct: Option<f64>,
    /// Endereço de origem que mandou quantia muito parecida (baixa
    /// dispersão) pra 5+ destinatários distintos logo após o lançamento —
    /// candidato a distribuição coordenada (padrão sybil). É uma heurística
    /// sobre o mesmo dado de Transfer já buscado pra concentração de
    /// holders, não uma chamada extra — e é apresentada como CANDIDATO, não
    /// veredito (airdrop legítimo também bate esse padrão).
    sybil_candidate: Option<(String, usize)>,
}

/// Concentração de holders + candidato a distribuição coordenada, ambos via
/// UMA busca de eth_getLogs (todos os Transfer do token desde o bloco de
/// criação do par — viável sem indexador pago justamente porque é
/// recém-lançado: poucos blocos de histórico). Campos `None` quando a
/// consulta falha ou não há dado suficiente — nunca inventa um número.
async fn analyze_transfers(client: &reqwest::Client, token: &str, from_block: u64) -> Option<TransferAnalysis> {
    let params = serde_json::json!([{
        "address": token,
        "topics": [TRANSFER_TOPIC],
        "fromBlock": format!("0x{from_block:x}"),
        "toBlock": "latest",
    }]);
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_getLogs", "params": params });
    let resp = client.post(eth_http_url()).json(&body).send().await.ok()?;
    let json: Value = resp.json().await.ok()?;
    let logs = json.get("result").and_then(Value::as_array)?;

    let mut balances: HashMap<String, i128> = HashMap::new();
    // from -> (to -> quantia recebida) — usa HashMap interno pra já deduplicar
    // destinatários que receberam em mais de uma transferência da mesma origem.
    let mut fan_out: HashMap<String, HashMap<String, i128>> = HashMap::new();

    for log in logs {
        let Some(topics) = log.get("topics").and_then(Value::as_array) else { continue };
        if topics.len() < 3 {
            continue;
        }
        let from = extract_address(topics.get(1));
        let to = extract_address(topics.get(2));
        let Some(data) = log.get("data").and_then(Value::as_str) else { continue };
        let Some(amount) = parse_hex_u128(data.strip_prefix("0x").unwrap_or(data)) else { continue };
        let amount = amount as i128;

        // Zero address representa mint/burn, não é um "holder" de verdade.
        if !from.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000") {
            *balances.entry(from.clone()).or_insert(0) -= amount;
            *fan_out.entry(from).or_default().entry(to.clone()).or_insert(0) += amount;
        }
        if !to.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000") {
            *balances.entry(to).or_insert(0) += amount;
        }
    }

    let mut positive: Vec<i128> = balances.values().copied().filter(|&b| b > 0).collect();
    let (top_holder_pct, top5_holder_pct) = if positive.is_empty() {
        (None, None)
    } else {
        positive.sort_unstable_by(|a, b| b.cmp(a));
        let total: i128 = positive.iter().sum();
        if total <= 0 {
            (None, None)
        } else {
            let top1_pct = 100.0 * positive[0] as f64 / total as f64;
            let top5_sum: i128 = positive.iter().take(5).sum();
            (Some(top1_pct), Some(100.0 * top5_sum as f64 / total as f64))
        }
    };

    let sybil_candidate = fan_out
        .into_iter()
        .filter_map(|(source, recipients)| {
            if recipients.len() < 5 {
                return None;
            }
            let amounts: Vec<f64> = recipients.values().map(|&v| v as f64).collect();
            let mean = amounts.iter().sum::<f64>() / amounts.len() as f64;
            if mean <= 0.0 {
                return None;
            }
            let variance = amounts.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / amounts.len() as f64;
            let coeff_of_variation = variance.sqrt() / mean;
            // Quantias dentro de ~15% umas das outras — dispersão baixa
            // demais pra ser coincidência entre 5+ destinatários distintos.
            (coeff_of_variation < 0.15).then_some((source, recipients.len()))
        })
        .max_by_key(|(_, count)| *count);

    Some(TransferAnalysis { top_holder_pct, top5_holder_pct, sybil_candidate })
}

fn parse_hex_u64(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
}

async fn eth_call(client: &reqwest::Client, to: &str, selector: &str) -> Option<String> {
    eth_call_raw(client, "eth_call", serde_json::json!([{ "to": to, "data": format!("0x{selector}") }, "latest"])).await
}

async fn eth_call_raw(client: &reqwest::Client, method: &str, params: Value) -> Option<String> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp = client.post(eth_http_url()).json(&body).send().await.ok()?;
    let json: Value = resp.json().await.ok()?;
    json.get("result").and_then(Value::as_str).map(String::from)
}

fn extract_address(topic: Option<&Value>) -> String {
    let Some(t) = topic.and_then(Value::as_str) else {
        return "?".to_string();
    };
    let hex = t.trim_start_matches("0x");
    let addr = if hex.len() >= 40 { &hex[hex.len() - 40..] } else { hex };
    format!("0x{addr}")
}

/// Uma palavra ABI de 32 bytes (64 chars hex) cabe folgadamente num u128
/// depois de tirar os zeros à esquerda — reserves de pool são uint112, bem
/// menores que o limite de 128 bits. String vazia (valor zero) vira 0.
fn parse_hex_u128(word: &str) -> Option<u128> {
    let trimmed = word.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some(0);
    }
    u128::from_str_radix(trimmed, 16).ok()
}

/// Extrai um endereço (últimos 20 bytes de uma palavra de 32 bytes) de um
/// blob de dados ABI-encoded, na posição da palavra `word_index` (0 = primeiros
/// 32 bytes, 1 = próximos 32, etc).
fn decode_address_from_data(data: &str, word_index: usize) -> Option<String> {
    let hex = data.strip_prefix("0x").unwrap_or(data);
    let start = word_index * 64;
    let word = hex.get(start..start + 64)?;
    let addr = &word[word.len() - 40..];
    Some(format!("0x{addr}"))
}
