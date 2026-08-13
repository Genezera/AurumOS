use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::risk::{PortfolioState, RiskConfig, ScaleDirection, ScaleEvent};
use crate::types::{Opportunity, Strategy};

/// Tudo que o orquestrador transmite para o dashboard. Serializado como JSON
/// com um campo `type` que identifica a variante — o frontend só precisa
/// entender esse contrato, nunca o estado interno do Rust diretamente.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DashboardEvent {
    #[serde(rename = "opportunity_received")]
    OpportunityReceived {
        strategy: String,
        asset: String,
        net_edge: f64,
        confidence: f64,
        capital_needed: f64,
    },
    #[serde(rename = "decision")]
    Decision {
        strategy: String,
        asset: String,
        approved: bool,
        reason: Option<String>,
        order_size: Option<f64>,
    },
    #[serde(rename = "trade_result")]
    TradeResult {
        strategy: String,
        asset: String,
        outcome: String,
        pnl: f64,
        equity_after: f64,
    },
    #[serde(rename = "portfolio_snapshot")]
    PortfolioSnapshot {
        equity: f64,
        protected_reserve: f64,
        infra_reserve: f64,
        total_equity: f64,
        /// Perna por estratégia (task #97 — não é mais um único valor
        /// global). `(strategy_key, leg_size)`.
        leg_sizes: Vec<(String, f64)>,
        // Drawdown desde o pico histórico (all-time) — mesma métrica que o
        // kill-switch compara contra total_drawdown_halt_pct. Nome mantido
        // por compatibilidade; diario/semanal são campos separados abaixo.
        drawdown_pct: f64,
        daily_drawdown_pct: f64,
        weekly_drawdown_pct: f64,
        total_cycles: u64,
        wins: u64,
        losses: u64,
        rejections: u64,
        win_rate_pct: f64,
        exposure_by_strategy: Vec<(String, f64)>,
    },
    /// Emitido uma vez, no boot, com os limites de risco configurados —
    /// permite o painel mostrar "usado vs. limite" por estratégia sem
    /// duplicar os números do config/risk.toml no frontend.
    #[serde(rename = "risk_config")]
    RiskConfig {
        total_equity_start: f64,
        total_risk_pct: f64,
        strategy_risk_pct: Vec<(String, f64)>,
        leverage_launch_max: f64,
        leverage_liquid_max: f64,
        min_cycles_between_scale: u32,
        drawdown_halve_threshold_pct: f64,
        // Kill-switch em 4 camadas (revisão 13/08/2026) — expostos aqui pra
        // o painel poder desenhar o cockpit de risco com os limites reais
        // configurados, em vez de hardcodar/chutar no JS.
        daily_warn_pct: f64,
        daily_halt_pct: f64,
        weekly_halt_pct: f64,
        total_drawdown_halt_pct: f64,
    },
    /// O motor de risco mudou o tamanho da perna operacional (dobrou em
    /// nova máxima estável, ou reduziu pela metade em drawdown). Vira um
    /// marcador anotado direto no gráfico de equity.
    #[serde(rename = "leg_resized")]
    LegResized {
        /// `None` = ajuste portfolio-wide (redução de segurança). `Some` =
        /// perna de uma estratégia específica (task #97).
        strategy: Option<String>,
        old_size: f64,
        new_size: f64,
        direction: String,
        equity_at_event: f64,
    },
    /// Duas ou mais estratégias diferentes sinalizaram o mesmo ativo numa
    /// janela curta — o núcleo do que o roadmap pede ("orquestrador...
    /// vários métodos... se comunicando"). `price_confirmed` indica se pelo
    /// menos duas das estratégias envolvidas vêm de preço/book real
    /// (Arbitragem, Order Flow, Pump Exhaustion) — nesse caso o score da
    /// oportunidade recebe um bônus ao decidir; caso contrário é só
    /// contexto (ex.: Whale Watch coincidindo com algo), sem afetar a
    /// decisão de risco.
    #[serde(rename = "confluence")]
    Confluence {
        symbol: String,
        strategies: Vec<String>,
        price_confirmed: bool,
    },
    /// "Prova de vida" do escaneamento real: Arbitragem, Order Flow e Pump
    /// Exhaustion só emitem `opportunity_received` quando cruzam o limiar
    /// de edge — o que pode ficar em silêncio por muito tempo em mercado
    /// eficiente, dando a impressão de sistema parado quando na verdade
    /// está escaneando dezenas de pares a cada poucos milissegundos. Isso
    /// mostra o que está sendo encontrado agora, positivo ou negativo, sem
    /// filtrar nada — transparência real em vez de silêncio.
    #[serde(rename = "scan_heartbeat")]
    ScanHeartbeat {
        strategy: String,
        symbols_watched: u32,
        best_symbol: String,
        best_edge_pct: f64,
    },
    /// Kill-switch (Seção 12/Fase 12 do roadmap): novas execuções pausadas —
    /// manualmente (arquivo `data/KILL`) ou automaticamente (drawdown diário
    /// acima do limite configurado). Oportunidades continuam sendo
    /// observadas e visíveis, só a execução para.
    #[serde(rename = "system_halt")]
    SystemHalt { halted: bool, reason: Option<String> },
    /// Aviso preventivo do kill-switch em camadas (drawdown diário acima do
    /// nível de atenção mas ainda abaixo do bloqueio rígido) — não-bloqueante,
    /// só visibilidade. Ver risk.rs::kill_switch_reason.
    #[serde(rename = "risk_warning")]
    RiskWarning { warning: bool, message: Option<String> },
    /// Calendário de eventos macro (Fase 7) — emitido uma vez no boot.
    /// Diferente dos outros módulos, o Macro Engine passa a maior parte do
    /// tempo sem emitir `opportunity_received` nenhum (só dispara perto do
    /// horário oficial de cada evento) — sem isto, a aba do dashboard fica
    /// vazia o tempo todo, mesmo o módulo estando com o calendário certo
    /// carregado e funcionando. `event_ts_ms` é o instante exato (UTC) —
    /// o frontend calcula a contagem regressiva ao vivo a partir daí.
    #[serde(rename = "macro_calendar")]
    MacroCalendar { events: Vec<MacroCalendarEntry> },
    /// Universo de símbolos dinâmico (13/08/2026) — emitido a cada rotação
    /// (padrão: a cada 2h) pra o painel mostrar o que está sendo observado
    /// agora e por quê, em vez de uma lista fixa invisível.
    #[serde(rename = "symbol_universe")]
    SymbolUniverse {
        /// "linear" (perpétuos — Order Flow/Pump Exhaustion/Liquidation
        /// Hunter) ou "spot" (Bybit spot x Bitget spot — Arbitragem). Cada
        /// mercado tem seu próprio universo dinâmico independente (12/08/2026)
        /// porque o edge medido e os pares disponíveis são diferentes entre eles.
        kind: String,
        total: usize,
        top: Vec<SymbolRanking>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRanking {
    pub symbol: String,
    pub mean_edge_pct: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroCalendarEntry {
    pub kind: String,
    pub label: String,
    pub date: String,
    pub event_ts_ms: i64,
}

impl DashboardEvent {
    pub fn opportunity_received(opp: &Opportunity) -> Self {
        DashboardEvent::OpportunityReceived {
            strategy: opp.strategy.key().to_string(),
            asset: opp.asset.clone(),
            net_edge: opp.net_edge,
            confidence: opp.confidence,
            capital_needed: opp.capital_needed,
        }
    }

    pub fn decision_approved(strategy: Strategy, asset: &str, order_size: f64) -> Self {
        DashboardEvent::Decision {
            strategy: strategy.key().to_string(),
            asset: asset.to_string(),
            approved: true,
            reason: None,
            order_size: Some(order_size),
        }
    }

    pub fn decision_rejected(strategy: Strategy, asset: &str, reason: String) -> Self {
        DashboardEvent::Decision {
            strategy: strategy.key().to_string(),
            asset: asset.to_string(),
            approved: false,
            reason: Some(reason),
            order_size: None,
        }
    }

    pub fn trade_result(strategy: Strategy, asset: &str, won: bool, pnl: f64, equity_after: f64) -> Self {
        DashboardEvent::TradeResult {
            strategy: strategy.key().to_string(),
            asset: asset.to_string(),
            outcome: if won { "win" } else { "loss" }.to_string(),
            pnl,
            equity_after,
        }
    }

    pub fn portfolio_snapshot(p: &PortfolioState) -> Self {
        let win_rate_pct = if p.wins + p.losses > 0 {
            100.0 * p.wins as f64 / (p.wins + p.losses) as f64
        } else {
            0.0
        };
        DashboardEvent::PortfolioSnapshot {
            equity: p.equity,
            protected_reserve: p.protected_reserve,
            infra_reserve: p.infra_reserve,
            total_equity: p.equity + p.protected_reserve + p.infra_reserve,
            leg_sizes: p
                .strategy_scaling
                .iter()
                .map(|(s, sc)| (s.key().to_string(), sc.leg_size))
                .collect(),
            drawdown_pct: p.drawdown_pct() * 100.0,
            daily_drawdown_pct: p.daily_drawdown_pct() * 100.0,
            weekly_drawdown_pct: p.weekly_drawdown_pct() * 100.0,
            total_cycles: p.total_cycles,
            wins: p.wins,
            losses: p.losses,
            rejections: p.rejections,
            win_rate_pct,
            exposure_by_strategy: p
                .exposure_by_strategy
                .iter()
                .map(|(s, v)| (s.key().to_string(), *v))
                .collect(),
        }
    }

    pub fn leg_resized(scale: ScaleEvent, equity_at_event: f64) -> Self {
        DashboardEvent::LegResized {
            strategy: scale.strategy.map(|s| s.key().to_string()),
            old_size: scale.old_size,
            new_size: scale.new_size,
            direction: match scale.direction {
                ScaleDirection::Increase => "increase",
                ScaleDirection::Decrease => "decrease",
            }
            .to_string(),
            equity_at_event,
        }
    }

    pub fn scan_heartbeat(strategy: Strategy, symbols_watched: u32, best_symbol: &str, best_edge_pct: f64) -> Self {
        DashboardEvent::ScanHeartbeat {
            strategy: strategy.key().to_string(),
            symbols_watched,
            best_symbol: best_symbol.to_string(),
            best_edge_pct,
        }
    }

    pub fn confluence(symbol: &str, strategies: Vec<&'static str>, price_confirmed: bool) -> Self {
        DashboardEvent::Confluence {
            symbol: symbol.to_string(),
            strategies: strategies.into_iter().map(String::from).collect(),
            price_confirmed,
        }
    }

    pub fn system_halt(reason: Option<String>) -> Self {
        DashboardEvent::SystemHalt { halted: reason.is_some(), reason }
    }

    pub fn risk_warning(message: Option<String>) -> Self {
        DashboardEvent::RiskWarning { warning: message.is_some(), message }
    }

    pub fn macro_calendar(events: Vec<MacroCalendarEntry>) -> Self {
        DashboardEvent::MacroCalendar { events }
    }

    pub fn symbol_universe(kind: &str, total: usize, top: Vec<SymbolRanking>) -> Self {
        DashboardEvent::SymbolUniverse { kind: kind.to_string(), total, top }
    }

    pub fn risk_config(cfg: &RiskConfig) -> Self {
        DashboardEvent::RiskConfig {
            total_equity_start: cfg.total_equity_start,
            total_risk_pct: cfg.total_risk_pct * 100.0,
            strategy_risk_pct: cfg
                .strategy_risk_pct
                .iter()
                .map(|(k, v)| (k.clone(), v * 100.0))
                .collect(),
            leverage_launch_max: cfg.leverage.launch_max,
            leverage_liquid_max: cfg.leverage.liquid_max,
            min_cycles_between_scale: cfg.scaling.min_cycles_between_scale,
            drawdown_halve_threshold_pct: cfg.scaling.drawdown_halve_threshold_pct * 100.0,
            daily_warn_pct: cfg.daily_warn_pct * 100.0,
            daily_halt_pct: cfg.daily_halt_pct * 100.0,
            weekly_halt_pct: cfg.weekly_halt_pct * 100.0,
            total_drawdown_halt_pct: cfg.total_drawdown_halt_pct * 100.0,
        }
    }
}

/// Um evento com número de sequência monotônico (para deduplicar entre o
/// backlog e o fluxo ao vivo) e o instante real em que aconteceu — usado
/// pelo frontend para timestamps corretos mesmo quando o evento chega via
/// replay de histórico muito depois de ter ocorrido de verdade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: u64,
    pub ts_ms: u64,
    #[serde(flatten)]
    pub event: DashboardEvent,
}

/// Barramento de eventos do orquestrador: além de transmitir ao vivo via
/// broadcast, retém um histórico em memória E em disco (se criado via
/// `new_persistent`) desde a primeira vez que o processo rodou. É isso que
/// garante que fechar/reabrir o dashboard no navegador — E reiniciar o
/// próprio processo do orquestrador — não perdem nada: equity, feed e
/// gráfico continuam de onde pararam.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Envelope>,
    history: Arc<Mutex<VecDeque<Envelope>>>,
    seq: Arc<AtomicU64>,
    cap: usize,
    started_at_ms: u64,
    // Handle unico, aberto uma vez, protegido por mutex — substitui o
    // padrao anterior de reabrir o arquivo a cada emit() e confiar so na
    // atomicidade de append() do SO (revisao tecnica externa, 13/08/2026:
    // "usar um unico writer, fila ou mutex" em vez de depender de garantia
    // de baixo nivel dificil de auditar). Agora e impossivel duas emissoes
    // concorrentes intercalarem, ponto — nao depende de nenhuma semantica
    // de sistema de arquivos.
    persist_file: Option<Arc<Mutex<std::fs::File>>>,
}

impl EventBus {
    pub fn new(cap: usize) -> Self {
        let (tx, _rx) = broadcast::channel(4096);
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            tx,
            history: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(4096)))),
            seq: Arc::new(AtomicU64::new(0)),
            cap,
            started_at_ms,
            persist_file: None,
        }
    }

    /// Como `new`, mas lendo (se existir) e depois anexando cada evento novo
    /// a um arquivo JSONL em `path` — uma linha por evento. Linhas
    /// corrompidas são puladas silenciosamente em vez de derrubar o boot; o
    /// pior caso é perder aquele evento específico, não o histórico inteiro.
    pub fn new_persistent(cap: usize, path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        let mut history: VecDeque<Envelope> = VecDeque::with_capacity(cap.min(4096));
        let mut max_seq: Option<u64> = None;
        let mut first_ts_ms: Option<u64> = None;

        if let Ok(raw) = std::fs::read_to_string(&path) {
            for line in raw.lines() {
                let line = line.strip_prefix('\u{feff}').unwrap_or(line);
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(env) = serde_json::from_str::<Envelope>(line) else {
                    continue;
                };
                max_seq = Some(env.seq.max(max_seq.unwrap_or(0)));
                first_ts_ms.get_or_insert(env.ts_ms);
                history.push_back(env);
                while history.len() > cap {
                    history.pop_front();
                }
            }
            if !history.is_empty() {
                tracing::info!(eventos_recuperados = history.len(), path = %path.display(), "histórico de eventos recuperado do disco");
            }
        }

        let started_at_ms = first_ts_ms.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
        let next_seq = max_seq.map(|s| s + 1).unwrap_or(0);

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Abre UMA vez aqui — todo emit() futuro reusa o mesmo handle via
        // mutex, em vez de abrir um novo a cada chamada.
        let persist_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map(|f| Arc::new(Mutex::new(f)))
            .ok();
        if persist_file.is_none() {
            tracing::warn!(path = %path.display(), "não foi possível abrir o arquivo de eventos para escrita — histórico não será persistido nesta sessão");
        }

        let (tx, _rx) = broadcast::channel(4096);
        Self {
            tx,
            history: Arc::new(Mutex::new(history)),
            seq: Arc::new(AtomicU64::new(next_seq)),
            cap,
            started_at_ms,
            persist_file,
        }
    }

    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    pub fn emit(&self, event: DashboardEvent) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(self.started_at_ms);
        let env = Envelope { seq, ts_ms, event };

        if let Some(file) = &self.persist_file {
            if let Ok(mut line) = serde_json::to_string(&env) {
                line.push('\n');
                // Handle único + mutex: nenhuma emissão concorrente pode
                // intercalar com outra, ponto — não depende de nenhuma
                // garantia de baixo nível do sistema de arquivos.
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(line.as_bytes());
                }
            }
        }

        {
            let mut h = self.history.lock().unwrap();
            h.push_back(env.clone());
            while h.len() > self.cap {
                h.pop_front();
            }
        }
        let _ = self.tx.send(env);
    }

    /// Assina o barramento (para eventos futuros) e retorna também tudo que
    /// já aconteceu desde o início do processo, até o limite de retenção.
    /// Subscrever antes de ler o histórico garante que nenhum evento emitido
    /// bem no meio da conexão seja perdido; o `seq` de cada envelope permite
    /// ao chamador descartar duplicatas entre backlog e fluxo ao vivo.
    pub fn subscribe_with_backlog(&self) -> (Vec<Envelope>, broadcast::Receiver<Envelope>) {
        let rx = self.tx.subscribe();
        let backlog: Vec<Envelope> = self.history.lock().unwrap().iter().cloned().collect();
        (backlog, rx)
    }
}
