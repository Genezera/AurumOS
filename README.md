<div align="center">

<img src="docs/brand/aurumos-mark.svg" width="96" height="96" alt="AurumOS" />

# AurumOS

**Orquestrador multi-estratégia de trading quantitativo — 100% paper trading, dado real, sem mock.**

![status](https://img.shields.io/badge/status-paper--trading-f5c453?style=flat-square&labelColor=0c1117)
![linguagem](https://img.shields.io/badge/core-Rust-f5c453?style=flat-square&labelColor=0c1117)
![capital real](https://img.shields.io/badge/capital%20real-nunca-ff4d67?style=flat-square&labelColor=0c1117)
![dashboard](https://img.shields.io/badge/dashboard-axum%20%2B%20WebSocket-18d7ff?style=flat-square&labelColor=0c1117)
![licença](https://img.shields.io/badge/uso-projeto%20pessoal-aab6c3?style=flat-square&labelColor=0c1117)

</div>

---

## O que é

AurumOS é um orquestrador de trading que roda **9 estratégias independentes em paralelo**, cada uma lendo dado real de mercado (Bybit, Bitget, Ethereum on-chain, SEC EDGAR, Alpaca/IEX), competindo por um orçamento de risco comum através de um motor central de decisão. Nada aqui é simulado ou sintético: os preços, spreads, liquidações, filings e transações on-chain são reais — apenas a execução é em paper trading, sem dinheiro de verdade conectado.

O objetivo não é uma única estratégia "vencedora", e sim um **sistema que decide sozinho onde alocar risco** entre oportunidades de naturezas completamente diferentes — desde arbitragem de milissegundos até eventos macro que acontecem uma vez por mês — usando o mesmo motor de risco, o mesmo sizing e o mesmo kill-switch para todas.

> Regra permanente do projeto: **nunca conecta capital real, nunca cria contas em nome do usuário**, independentemente de qualquer autonomia concedida. Todo o histórico de operações abaixo é paper trading.

---

## Arquitetura

```
                        ┌─────────────────────────────────────────┐
                        │            9 SignalSource                │
                        │  (cada um lê dado real, emite            │
                        │   Opportunity num canal async comum)     │
                        └───────────────────┬───────────────────────┘
                                             │ mpsc::Sender<Opportunity>
                                             ▼
                        ┌─────────────────────────────────────────┐
                        │   Orquestrador (orchestrator.rs)          │
                        │   • motor de confluência (fusion.rs)      │
                        │   • scorer por velocidade/edge             │
                        │   • concorrência por orçamento de risco    │
                        └───────────────────┬───────────────────────┘
                                             │
                        ┌────────────────────┴────────────────────┐
                        ▼                                          ▼
        ┌───────────────────────────┐            ┌───────────────────────────┐
        │  Motor de risco (risk.rs)  │            │  EventBus (events.rs)     │
        │  • Kelly hierárquico        │            │  • backlog + replay ao     │
        │  • kill-switch em 4 camadas │            │    vivo via WebSocket      │
        │  • escalonamento por        │            │  • persistido em JSONL     │
        │    estratégia/símbolo       │            └─────────────┬─────────────┘
        └─────────────────────────────┘                          ▼
                                                    ┌───────────────────────────┐
                                                    │  Dashboard (axum)          │
                                                    │  http://127.0.0.1:7878     │
                                                    └───────────────────────────┘
```

Cada módulo implementa o mesmo contrato (`trait SignalSource` em [`sources/mod.rs`](orchestrator/src/sources/mod.rs)): lê dado real, emite `Opportunity`, nunca decide sozinho se executa. Toda decisão de execução, sizing e corte por risco passa por um único motor central — isso é o que permite comparar uma oportunidade de arbitragem de 3 segundos com um sinal de whale watch que dura horas usando a mesma régua.

---

## As 9 estratégias

Só 3 delas conseguem executar trade de verdade — as outras 6 são estruturalmente informativas: emitem `net_edge = 0.0` sempre, o que faz `score()` ser sempre zero, e `pick_best()` descarta qualquer candidato com score ≤ 0 antes mesmo de chegar no motor de risco. Não é uma convenção, é garantido em código.

| Estratégia | Caminho | Fonte de dado | O que captura |
|---|---|---|---|
| **Arbitragem** | Executa | Book Bybit ↔ Bitget (spot) | Divergência de preço executável entre exchanges |
| **Order Flow** | Executa | Book Bybit (spot) | Captura de spread como market maker |
| **Pump Exhaustion** | Executa* | Funding + OI + liquidações da Bybit (perpétuos) | Exaustão de movimento (fusão de whale watch on-chain + liquidation hunter) |
| **Whale Watch** | Observação | On-chain Ethereum (Transfer USDT/USDC) | Movimentação de grandes carteiras rotuladas |
| **Liquidation Hunter** | Observação | `allLiquidation` da Bybit | Cascatas de liquidação em perpétuos |
| **News Reactor** | Observação | SEC EDGAR (filings 8-K ao vivo) + LLM local (Ollama) | Reação a eventos corporativos classificados por IA local, sem chave, sem custo |
| **Launch Radar** | Observação (trava estrutural adicional) | Bybit `instruments-info` + `PairCreated` Uniswap V2 | Novos listings CEX/DEX, com checklist de holders/LP |
| **Macro Engine** | Observação | Calendário oficial FOMC/CPI/NFP | Janelas de volatilidade em torno de eventos macro |
| **Multi-Asset** | Observação | Alpaca (IEX) | Ações via feed gratuito, idle até o usuário fornecer chave própria |

*Pump Exhaustion tem a mesma camada de confirmação de preço real, taxa de execução descontada e reamostragem por bootstrap que Arbitragem/Order Flow já usam — mas ainda não acumulou amostra suficiente pra sair de `net_edge=0.0`: 0 trades executados até agora.

Whale Watch e Liquidation Hunter foram fundidos operacionalmente ao Pump Exhaustion — três fontes de dado independentes alimentando um único sinal de exaustão, em vez de três estratégias competindo pelo mesmo risco.

---

## Motor de risco

- **Exposição simultânea real**: posições ficam genuinamente reservadas entre a aprovação e a resolução (30s Order Flow, 2s Arbitragem, 20min Pump Exhaustion) — não mais round-trip instantâneo. Os limites de risco por estratégia/grupo/portfólio agora bloqueiam por excesso SIMULTÂNEO de verdade, não só por um trade grande demais sozinho.
- **Kelly hierárquico** (portfólio → estratégia → símbolo): a fração de capital por operação é ponderada pelo histórico de PnL em cada nível, com blend automático conforme a amostra por símbolo cresce. A taxa de acerto usada no cálculo é o limite inferior de confiança de Wilson (95%), não o valor pontual — menos sujeito a excesso de confiança em amostra pequena. Símbolos da mesma estratégia são normalizados entre si pra não reivindicarem 2x cada simultaneamente.
- **Hierarquia de profit factor**: PF ≥ 1,3 autoriza crescer a perna; PF < 1,2 força uma redução ativa; entre os dois, mantém o tamanho atual.
- **Gate de escalonamento por evidência**: crescer também exige orçamento de risco livre (< 50% do limite da estratégia em uso) e diversidade de símbolos (lucro vindo de pelo menos 2 símbolos distintos), além do piso de tempo/ciclos.
- **Teto global de alavancagem**: soma do notional alavancado de todas as posições abertas, dividido pelo equity, nunca passa de 1,0x configurado (2,0x é o teto absoluto no código, não editável).
- **Escalonamento não-destrutivo por drawdown**: em vez de reduzir o tamanho da perna permanentemente a cada operação em drawdown, um teto (`drawdown_ceiling_multiplier`) recalculado a cada ciclo aplica a redução como multiplicador — sem colapsar o sizing ao piso.
- **Recuperação parcial (80%)**: escalonar de volta para cima não exige recuperar 100% do pico de equity, só 80% da queda desde o pico.
- **Kill-switch em 4 camadas**: aviso preventivo (1% diário, reduz perna pela metade), bloqueio rígido diário (1,5%), bloqueio semanal (4%), bloqueio total por drawdown histórico (7%, exige reset manual). Só bloqueia NOVAS aberturas — posições já abertas resolvem sozinhas no prazo natural (no máximo 30s hoje).
- **Rate limiter real por venue**: token bucket (8 requisições/s), simulando o limite de uma conta de varejo comum — consumido só quando uma ordem é de fato aberta.
- **Notional mínimo real por símbolo**: buscado ao vivo da própria API pública da exchange no boot (Bybit e Bitget); ordem abaixo do mínimo real é rejeitada.
- **Reamostragem por bootstrap**: o resultado de cada trade (Arbitragem, Order Flow, Pump Exhaustion) é sorteado de um desfecho REAL já confirmado contra preço, não de uma fórmula `confidence×net_edge`.
- **Deduplicação de sinais sobrepostos**: janelas de confirmação sobrepostas do mesmo símbolo não contam como amostras independentes — só uma confirmação em voo por símbolo por vez.
- **Trava estrutural do Launch Radar**: independente do gate de score (que já bloqueia por net_edge=0), uma flag no código impede execução de lançamentos de token — o contexto de menor liquidez do sistema.
- **Concorrência por orçamento de risco**: sem tabela fixa de "N trades simultâneos por faixa de capital" — o motor de correlação e risco total (`risk::evaluate`) decide, reavaliado a cada ciclo.
- **Universo de símbolos dinâmico**: rotação a cada 15 minutos por todo o mercado disponível (perpétuos e spot separadamente), ranqueado por edge observado, com refresh de exibição a cada 5 segundos.

Todo o estado de risco (posições, PnL por estratégia/símbolo, edge scores por símbolo, histórico de confirmação de preço) é persistido em disco e recarregado no boot — reiniciar o processo não zera o progresso acumulado.

---

## Dashboard

Servidor axum embutido no próprio binário, servindo um frontend vanilla JS/HTML (sem framework) com o brand kit oficial do AurumOS. WebSocket com replay de backlog completo ao conectar — abrir o dashboard nunca perde histórico.

```
http://127.0.0.1:7878
```

Painéis: pulso por estratégia (com sparkline de PnL para as que já operaram, e explicação honesta de por que as outras ainda estão dormentes), cockpit de risco (kill-switch, drawdown, parada manual), exposição em tempo real por estratégia, universo de símbolos ao vivo com edge ranqueado, feed de eventos.

---

## Como rodar

```bash
# build (a partir da raiz do repositório)
cargo build --release

# executar (precisa rodar de dentro de orchestrator/, onde fica config/risk.toml)
cd orchestrator
../target/release/orchestrator.exe
```

Variáveis de ambiente opcionais (tudo tem fallback público/gratuito se omitido):

| Variável | Módulo | Padrão sem ela |
|---|---|---|
| `AURUMOS_ETH_WS_URL` / `AURUMOS_ETH_HTTP_URL` | Whale Watch, Launch Radar DEX | Nó RPC público (publicnode.com) |
| `ALPACA_API_KEY_ID` / `ALPACA_API_SECRET_KEY` | Multi-Asset | Módulo fica idle |

**Ollama local** (opcional, não é variável de ambiente): News Reactor usa `llama3.2:3b` via `http://localhost:11434` pra classificar filings da SEC. Sem o Ollama rodando, cai num fallback neutro determinístico — nunca inventa uma direção, só fica sem a classificação real.

**Kill-switch manual**: criar o arquivo `orchestrator/data/KILL` (qualquer conteúdo) pausa novas execuções; apagar retoma. Não cancela posições já abertas — elas resolvem sozinhas no prazo natural (no máximo 30s hoje).

---

## Estrutura do repositório

```
orchestrator/          núcleo Rust — orquestrador, motor de risco, dashboard, 9 fontes de sinal
  src/sources/          um módulo por estratégia, todos implementando SignalSource
  config/risk.toml       toda a configuração de capital e risco, documentada inline
  dashboard/             frontend vanilla JS/HTML embutido no binário
backtests/              scripts Python de análise sobre dado real coletado (events.jsonl, raw logs)
docs/                   gerador do roadmap técnico (generate_roadmap.py, reportlab) + brand kit
```

O roadmap técnico completo (arquitetura detalhada, histórico de auditorias, bugs encontrados e corrigidos, próximos passos) é gerado localmente a partir de `docs/generate_roadmap.py` e não é versionado neste repositório — rode o script para obter a versão mais atual em PDF.

---

## Estado do projeto

Projeto pessoal, em desenvolvimento ativo e solo. Cada estratégia evolui de "instrumentada" para "confiável" com base em dado real acumulado em paper trading — não existe atalho de engenharia para a passagem de tempo que isso exige. Bugs reais já encontrados e corrigidos incluem corrupção de eventos concorrentes, erros de cálculo de breakeven, universos de símbolos incompatíveis com a exchange conectada, redução de risco destrutiva sob drawdown, um book congelado da Bitget sendo tratado como cotação executável (gerando "100% de acerto" falso num símbolo), e um deadlock de correlação que travava o sistema inteiro assim que a exposição simultânea passou a ser real — cada um documentado no histórico de commits no momento em que foi corrigido.

Resultado numérico mais recente (todo o histórico acumulado, não uma janela limpa pós-correção): Arbitragem 87,9% de acerto / +US$1.002,79, Order Flow 90,9% / +US$2.268,81, Pump Exhaustion ainda sem trade (acumulando amostra). Ver o PDF (`docs/generate_roadmap.py`) para o detalhamento completo, inclusive o aviso honesto de que esse número mistura código antigo (menos realista) com o mais recente.
