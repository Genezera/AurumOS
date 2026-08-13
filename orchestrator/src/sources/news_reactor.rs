use std::collections::{HashSet, VecDeque};
use std::time::Instant;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio::time::{sleep, Duration};

use crate::sources::SignalSource;
use crate::types::{Direction, Market, Opportunity, Strategy};

// Feed Atom oficial de filings recentes — não é push em tempo real (a SEC
// não oferece WebSocket para isso), então fazemos polling. 45s fica bem
// abaixo do limite de 10 req/s da SEC e ainda é rápido pra um 8-K novo.
const EDGAR_URL: &str = "https://www.sec.gov/cgi-bin/browse-edgar?action=getcurrent&type=8-K&company=&dateb=&owner=include&count=100&output=atom";
const POLL_INTERVAL: Duration = Duration::from_secs(45);
const SEEN_CAP: usize = 800;

// LLM local via Ollama (http://localhost:11434) — gratuito, sem chave, sem
// custo por chamada, roda inteiramente na máquina do usuário (dado
// financeiro nunca sai da máquina). Nome do modelo registrado em cada
// evento (Seção 5 do roadmap — governança de IA: toda decisão rastreia qual
// versão de modelo a gerou).
const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const OLLAMA_MODEL: &str = "llama3.2:3b";
// Generoso de propósito: a primeira chamada depois do Ollama ficar ocioso
// recarrega o modelo do zero (~1min em CPU no teste local); chamadas
// seguintes com o modelo já quente respondem em <1s. Um timeout perdido
// aqui só significa "esse filing específico fica sem classificação
// automática" — cai no fallback neutro, nunca trava o resto do sistema.
const OLLAMA_TIMEOUT: Duration = Duration::from_secs(45);

struct Entry {
    id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct OllamaClassification {
    direction: String,
    confidence: f64,
}

/// Resultado de classificar um filing — `None` quando o Ollama está
/// indisponível ou respondeu algo não parseável. Nesse caso o chamador cai
/// no comportamento neutro de sempre (fallback determinístico, exigido pela
/// Seção 5 do roadmap — nenhum modelo é obrigatório para o sistema
/// continuar funcionando).
async fn classify_with_ollama(client: &reqwest::Client, title: &str) -> Option<OllamaClassification> {
    let prompt = format!(
        "Voce e um classificador financeiro objetivo. Dado o titulo de um filing 8-K \
         da SEC, responda APENAS com um JSON no formato {{\"direction\": \"up\" | \"down\" | \"neutral\", \"confidence\": 0.0 a 1.0}}. \
         Nao invente informacao que nao esta explicita no titulo — se o titulo nao \
         permitir concluir uma direcao provavel, responda neutral com confidence baixa (0.1 a 0.3). \
         Titulo do filing: \"{title}\""
    );
    let body = serde_json::json!({
        "model": OLLAMA_MODEL,
        "prompt": prompt,
        "stream": false,
        "format": "json",
    });

    let resp = tokio::time::timeout(OLLAMA_TIMEOUT, client.post(OLLAMA_URL).json(&body).send())
        .await
        .ok()?
        .ok()?;
    let json: Value = resp.json().await.ok()?;
    let raw = json.get("response").and_then(Value::as_str)?;
    serde_json::from_str::<OllamaClassification>(raw).ok()
}

/// Fase 3 do roadmap: monitora filings 8-K novos na SEC EDGAR em quase
/// tempo real (via polling do feed Atom oficial, sem chave de API). Sem
/// classificação por IA nesta fase — isso é uma decisão pendente do roadmap
/// (custo por chamada de LLM). Por enquanto, todo filing novo vira um
/// evento estruturado e visível no dashboard com `net_edge = 0.0`: assim
/// como o Whale Watch, uma notícia sozinha não é um sinal de preço
/// confiável — precisa da camada de confirmação por movimento real de
/// preço, que ainda não existe nesta fase (ver roadmap).
pub struct NewsReactorSource;

#[async_trait::async_trait]
impl SignalSource for NewsReactorSource {
    fn name(&self) -> &'static str {
        "news_reactor_sec_edgar"
    }

    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()> {
        // Substitua pelo seu contato real — a SEC exige um User-Agent
        // identificável em toda chamada programática à EDGAR e pode
        // bloquear por IP quem não seguir isso.
        let client = reqwest::Client::builder()
            .user_agent("AurumOS-ResearchBot/0.1 (contato: defina-seu-email-aqui@exemplo.com)")
            .timeout(Duration::from_secs(15))
            .build()?;

        let mut seen_order: VecDeque<String> = VecDeque::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut first_poll = true;

        loop {
            match poll_once(&client).await {
                Ok(entries) => {
                    let mut new_count = 0;
                    for entry in entries {
                        if seen.contains(&entry.id) {
                            continue;
                        }
                        seen.insert(entry.id.clone());
                        seen_order.push_back(entry.id.clone());
                        while seen_order.len() > SEEN_CAP {
                            if let Some(old) = seen_order.pop_front() {
                                seen.remove(&old);
                            }
                        }

                        if first_poll {
                            // Primeira leitura só estabelece a linha de base
                            // (o feed já vem com até 100 filings recentes) —
                            // não emitimos isso como "ao vivo", só o que
                            // aparecer dali pra frente.
                            continue;
                        }
                        new_count += 1;
                        emit(&tx, &client, &entry).await;
                    }
                    if first_poll {
                        tracing::info!(baseline = seen.len(), "news reactor: linha de base da EDGAR estabelecida");
                    } else if new_count > 0 {
                        tracing::info!(new_count, "news reactor: novos filings 8-K detectados");
                    }
                    first_poll = false;
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "falha ao consultar EDGAR, tentando de novo no próximo ciclo");
                }
            }
            sleep(POLL_INTERVAL).await;
        }
    }
}

async fn poll_once(client: &reqwest::Client) -> anyhow::Result<Vec<Entry>> {
    let resp = client.get(EDGAR_URL).send().await?.error_for_status()?;
    let body = resp.text().await?;
    parse_atom_entries(&body)
}

fn parse_atom_entries(xml: &str) -> anyhow::Result<Vec<Entry>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut entries = Vec::new();

    let mut in_entry = false;
    let mut cur_tag = String::new();
    let mut title = String::new();
    let mut link = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "entry" {
                    in_entry = true;
                    title.clear();
                    link.clear();
                }
                if in_entry && name == "link" {
                    if let Some(attr) = e.attributes().flatten().find(|a| a.key.as_ref() == b"href") {
                        link = String::from_utf8_lossy(&attr.value).to_string();
                    }
                }
                cur_tag = name;
            }
            Ok(Event::Text(t)) => {
                if in_entry && cur_tag == "title" {
                    title.push_str(t.unescape().unwrap_or_default().trim());
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "entry" {
                    in_entry = false;
                    if !title.is_empty() {
                        let id = if link.is_empty() { title.clone() } else { link.clone() };
                        entries.push(Entry { id, title: title.clone() });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("erro ao parsear XML da EDGAR: {e}"),
            _ => {}
        }
        buf.clear();
    }
    Ok(entries)
}

async fn emit(tx: &Sender<Opportunity>, client: &reqwest::Client, entry: &Entry) {
    let title_short: String = entry.title.chars().take(60).collect();
    let classification = classify_with_ollama(client, &entry.title).await;

    // Fallback determinístico (Seção 5 do roadmap — governança de IA: o
    // sistema nunca depende de o modelo estar disponível): sem
    // classificação, mantém o comportamento neutro de sempre.
    let (direction, confidence, asset, model_tag) = match &classification {
        Some(c) => {
            let dir = if c.direction.eq_ignore_ascii_case("down") { Direction::Short } else { Direction::Long };
            let conf = c.confidence.clamp(0.0, 1.0);
            let asset = format!("{title_short} [{OLLAMA_MODEL}: {} {:.2}]", c.direction, conf);
            (dir, conf.max(0.2), asset, Some(OLLAMA_MODEL))
        }
        None => (Direction::Long, 0.25, title_short, None),
    };

    tracing::info!(
        filing = %entry.title,
        classificado_por = ?model_tag,
        direction = ?direction,
        confidence,
        "novo filing 8-K detectado"
    );

    // net_edge continua 0.0 mesmo com classificação real: a Seção 0 do
    // roadmap exige confirmação por preço/livro antes de qualquer sinal
    // isolado virar ordem, e a Seção 3 (Fase 3) exige validação
    // retrospectiva documentada da taxa de acerto do classificador antes de
    // ele contar como vantagem numérica de verdade — nenhuma das duas
    // existe ainda, então isso continua 100% informativo.
    let opp = Opportunity {
        market: Market::Stocks,
        strategy: Strategy::News,
        asset,
        direction,
        net_edge: 0.0,
        confidence,
        valid_for_ms: 120_000,
        // Informativo (net_edge=0) — placeholder consistente com valid_for_ms.
        expected_holding_secs: 120.0,
        capital_needed: 10.0,
        max_loss_pct: 0.01,
        leverage: 1.0,
        correlation_group: "news_driven".to_string(),
        sampled_return: None,
        emitted_at: Instant::now(),
    };
    let _ = tx.send(opp).await;
}
