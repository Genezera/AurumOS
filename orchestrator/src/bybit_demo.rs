//! Adaptador de execução restrito ao ambiente Demo da Bybit.
//!
//! O hostname é constante e não pode ser trocado por configuração. O
//! adaptador usa ordens market com tolerância de slippage, fecha apenas com
//! `reduceOnly` e calcula o resultado pelos fills retornados pela venue.
//!
//! Também observa rate limit e rejeições da API privada (`X-Bapi-Limit*`,
//! retCode 10006, HTTP 403) e persiste um resumo em
//! `data/bybit_demo_api_health_v1.json` — puramente diagnóstico; nada aqui
//! influencia ordem, posição, risco ou PnL.

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::types::{Direction, ExecutionReport, ExecutionRequest, ExecutionStatus};

const DEMO_REST_BASE: &str = "https://api-demo.bybit.com";
const RECV_WINDOW: &str = "5000";
const MAX_SLIPPAGE_PERCENT: &str = "0.50";
const FILL_POLLS: usize = 24;
const FILL_POLL_INTERVAL: Duration = Duration::from_millis(125);
const CLOSE_ATTEMPTS: usize = 3;
/// retCode oficial da Bybit V5 pra "Too many visits!" (rate limit por UID).
/// https://bybit-exchange.github.io/docs/v5/rate-limit
const RATE_LIMIT_RETCODE: i64 = 10_006;
/// Avisa quando o restante do rate limit cai pra 1/5 (20%) ou menos do teto.
const LOW_REMAINING_WARN_DIVISOR: u32 = 5;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct BybitDemoClient {
    http: reqwest::Client,
    api_key: Arc<str>,
    api_secret: Arc<[u8]>,
    clock_offset_ms: Arc<AtomicI64>,
    api_health: Arc<Mutex<ApiHealthCounters>>,
}

#[derive(Debug, Clone, Default)]
struct FillSummary {
    qty: f64,
    value: f64,
    fee: f64,
    executions: usize,
}

impl FillSummary {
    fn add(&mut self, other: &Self) {
        self.qty += other.qty;
        self.value += other.value;
        self.fee += other.fee;
        self.executions += other.executions;
    }
}

/// Observabilidade pura da API privada Demo — nunca influencia ordem,
/// posição, risco ou PnL. Só ajuda o operador a enxergar rate limit e
/// rejeições durante uma campanha ao vivo.
#[derive(Debug, Clone, Default, Serialize)]
struct ApiHealthCounters {
    calls_with_rate_limit_headers: u64,
    last_limit: Option<u32>,
    last_remaining: Option<u32>,
    last_reset_ms: Option<u64>,
    min_remaining_seen: Option<u32>,
    rate_limit_rejections_total: u64,
    ip_throttle_rejections_total: u64,
    other_private_rejections_total: u64,
    last_rejection_retcode: Option<i64>,
    last_rejection_message: Option<String>,
}

impl ApiHealthCounters {
    /// Atualiza a partir dos headers de uma resposta. Retorna `false` (sem
    /// efeito) quando nenhum dos três headers de rate limit está presente —
    /// por exemplo, num endpoint público sem chave.
    fn record_headers(&mut self, headers: &HeaderMap) -> bool {
        let limit = header_u32(headers, "x-bapi-limit");
        let remaining = header_u32(headers, "x-bapi-limit-status");
        let reset_ms = header_u64(headers, "x-bapi-limit-reset-timestamp");
        if limit.is_none() && remaining.is_none() && reset_ms.is_none() {
            return false;
        }
        self.calls_with_rate_limit_headers += 1;
        if let Some(value) = limit {
            self.last_limit = Some(value);
        }
        if let Some(value) = remaining {
            self.last_remaining = Some(value);
            self.min_remaining_seen = Some(match self.min_remaining_seen {
                Some(current) => current.min(value),
                None => value,
            });
        }
        if let Some(value) = reset_ms {
            self.last_reset_ms = Some(value);
        }
        true
    }

    fn record_rejection(&mut self, retcode: i64, message: &str) {
        self.last_rejection_retcode = Some(retcode);
        self.last_rejection_message = Some(message.to_string());
        if retcode == RATE_LIMIT_RETCODE {
            self.rate_limit_rejections_total += 1;
        } else {
            self.other_private_rejections_total += 1;
        }
    }

    fn record_ip_throttle(&mut self) {
        self.ip_throttle_rejections_total += 1;
    }

    /// `true` quando o restante mais recente já caiu pra `1/divisor` (ou
    /// menos) do limite mais recente conhecido.
    fn near_limit(&self, divisor: u32) -> bool {
        match (self.last_remaining, self.last_limit) {
            (Some(remaining), Some(limit)) if limit > 0 => {
                remaining.saturating_mul(divisor) <= limit
            }
            _ => false,
        }
    }

    /// `true` quando esta atualização merece um snapshot novo em disco:
    /// a primeira observação (garante que o arquivo existe pro operador
    /// checar) ou perto do teto — nunca toda chamada de rotina, já que um
    /// único round-trip de trade faz 20+ chamadas autenticadas.
    fn worth_persisting(&self, just_updated_headers: bool, near_limit_divisor: u32) -> bool {
        let first_observation = just_updated_headers && self.calls_with_rate_limit_headers == 1;
        first_observation || self.near_limit(near_limit_divisor)
    }
}

fn header_u32(headers: &HeaderMap, name: &str) -> Option<u32> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

#[derive(Debug, Serialize)]
struct ApiHealthReport {
    schema: &'static str,
    generated_at_ms: u64,
    note: &'static str,
    #[serde(flatten)]
    counters: ApiHealthCounters,
}

fn persist_api_health(counters: &ApiHealthCounters) {
    let report = ApiHealthReport {
        schema: "aurumos.bybit_demo.api_health.v1",
        generated_at_ms: crate::raw_log::now_ms(),
        note: "observabilidade pura da API privada Demo; nunca altera ordem, posição, risco ou PnL",
        counters: counters.clone(),
    };
    let path = format!(
        "{}/data/bybit_demo_api_health_v1.json",
        env!("CARGO_MANIFEST_DIR")
    );
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::write(path, json);
    }
}

impl BybitDemoClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = std::env::var("BYBIT_DEMO_API_KEY")
            .map_err(|_| anyhow::anyhow!("BYBIT_DEMO_API_KEY ausente para modo demo"))?;
        let api_secret = std::env::var("BYBIT_DEMO_API_SECRET")
            .map_err(|_| anyhow::anyhow!("BYBIT_DEMO_API_SECRET ausente para modo demo"))?;
        if api_key.trim().is_empty() || api_secret.trim().is_empty() {
            anyhow::bail!("credenciais Demo da Bybit não podem estar vazias");
        }
        let http = reqwest::Client::builder()
            .user_agent("AurumOS-DemoExecutor/0.2")
            .timeout(Duration::from_secs(6))
            .build()?;
        Ok(Self {
            http,
            api_key: Arc::from(api_key),
            api_secret: Arc::from(api_secret.into_bytes()),
            clock_offset_ms: Arc::new(AtomicI64::new(0)),
            api_health: Arc::new(Mutex::new(ApiHealthCounters::default())),
        })
    }

    /// Confere relógio, saldo e conta limpa. Uma conta Demo dedicada evita
    /// que `reduceOnly` interaja com posições manuais ou de outro robô.
    pub async fn preflight(&self, required_usd: f64) -> anyhow::Result<()> {
        self.sync_clock().await?;

        let wallet = self
            .get(
                "/v5/account/wallet-balance",
                "accountType=UNIFIED&coin=USDT",
            )
            .await?;
        let available = parse_number(wallet.pointer("/result/list/0/totalAvailableBalance"))
            .ok_or_else(|| anyhow::anyhow!("saldo disponível ausente na resposta Demo"))?;
        if available + 1e-9 < required_usd {
            anyhow::bail!(
                "saldo Demo insuficiente: disponível US${available:.2}, necessário US${required_usd:.2}"
            );
        }

        let positions = self
            .get(
                "/v5/position/list",
                "category=linear&settleCoin=USDT&limit=200",
            )
            .await?;
        let open_positions: Vec<String> = list_at(&positions)
            .iter()
            .filter(|item| parse_number(item.get("size")).unwrap_or(0.0) > 0.0)
            .filter_map(|item| {
                item.get("symbol")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        if !open_positions.is_empty() {
            anyhow::bail!(
                "preflight Demo recusado: existem posições abertas: {}",
                open_positions.join(", ")
            );
        }

        let orders = self
            .get(
                "/v5/order/realtime",
                "category=linear&settleCoin=USDT&openOnly=0&limit=50",
            )
            .await?;
        if !list_at(&orders).is_empty() {
            anyhow::bail!("preflight Demo recusado: existem ordens lineares abertas");
        }

        tracing::info!(
            available_usd = available,
            local_capital_limit = required_usd,
            endpoint = DEMO_REST_BASE,
            "preflight da conta Bybit Demo aprovado"
        );
        Ok(())
    }

    pub async fn run(self, mut rx: Receiver<ExecutionRequest>, report_tx: Sender<ExecutionReport>) {
        while let Some(request) = rx.recv().await {
            let client = self.clone();
            let reports = report_tx.clone();
            tokio::spawn(async move {
                if let Err(error) = client.execute_round_trip(request.clone(), &reports).await {
                    tracing::error!(
                        signal_id = request.signal_id,
                        symbol = %request.symbol,
                        %error,
                        "falha no ciclo Bybit Demo"
                    );
                    let _ = reports
                        .send(ExecutionReport {
                            signal_id: request.signal_id,
                            status: ExecutionStatus::OpenRisk,
                            return_pct: 0.0,
                            realized_pnl: None,
                            max_executable_notional: 0.0,
                            observed_latency_ms: 0,
                            note: format!("erro_demo_requer_reconciliacao: {error}"),
                        })
                        .await;
                }
            });
        }
    }

    async fn execute_round_trip(
        &self,
        request: ExecutionRequest,
        report_tx: &Sender<ExecutionReport>,
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        if let Err(error) = self.sync_clock().await {
            send_unfilled(
                report_tx,
                &request,
                &started,
                format!("bybit_demo_pre_entrada_relogio: {error}"),
            )
            .await;
            return Ok(());
        }
        let started_at_ms = self.timestamp().saturating_sub(1_000);
        let entry_link = format!("aur-{}-e", request.signal_id);
        let entry_side = side_for(request.direction);
        let quantity = format_quantity(request.quantity);

        if let Err(error) = self.prepare_symbol(&request.symbol).await {
            send_unfilled(
                report_tx,
                &request,
                &started,
                format!("bybit_demo_pre_entrada_configuracao: {error}"),
            )
            .await;
            return Ok(());
        }

        if let Err(error) = self
            .place_market(&request.symbol, entry_side, &quantity, &entry_link, false)
            .await
        {
            // Um timeout pode acontecer depois de a venue aceitar a ordem.
            // Consultar pelo mesmo ID idempotente antes de declarar unfilled.
            tracing::warn!(signal_id = request.signal_id, %error, "ack da entrada Demo falhou; reconciliando pelo orderLinkId");
        }

        let entry = self.poll_fills(&entry_link, request.quantity).await?;
        if entry.qty <= 0.0 || entry.value <= 0.0 {
            // `/v5/execution/list` pode não ter propagado o fill ainda mesmo
            // que a entrada já tenha sido aceita e preenchida na venue.
            // Declarar "unfilled" aqui liberaria a reserva de risco e
            // abandonaria uma posição possivelmente real sem stop e sem
            // nenhum acompanhamento. Só é seguro tratar como unfilled
            // quando a posição na venue confirma zero; erro de rede na
            // checagem também vira risco aberto, não unfilled.
            let confirmed_flat = matches!(
                self.position_size(&request.symbol).await,
                Ok(size) if size <= quantity_tolerance(request.quantity)
            );
            if !confirmed_flat {
                let _ = report_tx
                    .send(ExecutionReport {
                        signal_id: request.signal_id,
                        status: ExecutionStatus::OpenRisk,
                        return_pct: 0.0,
                        realized_pnl: None,
                        max_executable_notional: 0.0,
                        observed_latency_ms: started.elapsed().as_millis() as u64,
                        note: "bybit_demo_entrada_sem_fill_no_execution_list_mas_posicao_incerta"
                            .to_string(),
                    })
                    .await;
                return Ok(());
            }
            send_unfilled(
                report_tx,
                &request,
                &started,
                "bybit_demo_entrada_sem_fill".to_string(),
            )
            .await;
            return Ok(());
        }

        let entry_average = entry.value / entry.qty;
        let stop_price = protective_stop_price(
            request.direction,
            entry_average,
            request.stop_loss_pct,
            request.price_tick,
        )?;
        let stop_installed = match self.set_stop_loss(&request.symbol, &stop_price).await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(
                    signal_id = request.signal_id,
                    %error,
                    "stop protetor Demo não foi confirmado; fechando imediatamente"
                );
                false
            }
        };
        if stop_installed {
            tokio::time::sleep(Duration::from_secs_f64(
                request.expected_holding_secs.max(0.05),
            ))
            .await;
        }

        let mut client_exit = FillSummary::default();
        for attempt in 0..CLOSE_ATTEMPTS {
            let remaining = self.position_size(&request.symbol).await?;
            if remaining <= quantity_tolerance(request.quantity) {
                break;
            }
            let close_link = format!("aur-{}-x{}", request.signal_id, attempt + 1);
            let close_qty = format_quantity(remaining);
            if let Err(error) = self
                .place_market(
                    &request.symbol,
                    opposite_side(entry_side),
                    &close_qty,
                    &close_link,
                    true,
                )
                .await
            {
                tracing::warn!(signal_id = request.signal_id, attempt, %error, "ack do fechamento Demo falhou; reconciliando");
            }
            let closed = self.poll_fills(&close_link, remaining).await?;
            client_exit.add(&closed);
        }

        let residual = self.position_size(&request.symbol).await?;
        if residual > quantity_tolerance(request.quantity) {
            let _ = report_tx
                .send(ExecutionReport {
                    signal_id: request.signal_id,
                    status: ExecutionStatus::OpenRisk,
                    return_pct: 0.0,
                    realized_pnl: None,
                    max_executable_notional: entry.value,
                    observed_latency_ms: started.elapsed().as_millis() as u64,
                    note: format!("bybit_demo_posicao_residual_qty={residual}"),
                })
                .await;
            return Ok(());
        }

        // Inclui tanto nossos fechamentos `reduceOnly` quanto um stop
        // protetor que tenha disparado durante a janela. A posição já foi
        // confirmada zerada na venue (checado acima); o que pode faltar
        // aqui é só o `/v5/execution/list` propagar os fills de saída —
        // tenta de novo antes de desistir, com o mesmo orçamento do poll
        // de entrada, em vez de declarar risco aberto na primeira leitura
        // vazia.
        let mut exit = FillSummary::default();
        for _ in 0..FILL_POLLS {
            exit = self
                .fills_for_side_since(&request.symbol, opposite_side(entry_side), started_at_ms)
                .await?;
            if exit.qty > 0.0 && exit.value > 0.0 {
                break;
            }
            tokio::time::sleep(FILL_POLL_INTERVAL).await;
        }
        if exit.qty <= 0.0 || exit.value <= 0.0 {
            // A posição está confirmadamente flat na venue — isto não é
            // risco aberto de exposição de verdade, é PnL desconhecido de
            // uma posição já fechada. Reporta com o notional real de
            // entrada (nunca zero) pra o dashboard não subestimar o que
            // estava em jogo, e usa uma nota específica em vez de cair no
            // handler genérico de erro em `run()`, que sempre reporta
            // notional zero.
            let _ = report_tx
                .send(ExecutionReport {
                    signal_id: request.signal_id,
                    status: ExecutionStatus::OpenRisk,
                    return_pct: 0.0,
                    realized_pnl: None,
                    max_executable_notional: entry.value,
                    observed_latency_ms: started.elapsed().as_millis() as u64,
                    note: "bybit_demo_posicao_flat_sem_fills_de_saida_reconciliaveis".to_string(),
                })
                .await;
            return Ok(());
        }

        let pnl = realized_pnl(request.direction, &entry, &exit);
        let return_pct = pnl / entry.value;
        let total_fee = entry.fee + exit.fee;
        let _ = report_tx
            .send(ExecutionReport {
                signal_id: request.signal_id,
                status: ExecutionStatus::Filled,
                return_pct,
                realized_pnl: Some(pnl),
                max_executable_notional: entry.value,
                observed_latency_ms: started.elapsed().as_millis() as u64,
                note: format!(
                    "bybit_demo_fills entrada={} saida={} fee_usdt={total_fee:.8} stop_protetor={stop_installed}",
                    entry.executions, exit.executions,
                ),
            })
            .await;
        Ok(())
    }

    async fn sync_clock(&self) -> anyhow::Result<()> {
        let before = local_now_ms();
        let response = self
            .http
            .get(format!("{DEMO_REST_BASE}/v5/market/time"))
            .send()
            .await?;
        let response = self.receive(response).await?;
        ensure_success(&response)?;
        let after = local_now_ms();
        let server = response
            .get("time")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("server time ausente na resposta Demo"))?;
        let midpoint = before.saturating_add(after).saturating_div(2);
        self.clock_offset_ms
            .store(server.saturating_sub(midpoint), Ordering::Relaxed);
        Ok(())
    }

    async fn place_market(
        &self,
        symbol: &str,
        side: &str,
        qty: &str,
        order_link_id: &str,
        reduce_only: bool,
    ) -> anyhow::Result<Value> {
        let body = serde_json::json!({
            "category": "linear",
            "symbol": symbol,
            "side": side,
            "orderType": "Market",
            "qty": qty,
            "positionIdx": 0,
            "orderLinkId": order_link_id,
            "reduceOnly": reduce_only,
            "slippageToleranceType": "Percent",
            "slippageTolerance": MAX_SLIPPAGE_PERCENT,
        });
        self.post("/v5/order/create", &body).await
    }

    async fn prepare_symbol(&self, symbol: &str) -> anyhow::Result<()> {
        let position_mode = serde_json::json!({
            "category": "linear",
            "symbol": symbol,
            "mode": 0,
        });
        self.post_allow_codes("/v5/position/switch-mode", &position_mode, &[110025])
            .await?;

        let leverage = serde_json::json!({
            "category": "linear",
            "symbol": symbol,
            "buyLeverage": "1",
            "sellLeverage": "1",
        });
        self.post_allow_codes("/v5/position/set-leverage", &leverage, &[110043])
            .await?;
        Ok(())
    }

    async fn set_stop_loss(&self, symbol: &str, stop_loss: &str) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "category": "linear",
            "symbol": symbol,
            "tpslMode": "Full",
            "positionIdx": 0,
            "stopLoss": stop_loss,
            "slTriggerBy": "MarkPrice",
            "slOrderType": "Market",
        });
        self.post("/v5/position/trading-stop", &body).await?;
        Ok(())
    }

    async fn poll_fills(
        &self,
        order_link_id: &str,
        desired_qty: f64,
    ) -> anyhow::Result<FillSummary> {
        let query = format!("category=linear&orderLinkId={order_link_id}&limit=100");
        let mut latest = FillSummary::default();
        for _ in 0..FILL_POLLS {
            let value = self.get("/v5/execution/list", &query).await?;
            latest = parse_fills(&value);
            if latest.qty + quantity_tolerance(desired_qty) >= desired_qty {
                break;
            }
            tokio::time::sleep(FILL_POLL_INTERVAL).await;
        }
        Ok(latest)
    }

    async fn position_size(&self, symbol: &str) -> anyhow::Result<f64> {
        let query = format!("category=linear&symbol={symbol}");
        let value = self.get("/v5/position/list", &query).await?;
        Ok(list_at(&value)
            .iter()
            .map(|item| parse_number(item.get("size")).unwrap_or(0.0))
            .sum())
    }

    async fn fills_for_side_since(
        &self,
        symbol: &str,
        side: &str,
        started_at_ms: i64,
    ) -> anyhow::Result<FillSummary> {
        let query = format!("category=linear&symbol={symbol}&startTime={started_at_ms}&limit=100");
        let value = self.get("/v5/execution/list", &query).await?;
        Ok(parse_fills_for_side(&value, Some(side)))
    }

    async fn get(&self, path: &str, query: &str) -> anyhow::Result<Value> {
        let timestamp = self.timestamp();
        let payload = format!("{timestamp}{}{RECV_WINDOW}{query}", self.api_key);
        let signature = sign_hex(&self.api_secret, payload.as_bytes())?;
        let response = self
            .http
            .get(format!("{DEMO_REST_BASE}{path}?{query}"))
            .headers(self.auth_headers(timestamp, &signature)?)
            .send()
            .await?;
        let response = self.receive(response).await?;
        self.check_response(&response, &[])?;
        Ok(response)
    }

    async fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        self.post_allow_codes(path, body, &[]).await
    }

    async fn post_allow_codes(
        &self,
        path: &str,
        body: &Value,
        allowed_codes: &[i64],
    ) -> anyhow::Result<Value> {
        let timestamp = self.timestamp();
        let json_body = serde_json::to_string(body)?;
        let payload = format!("{timestamp}{}{RECV_WINDOW}{json_body}", self.api_key);
        let signature = sign_hex(&self.api_secret, payload.as_bytes())?;
        let response = self
            .http
            .post(format!("{DEMO_REST_BASE}{path}"))
            .headers(self.auth_headers(timestamp, &signature)?)
            .body(json_body)
            .send()
            .await?;
        let response = self.receive(response).await?;
        self.check_response(&response, allowed_codes)?;
        Ok(response)
    }

    /// Extrai os headers de rate limit e sinaliza bloqueio 403 por
    /// frequência de IP antes de consumir o corpo. Puramente observacional —
    /// nunca decide sucesso/falha da chamada; isso é `check_response`.
    async fn receive(&self, response: reqwest::Response) -> anyhow::Result<Value> {
        let is_forbidden = response.status() == reqwest::StatusCode::FORBIDDEN;
        let (should_persist, snapshot) = {
            let mut health = self
                .api_health
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let updated = health.record_headers(response.headers());
            if is_forbidden {
                health.record_ip_throttle();
            }
            let should_persist =
                health.worth_persisting(updated, LOW_REMAINING_WARN_DIVISOR) || is_forbidden;
            (
                should_persist,
                (updated || is_forbidden).then(|| health.clone()),
            )
        };
        if let Some(health) = snapshot {
            if health.near_limit(LOW_REMAINING_WARN_DIVISOR) {
                tracing::warn!(
                    remaining = health.last_remaining,
                    limit = health.last_limit,
                    "rate limit privado da Bybit Demo perto do teto"
                );
            }
            if is_forbidden {
                tracing::warn!(
                    "Bybit Demo HTTP 403 — possível bloqueio de acesso por frequência (nível IP)"
                );
            }
            if should_persist {
                persist_api_health(&health);
            }
        }

        let response = response.error_for_status()?;
        Ok(response.json::<Value>().await?)
    }

    /// Confere o `retCode` e registra rejeição privada (rate limit ou outra)
    /// antes de delegar o veredito real a `ensure_success_or`, que continua
    /// sendo a única fonte de verdade sobre sucesso/falha.
    fn check_response(&self, response: &Value, allowed_codes: &[i64]) -> anyhow::Result<()> {
        let code = response
            .get("retCode")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        if code != 0 && !allowed_codes.contains(&code) {
            let message = response
                .get("retMsg")
                .and_then(Value::as_str)
                .unwrap_or("erro sem mensagem")
                .to_string();
            let snapshot = {
                let mut health = self
                    .api_health
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                health.record_rejection(code, &message);
                health.clone()
            };
            if code == RATE_LIMIT_RETCODE {
                tracing::warn!(retcode = code, %message, "Bybit Demo recusou por rate limit");
            }
            persist_api_health(&snapshot);
        }
        ensure_success_or(response, allowed_codes)
    }

    fn timestamp(&self) -> i64 {
        local_now_ms().saturating_add(self.clock_offset_ms.load(Ordering::Relaxed))
    }

    fn auth_headers(&self, timestamp: i64, signature: &str) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("X-BAPI-API-KEY", HeaderValue::from_str(&self.api_key)?);
        headers.insert(
            "X-BAPI-TIMESTAMP",
            HeaderValue::from_str(&timestamp.to_string())?,
        );
        headers.insert("X-BAPI-RECV-WINDOW", HeaderValue::from_static(RECV_WINDOW));
        headers.insert("X-BAPI-SIGN", HeaderValue::from_str(signature)?);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }
}

async fn send_unfilled(
    report_tx: &Sender<ExecutionReport>,
    request: &ExecutionRequest,
    started: &Instant,
    note: String,
) {
    let _ = report_tx
        .send(ExecutionReport {
            signal_id: request.signal_id,
            status: ExecutionStatus::Unfilled,
            return_pct: 0.0,
            realized_pnl: None,
            max_executable_notional: 0.0,
            observed_latency_ms: started.elapsed().as_millis() as u64,
            note,
        })
        .await;
}

fn ensure_success(value: &Value) -> anyhow::Result<()> {
    ensure_success_or(value, &[])
}

fn ensure_success_or(value: &Value, allowed_codes: &[i64]) -> anyhow::Result<()> {
    let code = value.get("retCode").and_then(Value::as_i64).unwrap_or(-1);
    if code == 0 || allowed_codes.contains(&code) {
        return Ok(());
    }
    let message = value
        .get("retMsg")
        .and_then(Value::as_str)
        .unwrap_or("erro sem mensagem");
    anyhow::bail!("Bybit Demo retCode={code}: {message}")
}

fn list_at(value: &Value) -> &[Value] {
    value
        .pointer("/result/list")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn parse_fills(value: &Value) -> FillSummary {
    parse_fills_for_side(value, None)
}

fn parse_fills_for_side(value: &Value, expected_side: Option<&str>) -> FillSummary {
    let mut seen = HashSet::new();
    let mut summary = FillSummary::default();
    for item in list_at(value) {
        if expected_side
            .map(|side| item.get("side").and_then(Value::as_str) != Some(side))
            .unwrap_or(false)
        {
            continue;
        }
        let Some(exec_id) = item.get("execId").and_then(Value::as_str) else {
            continue;
        };
        if !seen.insert(exec_id) {
            continue;
        }
        let qty = parse_number(item.get("execQty")).unwrap_or(0.0);
        let value = parse_number(item.get("execValue")).unwrap_or(0.0);
        let fee = parse_number(item.get("execFee")).unwrap_or(0.0).abs();
        if qty <= 0.0 || value <= 0.0 {
            continue;
        }
        summary.qty += qty;
        summary.value += value;
        summary.fee += fee;
        summary.executions += 1;
    }
    summary
}

fn parse_number(value: Option<&Value>) -> Option<f64> {
    value?
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn side_for(direction: Direction) -> &'static str {
    match direction {
        Direction::Long => "Buy",
        Direction::Short => "Sell",
    }
}

fn opposite_side(side: &str) -> &'static str {
    if side == "Buy" {
        "Sell"
    } else {
        "Buy"
    }
}

fn quantity_tolerance(qty: f64) -> f64 {
    (qty.abs() * 1e-8).max(1e-12)
}

fn format_quantity(qty: f64) -> String {
    let formatted = format!("{qty:.12}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn protective_stop_price(
    direction: Direction,
    entry_average: f64,
    stop_loss_pct: f64,
    price_tick: f64,
) -> anyhow::Result<String> {
    if !entry_average.is_finite()
        || entry_average <= 0.0
        || !stop_loss_pct.is_finite()
        || !(0.0..1.0).contains(&stop_loss_pct)
        || !price_tick.is_finite()
        || price_tick <= 0.0
    {
        anyhow::bail!("parâmetros inválidos para stop protetor Demo");
    }
    let raw = match direction {
        Direction::Long => entry_average * (1.0 - stop_loss_pct),
        Direction::Short => entry_average * (1.0 + stop_loss_pct),
    };
    let ticks = raw / price_tick;
    let rounded = match direction {
        Direction::Long => ticks.floor() * price_tick,
        Direction::Short => ticks.ceil() * price_tick,
    };
    if rounded <= 0.0 || !rounded.is_finite() {
        anyhow::bail!("preço inválido para stop protetor Demo");
    }
    Ok(format_quantity(rounded))
}

fn realized_pnl(direction: Direction, entry: &FillSummary, exit: &FillSummary) -> f64 {
    let gross = match direction {
        Direction::Long => exit.value - entry.value,
        Direction::Short => entry.value - exit.value,
    };
    gross - entry.fee - exit.fee
}

fn local_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn sign_hex(secret: &[u8], payload: &[u8]) -> anyhow::Result<String> {
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|_| anyhow::anyhow!("chave HMAC inválida"))?;
    mac.update(payload);
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_matches_known_vector() {
        let signature = sign_hex(b"key", b"The quick brown fox jumps over the lazy dog").unwrap();
        assert_eq!(
            signature,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn quantity_has_no_scientific_notation_or_trailing_zeroes() {
        assert_eq!(format_quantity(0.012_300_000_000_1), "0.0123");
        assert_eq!(format_quantity(25.0), "25");
    }

    #[test]
    fn parses_and_deduplicates_multiple_executions() {
        let value = serde_json::json!({"result":{"list":[
            {"execId":"a","execQty":"0.1","execValue":"10","execFee":"0.0055"},
            {"execId":"b","execQty":"0.2","execValue":"20","execFee":"0.011"},
            {"execId":"a","execQty":"0.1","execValue":"10","execFee":"0.0055"}
        ]}});
        let fills = parse_fills(&value);
        assert!((fills.qty - 0.3).abs() < 1e-12);
        assert!((fills.value - 30.0).abs() < 1e-12);
        assert_eq!(fills.executions, 2);
    }

    #[test]
    fn filters_exit_fills_by_side() {
        let value = serde_json::json!({"result":{"list":[
            {"execId":"entry","side":"Buy","execQty":"1","execValue":"100","execFee":"0.055"},
            {"execId":"exit","side":"Sell","execQty":"1","execValue":"101","execFee":"0.055"}
        ]}});
        let fills = parse_fills_for_side(&value, Some("Sell"));
        assert_eq!(fills.qty, 1.0);
        assert_eq!(fills.value, 101.0);
        assert_eq!(fills.executions, 1);
    }

    #[test]
    fn protective_stop_uses_tick_and_stays_beyond_entry() {
        assert_eq!(
            protective_stop_price(Direction::Long, 100.03, 0.005, 0.1).unwrap(),
            "99.5"
        );
        assert_eq!(
            protective_stop_price(Direction::Short, 100.03, 0.005, 0.1).unwrap(),
            "100.6"
        );
    }

    #[test]
    fn accepted_no_change_codes_are_explicit() {
        let already_one_way = serde_json::json!({"retCode": 110025, "retMsg": "not modified"});
        let already_one_x = serde_json::json!({"retCode": 110043, "retMsg": "not modified"});
        assert!(ensure_success_or(&already_one_way, &[110025]).is_ok());
        assert!(ensure_success_or(&already_one_x, &[110043]).is_ok());
        assert!(ensure_success(&already_one_way).is_err());
    }

    #[test]
    fn realized_pnl_uses_venue_values_and_both_fees() {
        let entry = FillSummary {
            qty: 1.0,
            value: 100.0,
            fee: 0.055,
            executions: 1,
        };
        let exit = FillSummary {
            qty: 1.0,
            value: 100.3,
            fee: 0.055,
            executions: 1,
        };
        assert!((realized_pnl(Direction::Long, &entry, &exit) - 0.19).abs() < 1e-12);
        assert!((realized_pnl(Direction::Short, &entry, &exit) + 0.41).abs() < 1e-12);
    }

    #[test]
    fn records_rate_limit_headers_and_tracks_minimum_remaining() {
        let mut counters = ApiHealthCounters::default();
        let mut headers = HeaderMap::new();
        headers.insert("x-bapi-limit", HeaderValue::from_static("120"));
        headers.insert("x-bapi-limit-status", HeaderValue::from_static("90"));
        headers.insert(
            "x-bapi-limit-reset-timestamp",
            HeaderValue::from_static("1700000000000"),
        );
        assert!(counters.record_headers(&headers));
        assert_eq!(counters.last_limit, Some(120));
        assert_eq!(counters.last_remaining, Some(90));
        assert_eq!(counters.last_reset_ms, Some(1_700_000_000_000));
        assert_eq!(counters.min_remaining_seen, Some(90));

        let mut lower = HeaderMap::new();
        lower.insert("x-bapi-limit", HeaderValue::from_static("120"));
        lower.insert("x-bapi-limit-status", HeaderValue::from_static("40"));
        counters.record_headers(&lower);
        assert_eq!(counters.min_remaining_seen, Some(40));

        let mut higher = HeaderMap::new();
        higher.insert("x-bapi-limit", HeaderValue::from_static("120"));
        higher.insert("x-bapi-limit-status", HeaderValue::from_static("100"));
        counters.record_headers(&higher);
        assert_eq!(
            counters.min_remaining_seen,
            Some(40),
            "mínimo observado não deve regredir pra cima"
        );
    }

    #[test]
    fn response_without_rate_limit_headers_is_a_noop() {
        let mut counters = ApiHealthCounters::default();
        assert!(!counters.record_headers(&HeaderMap::new()));
        assert_eq!(counters.calls_with_rate_limit_headers, 0);
        assert_eq!(counters.last_limit, None);
    }

    #[test]
    fn only_retcode_10006_counts_as_rate_limit_rejection() {
        let mut counters = ApiHealthCounters::default();
        counters.record_rejection(10_006, "Too many visits!");
        assert_eq!(counters.rate_limit_rejections_total, 1);
        assert_eq!(counters.other_private_rejections_total, 0);

        counters.record_rejection(10_001, "params error");
        assert_eq!(counters.rate_limit_rejections_total, 1);
        assert_eq!(counters.other_private_rejections_total, 1);
        assert_eq!(counters.last_rejection_retcode, Some(10_001));
    }

    #[test]
    fn near_limit_triggers_at_twenty_percent_remaining() {
        let mut counters = ApiHealthCounters::default();
        let mut still_ok = HeaderMap::new();
        still_ok.insert("x-bapi-limit", HeaderValue::from_static("100"));
        still_ok.insert("x-bapi-limit-status", HeaderValue::from_static("25"));
        counters.record_headers(&still_ok);
        assert!(!counters.near_limit(LOW_REMAINING_WARN_DIVISOR));

        let mut at_threshold = HeaderMap::new();
        at_threshold.insert("x-bapi-limit", HeaderValue::from_static("100"));
        at_threshold.insert("x-bapi-limit-status", HeaderValue::from_static("20"));
        counters.record_headers(&at_threshold);
        assert!(counters.near_limit(LOW_REMAINING_WARN_DIVISOR));
    }

    #[test]
    fn only_persists_on_first_observation_or_near_limit_not_every_call() {
        let mut counters = ApiHealthCounters::default();
        let mut headers = HeaderMap::new();
        headers.insert("x-bapi-limit", HeaderValue::from_static("100"));
        headers.insert("x-bapi-limit-status", HeaderValue::from_static("80"));
        let updated = counters.record_headers(&headers);
        assert!(counters.worth_persisting(updated, LOW_REMAINING_WARN_DIVISOR));

        // Segunda chamada de rotina, ainda longe do teto: não vale a pena
        // gravar em disco de novo (isto é o que evita 20+ writes por trade).
        let mut routine = HeaderMap::new();
        routine.insert("x-bapi-limit", HeaderValue::from_static("100"));
        routine.insert("x-bapi-limit-status", HeaderValue::from_static("79"));
        let updated = counters.record_headers(&routine);
        assert!(!counters.worth_persisting(updated, LOW_REMAINING_WARN_DIVISOR));

        // Perto do teto: volta a valer a pena, mesmo não sendo a primeira.
        let mut low = HeaderMap::new();
        low.insert("x-bapi-limit", HeaderValue::from_static("100"));
        low.insert("x-bapi-limit-status", HeaderValue::from_static("15"));
        let updated = counters.record_headers(&low);
        assert!(counters.worth_persisting(updated, LOW_REMAINING_WARN_DIVISOR));
    }

    #[test]
    fn ip_throttle_is_tracked_independently_of_retcode_rejections() {
        let mut counters = ApiHealthCounters::default();
        counters.record_ip_throttle();
        counters.record_ip_throttle();
        assert_eq!(counters.ip_throttle_rejections_total, 2);
        assert_eq!(counters.rate_limit_rejections_total, 0);
    }
}
