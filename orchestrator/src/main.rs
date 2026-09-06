mod bybit_demo;
mod dashboard;
mod events;
mod exchange_filters;
mod fusion;
mod orchestrator;
mod raw_log;
mod risk;
mod sources;
mod symbol_universe;
mod types;

use std::net::SocketAddr;

use tokio::sync::mpsc;
use tokio::time::Duration;

use events::{DashboardEvent, EventBus};
use risk::RiskConfig;
use sources::dex_launch_radar::DexLaunchRadarSource;
use sources::funding_carry::FundingCarrySource;
use sources::launch_radar::LaunchRadarSource;
use sources::liquidation_hunter::LiquidationHunterSource;
use sources::macro_engine::MacroEngineSource;
use sources::news_reactor::NewsReactorSource;
use sources::order_flow::OrderFlowSource;
use sources::pump_exhaustion::PumpExhaustionSource;
use sources::whale_watch::WhaleWatchSource;
use sources::SignalSource;
use types::ExecutionBackend;

// Semente ampla de perpétuos lineares USDT. O universo dinâmico confirma os
// instrumentos atualmente negociáveis na Bybit e preserva vagas de
// exploração para não ficar preso aos símbolos que já têm histórico.
const WIDE_SYMBOLS: &[&str] = &[
    "BTCUSDT",
    "ETHUSDT",
    "SOLUSDT",
    "XRPUSDT",
    "DOGEUSDT",
    "ADAUSDT",
    "TRXUSDT",
    "LTCUSDT",
    "AVAXUSDT",
    "DOTUSDT",
    "LINKUSDT",
    "INJUSDT",
    "ATOMUSDT",
    "NEARUSDT",
    "UNIUSDT",
    "APTUSDT",
    "ARBUSDT",
    "OPUSDT",
    "SUIUSDT",
    "FILUSDT",
    "BCHUSDT",
    "ETCUSDT",
    "XLMUSDT",
    "ALGOUSDT",
    "AAVEUSDT",
    "MKRUSDT",
    "LDOUSDT",
    "GRTUSDT",
    "SANDUSDT",
    "IMXUSDT",
    "FTMUSDT",
    "RUNEUSDT",
    "STXUSDT",
    "DYDXUSDT",
    "GALAUSDT",
    "CHZUSDT",
    "ENJUSDT",
    "MANAUSDT",
    "AXSUSDT",
    "EOSUSDT",
    "XTZUSDT",
    "ONEUSDT",
    "KAVAUSDT",
    "ROSEUSDT",
    "FLOWUSDT",
    "COMPUSDT",
    "SNXUSDT",
    "CRVUSDT",
    "QTUMUSDT",
    "ICXUSDT",
    "WAVESUSDT",
    "KSMUSDT",
    "ZECUSDT",
    "DASHUSDT",
    "YFIUSDT",
    "STORJUSDT",
    "HBARUSDT",
    "VETUSDT",
    "THETAUSDT",
    "EGLDUSDT",
    "JASMYUSDT",
    "GMTUSDT",
    "APEUSDT",
    "WOOUSDT",
    "SUSHIUSDT",
    "GMXUSDT",
    "PENDLEUSDT",
    "ORDIUSDT",
    "1000BONKUSDT",
    "WIFUSDT",
    "1000FLOKIUSDT",
    "ONDOUSDT",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Carrega orchestrator/.env se existir (RPC da Alchemy, etc.) — assim o usuário só edita um arquivo de texto em vez
    // de configurar variável de ambiente toda vez que reinicia. Caminho
    // absoluto via CARGO_MANIFEST_DIR (mesmo padrão de events_path/
    // state_path abaixo) porque dotenvy::dotenv() sozinho busca a partir do
    // diretório de trabalho do processo — se o binário for iniciado a
    // partir da raiz do workspace (onde os caminhos de dado relativos
    // acima assumem que ele roda), nunca acharia orchestrator/.env, que
    // fica um nível abaixo, não acima. Silencioso se o arquivo não existir.
    dotenvy::from_path(format!("{}/.env", env!("CARGO_MANIFEST_DIR"))).ok();

    // tokio-tungstenite usa rustls para as conexões wss:// com a Bybit;
    // com múltiplos backends de criptografia disponíveis no workspace, o
    // rustls exige que um seja instalado explicitamente como padrão do
    // processo antes da primeira conexão TLS.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("falha ao instalar o provedor de criptografia do rustls");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "orchestrator=info".into()),
        )
        .init();

    let config_path = format!("{}/config/risk.toml", env!("CARGO_MANIFEST_DIR"));
    let cfg = RiskConfig::load(&config_path)?;
    let execution_backend = ExecutionBackend::parse(
        &std::env::var("AURUMOS_EXECUTION_MODE").unwrap_or_else(|_| "shadow".to_string()),
    )?;
    let linear_taker_fee_rates = exchange_filters::fetch_bybit_crypto_taker_fees().await;
    let demo_client = if execution_backend == ExecutionBackend::BybitDemo {
        let client = bybit_demo::BybitDemoClient::from_env()?;
        client.preflight(cfg.total_equity_start).await?;
        Some(client)
    } else {
        None
    };

    tracing::info!(
        equity_start = cfg.total_equity_start,
        leg_size = cfg.initial_leg_size,
        execution_backend = execution_backend.key(),
        "AurumOS — uma corretora (Bybit); demais módulos somente observacionais"
    );

    // Retém até 50.000 eventos, em memória E em disco (data/events.jsonl) —
    // sobrevive tanto a fechar/reabrir a aba do navegador quanto a
    // reiniciar o próprio processo (pra aplicar código novo, por exemplo).
    // Arquivos versionados: o estado antigo continha PnL produzido pelo
    // simulador de bootstrap e não pode contaminar a contabilidade nova.
    let persistence_version = match execution_backend {
        ExecutionBackend::Shadow => "v2",
        ExecutionBackend::BybitDemo => "v1",
    };
    let events_path = format!(
        "{}/data/events_{}_{}.jsonl",
        env!("CARGO_MANIFEST_DIR"),
        execution_backend.key(),
        persistence_version
    );
    let state_path = format!(
        "{}/data/portfolio_state_{}_{}.json",
        env!("CARGO_MANIFEST_DIR"),
        execution_backend.key(),
        persistence_version
    );
    // Kill-switch manual (Fase 12): crie esse arquivo (vazio, qualquer
    // conteúdo) pra pausar novas execuções sem matar o processo; apague pra
    // retomar. Checado a cada tick em orchestrator::run.
    let kill_file_path = format!("{}/data/KILL", env!("CARGO_MANIFEST_DIR"));
    // Reset manual do nível mais grave do kill-switch (drawdown total desde
    // o pico) — diferente de KILL, este é consumido (apagado sozinho) na
    // hora em que é lido, então recriar o arquivo é o gesto explícito de
    // "confirmo, destrave" a cada vez, não um interruptor permanente.
    let reset_drawdown_file_path =
        format!("{}/data/RESET_DRAWDOWN_HALT", env!("CARGO_MANIFEST_DIR"));

    // Equity e histórico de eventos (gráfico/feed) têm que resetar SEMPRE
    // juntos, nunca um sem o outro — senão o dashboard mostraria um
    // gráfico com equity antigo ao lado de um tile de equity zerado, o que
    // é mais confuso que simplesmente não ter persistência nenhuma. O
    // arquivo de estado do portfólio é a fonte da verdade. A persistência
    // mantém também `<estado>.prev`; durante a troca atômica ou depois de
    // uma escrita corrompida, esse backup ainda representa uma sessão
    // recuperável e o histórico não pode ser descartado.
    if !risk::has_recoverable_portfolio_state(&state_path) {
        if std::path::Path::new(&events_path).exists() {
            tracing::info!(events_path, "sem estado de portfólio salvo — descartando histórico de eventos antigo também, pra não misturar equity zerado com gráfico de uma sessão anterior");
        }
        let _ = std::fs::remove_file(&events_path);
    }

    let bus = EventBus::new_persistent(50_000, events_path);
    bus.emit(DashboardEvent::execution_backend(execution_backend));
    bus.emit(DashboardEvent::risk_config(&cfg));

    let dashboard_addr: SocketAddr = "127.0.0.1:7878".parse()?;
    let dashboard_bus = bus.clone();
    tokio::spawn(async move {
        if let Err(e) = dashboard::serve(dashboard_addr, dashboard_bus).await {
            tracing::error!(error = %e, "servidor do dashboard encerrou com erro");
        }
    });
    tracing::info!(url = %format!("http://{dashboard_addr}"), "abra o painel em tempo real neste endereço");

    let (tx, rx) = mpsc::channel(256);
    let (execution_tx, execution_rx) = mpsc::channel(256);
    let (execution_request_tx, execution_request_rx) = mpsc::channel(64);
    let demo_executor_handle = demo_client.map(|client| {
        let reports = execution_tx.clone();
        tokio::spawn(async move { client.run(execution_request_rx, reports).await })
    });

    // Universo de símbolos dinâmico (13/08/2026, pedido do usuário: "não
    // quero que fique travado no mesmo, quero uma análise inteira do
    // mercado inteiro"). Nasce com WIDE_SYMBOLS como semente (evita perder
    // cobertura no boot) e se recalcula sozinho — ver symbol_universe.rs.
    // `watch::channel` porque cada fonte só precisa da lista MAIS RECENTE,
    // nunca do histórico de mudanças.
    let seed_symbols: Vec<String> = WIDE_SYMBOLS.iter().map(|s| s.to_string()).collect();
    let (symbols_tx, symbols_rx) = tokio::sync::watch::channel(seed_symbols.clone());
    let edge_scores = symbol_universe::new_edge_scores();
    let symbol_universe_handle = {
        let scores = edge_scores.clone();
        let su_bus = bus.clone();
        let seed = seed_symbols.clone();
        tokio::spawn(async move {
            symbol_universe::run(symbol_universe::LINEAR, scores, symbols_tx, seed, su_bus).await
        })
    };

    // Salva o edge medido a cada 30s, independente da rotação de 15min
    // (pedido do usuário, 12/08/2026: "eu não quero que perde nada quando
    // reinicia o sistema") — `run()` já salva a cada rotação, mas um
    // restart no meio de uma janela de 15min perderia tudo desde a última
    // sem isto. Tarefa leve: só serializa o que já está em memória, não
    // refaz nenhuma medição.
    let edge_scores_saver_handle = {
        let scores_linear = edge_scores.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                iv.tick().await;
                symbol_universe::save_edge_scores(
                    &scores_linear,
                    symbol_universe::LINEAR.edge_scores_filename,
                );
            }
        })
    };

    // Quadros de fusão (pedido do usuário: "faça a fusão", "faça
    // intercomunicação") — Whale Watch e Liquidation Hunter alimentam,
    // Pump Exhaustion lê como reforço de confiança. Ver fusion.rs.
    let liquidation_board = fusion::new_liquidation_board();
    let whale_board = fusion::new_whale_board();

    // Microestrutura de uma única corretora: book + negócios públicos dos
    // perpétuos USDT da Bybit. É o único módulo autorizado a alimentar o
    // PnL no backend selecionado; os demais continuam como inteligência até ganharem um
    // executor próprio ligado por signal_id.
    let of_tx = tx.clone();
    let mut order_flow = OrderFlowSource::new(
        symbols_rx.clone(),
        bus.clone(),
        edge_scores.clone(),
        linear_taker_fee_rates,
        execution_tx.clone(),
        execution_backend == ExecutionBackend::Shadow,
    );
    tracing::info!(source = order_flow.name(), "iniciando fonte executável");
    let order_flow_handle = tokio::spawn(async move {
        if let Err(e) = order_flow.run(of_tx).await {
            tracing::error!(error = %e, "fonte de order flow encerrou com erro");
        }
    });

    // Whale Watch (Fase 4): nó Ethereum público, sem chave de API. Só
    // visibilidade por enquanto (net_edge=0 sempre) — ver comentário em
    // whale_watch.rs sobre por que isso nunca dispara ordem sozinho.
    let ww_tx = tx.clone();
    let mut whale_watch = WhaleWatchSource::new(whale_board.clone());
    let whale_watch_handle = tokio::spawn(async move {
        if let Err(e) = whale_watch.run(ww_tx).await {
            tracing::error!(error = %e, "fonte de whale watch encerrou com erro");
        }
    });

    // News Reactor (Fase 3): filings 8-K novos direto da SEC EDGAR, sem
    // chave de API. Ver comentário em news_reactor.rs — sem classificação
    // por IA ainda (decisão pendente do roadmap), então também é só
    // visibilidade por enquanto (net_edge=0).
    let news_tx = tx.clone();
    let mut news_reactor = NewsReactorSource;
    let news_reactor_handle = tokio::spawn(async move {
        if let Err(e) = news_reactor.run(news_tx).await {
            tracing::error!(error = %e, "fonte de news reactor encerrou com erro");
        }
    });

    // Pump Exhaustion (Fase 6, parcial): funding rate + variação 24h reais
    // de perpétuos na Bybit. Ver comentário em pump_exhaustion.rs — é só
    // uma fatia do detector completo do roadmap, e por isso também sai só
    // como visibilidade (net_edge=0) até passar por backtest de verdade.
    let pe_tx = tx.clone();
    let mut pump_exhaustion = PumpExhaustionSource::new(
        symbols_rx.clone(),
        bus.clone(),
        liquidation_board.clone(),
        whale_board.clone(),
    );
    let pump_exhaustion_handle = tokio::spawn(async move {
        if let Err(e) = pump_exhaustion.run(pe_tx).await {
            tracing::error!(error = %e, "fonte de pump exhaustion encerrou com erro");
        }
    });

    // Macro Engine (Fase 7): calendário real de FOMC/CPI/NFP (2026),
    // sem API — datas públicas pré-anunciadas. Ver comentário em
    // macro_engine.rs: net_edge=0, é aviso de janela de risco, não ordem.
    let macro_tx = tx.clone();
    let mut macro_engine = MacroEngineSource::new(bus.clone());
    let macro_engine_handle = tokio::spawn(async move {
        if let Err(e) = macro_engine.run(macro_tx).await {
            tracing::error!(error = %e, "fonte de macro engine encerrou com erro");
        }
    });

    // Launch Radar (Fase 5, escopo reduzido): pares novos aparecendo no
    // spot da Bybit via polling do endpoint público instruments-info. Ver
    // comentário em launch_radar.rs sobre o que falta (análise de contrato
    // exige provedor de block explorer — decisão pendente do roadmap).
    let launch_tx = tx.clone();
    let mut launch_radar = LaunchRadarSource;
    let launch_radar_handle = tokio::spawn(async move {
        if let Err(e) = launch_radar.run(launch_tx).await {
            tracing::error!(error = %e, "fonte de launch radar encerrou com erro");
        }
    });

    // Launch Radar (parte 2): checklist de contrato real em lançamentos
    // on-chain (Uniswap V2), não só listagens em exchange centralizada. Ver
    // comentário em dex_launch_radar.rs sobre o que ainda falta do
    // checklist completo do PDF (holders, deployer — exige indexador pago).
    let dex_tx = tx.clone();
    let mut dex_launch_radar = DexLaunchRadarSource;
    let dex_launch_radar_handle = tokio::spawn(async move {
        if let Err(e) = dex_launch_radar.run(dex_tx).await {
            tracing::error!(error = %e, "fonte de dex launch radar encerrou com erro");
        }
    });

    // Liquidation Hunter: cascatas de liquidação reais via Bybit
    // allLiquidation. Módulo do roadmap original que ainda não tinha sido
    // construído — ver comentário em liquidation_hunter.rs.
    let lh_tx = tx.clone();
    let mut liquidation_hunter =
        LiquidationHunterSource::new(symbols_rx.clone(), bus.clone(), liquidation_board.clone());
    let liquidation_hunter_handle = tokio::spawn(async move {
        if let Err(e) = liquidation_hunter.run(lh_tx).await {
            tracing::error!(error = %e, "fonte de liquidation hunter encerrou com erro");
        }
    });

    // Junção segura com o Snowball: radar cash-and-carry spot+perp dentro
    // da própria Bybit. O Snowball fazia esta conta em paper com taxas e
    // horários aproximados; esta versão lê bid/ask, tamanho no L1,
    // turnover, fundingIntervalHour e nextFundingTime diretamente da V5.
    // Continua estritamente observacional: uma posição neutra exige um
    // executor de duas pernas e uma máquina de recuperação própria.
    let mut funding_carry = FundingCarrySource::new(bus.clone());
    let funding_carry_handle = tokio::spawn(async move {
        if let Err(e) = funding_carry.run().await {
            tracing::error!(error = %e, "radar funding carry encerrou com erro");
        }
    });

    // Sem limite de ciclos: o orquestrador roda continuamente para que o
    // dashboard tenha algo ao vivo para mostrar. Encerre com Ctrl+C.
    let max_cycles = u64::MAX;
    let final_state = orchestrator::run(
        rx,
        execution_rx,
        cfg,
        max_cycles,
        Duration::from_millis(150),
        bus,
        state_path,
        kill_file_path,
        reset_drawdown_file_path,
        execution_backend,
        if execution_backend == ExecutionBackend::BybitDemo {
            Some(execution_request_tx)
        } else {
            None
        },
    )
    .await;

    symbol_universe_handle.abort();
    edge_scores_saver_handle.abort();
    order_flow_handle.abort();
    whale_watch_handle.abort();
    news_reactor_handle.abort();
    pump_exhaustion_handle.abort();
    macro_engine_handle.abort();
    launch_radar_handle.abort();
    dex_launch_radar_handle.abort();
    liquidation_hunter_handle.abort();
    funding_carry_handle.abort();
    if let Some(handle) = demo_executor_handle {
        handle.abort();
    }

    let win_rate = if final_state.wins + final_state.losses > 0 {
        100.0 * final_state.wins as f64 / (final_state.wins + final_state.losses) as f64
    } else {
        0.0
    };

    tracing::info!(
        execution_backend = execution_backend.key(),
        "===== resumo da execução ====="
    );
    tracing::info!(
        equity_final = final_state.equity,
        reserva_protegida = final_state.protected_reserve,
        patrimonio_total = final_state.equity + final_state.protected_reserve,
        maior_leg_size_final = final_state
            .strategy_scaling
            .values()
            .map(|s| s.leg_size)
            .fold(0.0, f64::max),
        ciclos = final_state.total_cycles,
        wins = final_state.wins,
        losses = final_state.losses,
        win_rate_pct = win_rate,
        rejeicoes_pelo_risk_engine = final_state.rejections,
        "resultado final"
    );

    Ok(())
}
