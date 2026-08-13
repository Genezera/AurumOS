use std::collections::HashMap;
use std::time::Instant;

use chrono::{Datelike, Timelike, Weekday};
use chrono_tz::America::New_York;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

// Feed IEX (gratuito) da Alpaca Markets — dado real de ações US com
// pequeno atraso, sem custo, mediante uma conta paper trading gratuita
// (sem cartão): https://alpaca.markets. Fase 8 do roadmap estava 100%
// bloqueada por decisão de provedor; esta é a recomendação: Alpaca cobre
// dados + execução (paper e real) na mesma conta, o que evita integrar
// dois provedores diferentes depois.
const ALPACA_WS_URL: &str = "wss://stream.data.alpaca.markets/v2/iex";
const WATCHLIST: [&str; 6] = ["AAPL", "MSFT", "SPY", "QQQ", "NVDA", "TSLA"];
// Variação de preço numa janela curta que consideramos digna de nota —
// sem ser um sinal de preço confirmado (mesma regra do resto do sistema).
const MOVE_THRESHOLD_PCT: f64 = 0.005; // 0.5%
const WINDOW: Duration = Duration::from_secs(60);
const EMIT_COOLDOWN: Duration = Duration::from_secs(30);

/// Bolsa aberta (NYSE, horário regular) → silêncio de mais de 30s no feed é
/// sinal real de conexão morta. Fora do horário, o feed IEX simplesmente não
/// tem trade nenhum pra mandar — tratar isso como "morta" fazia o módulo
/// reconectar a cada ~35s a noite inteira, todo santo dia, sem necessidade
/// (achado ao revisar os logs em 12/08/2026). Não considera feriados da
/// bolsa — simplificação aceitável: no pior caso, reconecta cedo demais um
/// punhado de dias por ano, nunca tarde demais.
fn market_hours_read_timeout() -> Duration {
    const OPEN: Duration = Duration::from_secs(30);
    const CLOSED: Duration = Duration::from_secs(20 * 60);

    let now = chrono::Utc::now().with_timezone(&New_York);
    if matches!(now.weekday(), Weekday::Sat | Weekday::Sun) {
        return CLOSED;
    }
    let minute_of_day = now.hour() * 60 + now.minute();
    let market_open = 9 * 60 + 30;
    let market_close = 16 * 60;
    if (market_open..market_close).contains(&minute_of_day) {
        OPEN
    } else {
        CLOSED
    }
}

#[derive(Debug, Clone, Copy)]
struct PricePoint {
    price: f64,
    at: Instant,
}

/// Fase 8 do roadmap: observação de volatilidade em ações líquidas via
/// Alpaca (dado real, gratuito). Assim como Whale Watch/News/Macro, emite
/// `net_edge = 0.0` — visibilidade real, nunca uma ordem sozinha. Ainda não
/// tem lógica de estratégia (o roadmap só pedia "estender a mecânica de
/// scoring/risco para novos Market", que os tipos já suportam desde a Fase
/// 0) nem regras de correlação cross-asset (cripto vs. ações de tech, por
/// exemplo) — isso é o próximo passo depois que houver dado suficiente.
pub struct MultiAssetSource;

#[async_trait::async_trait]
impl SignalSource for MultiAssetSource {
    fn name(&self) -> &'static str {
        "multi_asset_alpaca"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        let key = std::env::var("ALPACA_API_KEY_ID").unwrap_or_default();
        let secret = std::env::var("ALPACA_API_SECRET_KEY").unwrap_or_default();
        if key.is_empty() || secret.is_empty() {
            tracing::info!(
                "multi-asset (Alpaca) desativado — defina ALPACA_API_KEY_ID e ALPACA_API_SECRET_KEY \
                 (conta gratuita, paper trading, sem cartão, em https://alpaca.markets) para ativar. \
                 Isso não pode ser feito pelo assistente — criar conta é passo exclusivo do usuário."
            );
            // Não é um erro — só um módulo opcional sem credenciais ainda.
            // Fica parado sem tentar reconectar em loop por nada.
            std::future::pending::<()>().await;
            return Ok(());
        }

        loop {
            if let Err(e) = run_once(&key, &secret, &tx).await {
                tracing::warn!(error = %e, "conexão com Alpaca caiu, reconectando em 5s");
            }
            sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn run_once(key: &str, secret: &str, tx: &Sender<Opportunity>) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(ALPACA_WS_URL).await?;
    let (mut sink, mut stream) = ws.split();

    sink.send(WsMessage::Text(
        serde_json::json!({ "action": "auth", "key": key, "secret": secret }).to_string(),
    ))
    .await?;
    sink.send(WsMessage::Text(
        serde_json::json!({ "action": "subscribe", "trades": WATCHLIST }).to_string(),
    ))
    .await?;
    tracing::info!(watchlist = ?WATCHLIST, "multi-asset: assinatura de trades enviada à Alpaca");

    let mut history: HashMap<String, Vec<PricePoint>> = HashMap::new();
    let mut last_emitted: HashMap<String, Instant> = HashMap::new();
    let mut last_msg = Instant::now();
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                let timeout = market_hours_read_timeout();
                if last_msg.elapsed() > timeout {
                    anyhow::bail!("nenhuma mensagem em {}s — conexão provavelmente morta", timeout.as_secs());
                }
            }
            msg = stream.next() => {
                last_msg = Instant::now();
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        handle_message(&text, &mut history, &mut last_emitted, tx).await;
                    }
                    Some(Ok(WsMessage::Close(_))) | None => anyhow::bail!("conexão fechada pelo servidor"),
                    Some(Err(e)) => anyhow::bail!("erro no stream: {e}"),
                    _ => {}
                }
            }
        }
    }
}

async fn handle_message(
    text: &str,
    history: &mut HashMap<String, Vec<PricePoint>>,
    last_emitted: &mut HashMap<String, Instant>,
    tx: &Sender<Opportunity>,
) {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else { return };
    for item in items {
        let Some(msg_type) = item.get("T").and_then(Value::as_str) else { continue };
        if msg_type == "error" {
            tracing::warn!(raw = %item, "alpaca retornou erro (checar chaves/plano)");
            continue;
        }
        if msg_type != "t" {
            continue; // só trades ("t"); ignora acks de auth/subscribe etc.
        }
        let Some(symbol) = item.get("S").and_then(Value::as_str) else { continue };
        let Some(price) = item.get("p").and_then(Value::as_f64) else { continue };

        let now = Instant::now();
        let points = history.entry(symbol.to_string()).or_default();
        points.push(PricePoint { price, at: now });
        points.retain(|p| now.duration_since(p.at) <= WINDOW);

        let Some(oldest) = points.first() else { continue };
        if oldest.price <= 0.0 {
            continue;
        }
        let move_pct = (price - oldest.price) / oldest.price;
        if move_pct.abs() < MOVE_THRESHOLD_PCT {
            continue;
        }
        if let Some(t) = last_emitted.get(symbol) {
            if t.elapsed() < EMIT_COOLDOWN {
                continue;
            }
        }
        last_emitted.insert(symbol.to_string(), now);

        tracing::info!(symbol, move_pct = move_pct * 100.0, price, "movimento notável em ação (multi-asset)");

        let opp = Opportunity {
            market: Market::Stocks,
            strategy: Strategy::MultiAsset,
            asset: format!("{symbol} ({:+.2}% em {}s)", move_pct * 100.0, WINDOW.as_secs()),
            direction: if move_pct > 0.0 { Direction::Long } else { Direction::Short },
            net_edge: 0.0,
            confidence: 0.25,
            valid_for_ms: 60_000,
            // Informativo (net_edge=0) — placeholder consistente com valid_for_ms.
            expected_holding_secs: 60.0,
            capital_needed: 10.0,
            max_loss_pct: 0.01,
            leverage: 1.0,
            correlation_group: "us_equities".to_string(),
            sampled_return: None,
            emitted_at: Instant::now(),
        };
        let _ = tx.send(opp).await;
    }
}
