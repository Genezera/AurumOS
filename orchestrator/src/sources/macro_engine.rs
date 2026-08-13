use std::collections::HashSet;
use std::time::Instant;

use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone, Weekday};
use chrono_tz::America::New_York;
use tokio::sync::mpsc::Sender;
use tokio::time::{sleep, Duration};

use crate::events::{DashboardEvent, EventBus, MacroCalendarEntry};
use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

const CHECK_INTERVAL: Duration = Duration::from_secs(30);
// Janela em torno do horário oficial em que consideramos o evento "ao vivo".
// Cedo o bastante pra avisar antes, tarde o bastante pra cobrir o release
// em si (números costumam sair no minuto exato, mas o feed pode atrasar).
const WINDOW_BEFORE_MIN: i64 = 5;
const WINDOW_AFTER_MIN: i64 = 15;

#[derive(Debug, Clone, Copy)]
enum EventKind {
    Fomc,
    Cpi,
    Nfp,
}

impl EventKind {
    fn label(&self) -> &'static str {
        match self {
            EventKind::Fomc => "Decisão do FOMC (Fed)",
            EventKind::Cpi => "CPI (inflação ao consumidor)",
            EventKind::Nfp => "Employment Situation (payrolls)",
        }
    }
}

struct MacroEvent {
    kind: EventKind,
    date: NaiveDate,
    time_et: NaiveTime,
}

/// Fase 7 do roadmap: calendário de eventos macro que realmente move dólar,
/// juros, índices e cripto ao mesmo tempo. Diferente dos outros módulos,
/// isso não é um feed ao vivo — é calendário público pré-anunciado
/// (FOMC/CPI vêm de datas divulgadas pelo Fed/BLS; NFP segue a regra
/// conhecida de "primeira sexta-feira do mês"). O preço de ser estático:
/// as datas de FOMC e CPI precisam ser atualizadas manualmente a cada ano
/// (ver `fomc_dates_2026` e `cpi_dates_2026` abaixo).
///
/// Assim como Whale Watch e News, isso não tem vantagem numérica de
/// verdade sozinho — saber que "o CPI sai em 3 minutos" não diz pra que
/// lado o preço vai. `net_edge = 0.0` de propósito: é aviso de janela de
/// risco elevado (correlação entre ativos sobe muito nesses momentos — ver
/// a trava de `grupo_correlacao` no motor de risco), não uma ordem.
pub struct MacroEngineSource {
    pub bus: EventBus,
}

impl MacroEngineSource {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }
}

#[async_trait::async_trait]
impl SignalSource for MacroEngineSource {
    fn name(&self) -> &'static str {
        "macro_engine_calendar"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        let events = build_calendar();
        tracing::info!(total_eventos = events.len(), "macro engine: calendário carregado (FOMC/CPI/NFP)");
        log_next_event_per_kind(&events);
        self.bus.emit(DashboardEvent::macro_calendar(next_event_per_kind_for_dashboard(&events)));

        let mut already_fired: HashSet<(usize, NaiveDate)> = HashSet::new();

        loop {
            let now_et = chrono::Utc::now().with_timezone(&New_York);

            for (idx, event) in events.iter().enumerate() {
                let Some(event_dt) = New_York.from_local_datetime(&event.date.and_time(event.time_et)).single() else {
                    continue;
                };
                // minutes_since_event < 0 significa que o evento ainda não
                // aconteceu (estamos ANTES dele); > 0 significa que já
                // passou. A janela cobre de WINDOW_BEFORE_MIN antes até
                // WINDOW_AFTER_MIN depois do horário oficial.
                let minutes_since_event = (now_et - event_dt).num_minutes();
                let in_window = (-WINDOW_BEFORE_MIN..=WINDOW_AFTER_MIN).contains(&minutes_since_event);

                if in_window && already_fired.insert((idx, event.date)) {
                    tracing::info!(
                        evento = event.kind.label(),
                        data = %event.date,
                        horario_et = %event.time_et,
                        "janela de evento macro ativa"
                    );

                    let opp = Opportunity {
                        market: Market::Index,
                        strategy: Strategy::Macro,
                        asset: format!("{} — {}", event.kind.label(), event.date),
                        direction: Direction::Long,
                        net_edge: 0.0,
                        confidence: 0.2,
                        valid_for_ms: 90_000,
                        // Informativo (net_edge=0) — placeholder consistente com valid_for_ms.
                        expected_holding_secs: 90.0,
                        capital_needed: 10.0,
                        max_loss_pct: 0.02,
                        leverage: 1.0,
                        correlation_group: "usd_macro".to_string(),
                        sampled_return: None,
                        emitted_at: Instant::now(),
                    };
                    let _ = tx.send(opp).await;
                }
            }

            sleep(CHECK_INTERVAL).await;
        }
    }
}

/// Log de diagnóstico no boot: mostra o próximo evento de cada tipo e
/// quanto falta, em minutos — forma rápida de confirmar visualmente que a
/// conversão de fuso (America/New_York) e as datas estão corretas, sem
/// precisar esperar um evento real acontecer.
fn log_next_event_per_kind(events: &[MacroEvent]) {
    let now_et = chrono::Utc::now().with_timezone(&New_York);
    for kind_label in ["FOMC", "CPI", "NFP"] {
        let next = events
            .iter()
            .filter(|e| {
                matches!(
                    (kind_label, e.kind),
                    ("FOMC", EventKind::Fomc) | ("CPI", EventKind::Cpi) | ("NFP", EventKind::Nfp)
                )
            })
            .filter_map(|e| {
                New_York
                    .from_local_datetime(&e.date.and_time(e.time_et))
                    .single()
                    .map(|dt| (e, dt))
            })
            .filter(|(_, dt)| *dt > now_et)
            .min_by_key(|(_, dt)| *dt);

        if let Some((event, dt)) = next {
            let minutes_until = (dt - now_et.clone()).num_minutes();
            tracing::info!(
                evento = event.kind.label(),
                data = %event.date,
                minutos_ate_o_evento = minutes_until,
                "macro engine: próximo evento deste tipo"
            );
        }
    }
}

/// Mesma busca de "próximo evento por tipo" que o log de diagnóstico faz,
/// mas devolvendo dado estruturado (timestamp UTC em ms) pro dashboard —
/// o frontend calcula a contagem regressiva ao vivo a partir daí, em vez
/// de depender de um "minutos_ate_o_evento" que ficaria desatualizado
/// assim que fosse emitido uma única vez no boot.
fn next_event_per_kind_for_dashboard(events: &[MacroEvent]) -> Vec<MacroCalendarEntry> {
    let now_et = chrono::Utc::now().with_timezone(&New_York);
    let mut out = Vec::new();
    for (kind_tag, kind_label) in [("fomc", "FOMC"), ("cpi", "CPI"), ("nfp", "NFP")] {
        let next = events
            .iter()
            .filter(|e| {
                matches!(
                    (kind_label, e.kind),
                    ("FOMC", EventKind::Fomc) | ("CPI", EventKind::Cpi) | ("NFP", EventKind::Nfp)
                )
            })
            .filter_map(|e| {
                New_York
                    .from_local_datetime(&e.date.and_time(e.time_et))
                    .single()
                    .map(|dt| (e, dt))
            })
            .filter(|(_, dt)| *dt > now_et)
            .min_by_key(|(_, dt)| *dt);

        if let Some((event, dt)) = next {
            out.push(MacroCalendarEntry {
                kind: kind_tag.to_string(),
                label: event.kind.label().to_string(),
                date: event.date.to_string(),
                event_ts_ms: dt.with_timezone(&chrono::Utc).timestamp_millis(),
            });
        }
    }
    out
}

fn build_calendar() -> Vec<MacroEvent> {
    let mut events = Vec::new();

    // Decisões do FOMC saem às 14:00 ET no segundo dia de cada reunião.
    // Fonte: calendário oficial do Federal Reserve para 2026.
    for (y, m, d) in fomc_dates_2026() {
        events.push(MacroEvent {
            kind: EventKind::Fomc,
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            time_et: NaiveTime::from_hms_opt(14, 0, 0).unwrap(),
        });
    }

    // CPI sai às 8:30 ET, mas o dia exato do mês varia — sem um padrão de
    // dia-da-semana fixo, por isso a lista precisa vir de fonte (BLS).
    for (y, m, d) in cpi_dates_2026() {
        events.push(MacroEvent {
            kind: EventKind::Cpi,
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            time_et: NaiveTime::from_hms_opt(8, 30, 0).unwrap(),
        });
    }

    // Employment Situation (NFP) segue uma regra estável: primeira
    // sexta-feira do mês, 8:30 ET — calculado, não precisa de lista manual.
    // (Exceção rara: BLS pode adiar 1 dia por feriado federal — não
    // modelado aqui.)
    for month in 1..=12 {
        if let Some(date) = first_friday(2026, month) {
            events.push(MacroEvent {
                kind: EventKind::Nfp,
                date,
                time_et: NaiveTime::from_hms_opt(8, 30, 0).unwrap(),
            });
        }
    }

    events
}

fn first_friday(year: i32, month: u32) -> Option<NaiveDate> {
    (1..=7).find_map(|day| {
        NaiveDate::from_ymd_opt(year, month, day).filter(|d| d.weekday() == Weekday::Fri)
    })
}

/// Calendário oficial 2026 do Federal Reserve (segundo dia de cada reunião,
/// quando a decisão e o comunicado saem).
fn fomc_dates_2026() -> Vec<(i32, u32, u32)> {
    vec![
        (2026, 1, 28),
        (2026, 3, 18),
        (2026, 4, 29),
        (2026, 6, 17),
        (2026, 7, 29),
        (2026, 9, 16),
        (2026, 10, 28),
        (2026, 12, 9),
    ]
}

/// Calendário de divulgação do CPI (BLS) para 2026, referente aos meses
/// jan–nov/2026 (o CPI de dezembro só sai em janeiro de 2027).
fn cpi_dates_2026() -> Vec<(i32, u32, u32)> {
    vec![
        (2026, 2, 13),
        (2026, 3, 11),
        (2026, 4, 10),
        (2026, 5, 12),
        (2026, 6, 10),
        (2026, 7, 14),
        (2026, 8, 12),
        (2026, 9, 11),
        (2026, 10, 14),
        (2026, 11, 10),
        (2026, 12, 10),
    ]
}
