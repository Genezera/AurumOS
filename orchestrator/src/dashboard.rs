use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::events::{Envelope, EventBus};

const INDEX_HTML: &str = include_str!("../dashboard/index.html");
// Vendorizado localmente (TradingView Lightweight Charts v5.2.0, Apache-2.0)
// para o painel nunca depender de internet em runtime. Injetada inline no
// HTML (em vez de servida numa rota /vendor/... própria) porque bloqueadores
// de anúncio/rastreador do navegador (uBlock, Brave Shields) costumam
// bloquear por heurística de nome qualquer caminho parecido com
// "vendor/*.js" ou "*charts*.js" — inline não dá esse alvo para bloquear.
const LIGHTWEIGHT_CHARTS_JS: &str = include_str!("../dashboard/lightweight-charts.js");
static RENDERED_INDEX: OnceLock<String> = OnceLock::new();

fn rendered_index() -> &'static str {
    RENDERED_INDEX.get_or_init(|| {
        let inline_script = format!("<script>{LIGHTWEIGHT_CHARTS_JS}</script>");
        INDEX_HTML.replacen("<!--CHARTS_LIB_INLINE-->", &inline_script, 1)
    })
}

/// Sobe o servidor do painel em `addr` e nunca retorna enquanto o processo
/// estiver rodando. Roda em sua própria task, independente do loop do
/// orquestrador — se o dashboard cair, o paper trading continua.
pub async fn serve(addr: SocketAddr, bus: EventBus) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler))
        .with_state(bus);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(url = %format!("http://{addr}"), "dashboard disponível");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    Html(rendered_index())
}

async fn ws_handler(ws: WebSocketUpgrade, State(bus): State<EventBus>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, bus))
}

#[derive(Serialize)]
struct BacklogMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    started_at_ms: u64,
    events: &'a [Envelope],
}

async fn handle_socket(mut socket: WebSocket, bus: EventBus) {
    let (backlog, mut rx) = bus.subscribe_with_backlog();
    let last_backlog_seq = backlog.last().map(|e| e.seq);

    let msg = BacklogMessage {
        kind: "backlog",
        started_at_ms: bus.started_at_ms(),
        events: &backlog,
    };
    let payload = match serde_json::to_string(&msg) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "falha ao serializar backlog do dashboard");
            return;
        }
    };
    if socket.send(Message::Text(payload)).await.is_err() {
        return;
    }

    loop {
        match rx.recv().await {
            Ok(env) => {
                if let Some(last) = last_backlog_seq {
                    if env.seq <= last {
                        // já foi enviado como parte do backlog inicial.
                        continue;
                    }
                }
                let payload = match serde_json::to_string(&env) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, "falha ao serializar evento do dashboard");
                        continue;
                    }
                };
                if socket.send(Message::Text(payload)).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::debug!(skipped, "cliente do dashboard ficou para trás, pulando eventos");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
