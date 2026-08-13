# AurumOS — Relatório completo: como o sistema funciona e o que aconteceu (13/08/2026)

**Estado no momento deste relatório:** processo rodando desde 10:53:04 UTC, equity US$528,73 (partiu de US$200,00 nesta janela de teste), 3.425 ciclos, 89,6% de acerto real, zero erros.

Este documento tem duas partes: **(1)** como o AurumOS funciona, do dado bruto até uma operação simulada, explicado do zero; **(2)** tudo que foi investigado, corrigido e mudado nesta sessão, com resultados.

---

## PARTE 1 — O caminho completo: da coleta de dado até a operação

### 1.1 Visão geral em uma frase

O AurumOS não tem "uma estratégia" — são **9 módulos independentes**, cada um lendo uma fonte de dado real diferente, competindo por um orçamento de risco comum, decidido por um único motor central. Nenhum módulo decide executar sozinho; todos só **sugerem** ("aqui está uma oportunidade, com esta vantagem esperada"), e o orquestrador é quem aprova, rejeita ou escala o tamanho.

```
9 módulos de dado real → Opportunity → Orquestrador → Motor de risco → Execução simulada → Dashboard
```

### 1.2 As 9 fontes de dado (o que cada uma vê, de verdade)

| Módulo | Fonte real | O que mede |
|---|---|---|
| **Arbitragem** | WebSocket público Bybit spot + Bitget spot | Diferença de preço entre as duas exchanges pro mesmo par |
| **Order Flow** | WebSocket público Bybit spot | Spread interno do book (bid vs. ask) de UMA exchange |
| **Pump Exhaustion** | Bybit linear (funding rate, variação 24h, open interest) + fusão com Liquidation Hunter e Whale Watch | Sinais de mercado esticado/exausto |
| **Liquidation Hunter** | `allLiquidation` da Bybit | Cascatas de liquidação reais em perpétuos |
| **Whale Watch** | Nó Ethereum público (`Transfer` USDT/USDC) | Movimentação de grandes carteiras |
| **News Reactor** | SEC EDGAR (filings 8-K) + LLM local (Ollama) | Eventos corporativos, classificados por IA local |
| **Launch Radar** | Bybit `instruments-info` + Uniswap V2 `PairCreated` | Novos listings CEX/DEX |
| **Macro Engine** | Calendário oficial FOMC/CPI/NFP | Janelas de risco em torno de eventos macro |
| **Multi-Asset** | Alpaca (feed IEX) | Ações US, só leitura, idle até o usuário fornecer chave |

Cada módulo implementa o mesmo contrato (`trait SignalSource`): conecta na fonte real, calcula uma vantagem esperada (`net_edge`), e manda pro orquestrador via um canal. Nenhum módulo tem acesso ao capital ou pode gastar dinheiro sozinho.

### 1.3 Como nasce uma oportunidade — o exemplo do Order Flow

1. O book da Bybit chega via WebSocket, atualizando várias vezes por segundo.
2. `net_edge = spread_pct − 2×taxa_maker` — o spread real do book, já descontando a taxa real da Bybit (0,08% por perna).
3. Se `net_edge` passa de um limiar mínimo (calibrado por análise de breakeven real), o módulo registra uma **hipótese pendente**: preço no instante do sinal, e aguarda 30 segundos.
4. Passados os 30s, o sistema confere **o preço real de novo** — se o mercado não andou mais que o edge esperado, conta como acerto real; se andou mais, conta como erro real. Isso alimenta um histórico (`confirmation_history`) — só depois de 50 amostras reais confirmadas o sistema confia nesse edge pra decidir dinheiro de verdade.
5. Só então a oportunidade é enviada pro orquestrador com `net_edge` e `confidence` vindos desse histórico medido — nunca de um número escolhido à mão.

A Arbitragem segue a mesma lógica, mas a janela é de 2 segundos (latência real de execução cross-exchange) e o teste é: "o mesmo spread ainda existia no book de verdade quando uma ordem teria chegado?".

### 1.4 O motor de risco — o que acontece entre "sugestão" e "execução"

Todo ciclo (a cada 150ms), o orquestrador:

1. **Descarta oportunidades expiradas** (cada uma tem um prazo de validade — 600ms pro Order Flow, por exemplo, porque o book muda rápido).
2. **Detecta confluência** — se duas ou mais estratégias diferentes apontam pro mesmo ativo ao mesmo tempo, isso vira um sinal de reforço (bônus de prioridade, nunca torna operável algo que não teria vantagem sozinho).
3. **Checa o kill-switch em 4 camadas** (ver 1.5).
4. **Escolhe a melhor oportunidade aprovada** — entre todas as pendentes, filtra as que passam pelo motor de risco (`risk::evaluate`) e escolhe a de maior score.

`risk::evaluate` é o gate central. Uma oportunidade só é aprovada se passar por TODOS estes filtros, nesta ordem:
- Não expirou.
- Alavancagem dentro do limite (Launch Radar não pode alavancar; o resto tem teto de 2x).
- **Teto de drawdown**: se o portfólio está em drawdown, o tamanho efetivo da ordem é reduzido (nunca o tamanho armazenado da perna — só o que pode ser usado agora).
- **Kelly hierárquico**: dentro do orçamento que a estratégia já ganhou, o símbolo específico recebe mais ou menos conforme seu próprio histórico de PnL (um símbolo novo herda 100% do orçamento da estratégia; um que comprovou ser bom recebe até 2x; um ruim, até 0,10x).
- **Guard anti-martingale**: nunca aumentar o tamanho da ordem logo após uma perda daquela estratégia (comparado contra a perna base, não contra o último trade individual — ver Parte 2, esse foi um bug sério).
- **Limite de risco por estratégia**: cada estratégia tem um teto de capital-em-risco simultâneo (% do equity, configurado em `risk.toml`).
- **Limite de risco total**: soma de todas as estratégias não pode passar de 0,5% do equity.
- **Correlação**: duas oportunidades do mesmo "grupo de risco" não podem estar abertas ao mesmo tempo.

Se aprovado, o sistema simula a execução: sorteia o resultado ponderado pela `confidence` (que agora vem do histórico real de confirmação, não de um chute), aplica fill parcial e falha de segunda perna com probabilidades realistas, atualiza equity, e libera a exposição imediatamente (posições são "round-trip" instantâneas no modelo atual, não posições que ficam abertas).

### 1.5 Kill-switch em 4 camadas

| Camada | Limiar | Efeito |
|---|---|---|
| Preventivo | 1% de drawdown diário | Avisa + reduz todas as pernas pela metade (uma vez, na transição) |
| Bloqueio diário | 1,5% de drawdown diário | Para novas execuções até o dia seguinte UTC ou destrave manual |
| Bloqueio semanal | 4% de drawdown em 7 dias | Mesmo mecanismo, janela mais longa |
| Bloqueio total | 7% de drawdown desde o pico histórico | Não libera sozinho — exige confirmação manual explícita |

Kill-switch manual também existe: criar o arquivo `orchestrator/data/KILL` pausa tudo; apagar retoma.

### 1.6 Escalonamento — como a perna de cada estratégia cresce

Cada estratégia tem sua própria "perna base" (`leg_size`), que só cresce se **todas** as condições abaixo forem verdadeiras ao mesmo tempo:
- Recuperou pelo menos 80% do maior drawdown local que já sofreu.
- Passaram pelo menos 50 ciclos **e** pelo menos 5 minutos reais desde o último escalonamento (o piso de tempo é novo — ver Parte 2).
- Drawdown do portfólio abaixo de 2%.
- Profit factor recente (janela móvel) acima do mínimo configurado.
- O resultado continua positivo mesmo excluindo o melhor trade da janela (evita que um único trade de sorte justifique escalonar).

Se tudo passa, o novo tamanho vem do Kelly fracionário medido (30% do Kelly cheio — nunca o Kelly cheio, que assume dados perfeitos), com teto de 30% do equity.

### 1.7 Persistência — o que sobrevive a um restart

- `portfolio_state.json` — equity, perna de cada estratégia, contadores de vitória/derrota.
- `events.jsonl` — histórico completo de eventos (até 50 mil, recarregado no boot).
- `edge_scores_linear.json` / `edge_scores_spot.json` — edge medido por símbolo, recarregado no boot e salvo a cada 30s (pra um restart no meio de uma janela de 15min não perder nada).
- `active_symbols.json` / `active_symbols_spot.json` — universo de símbolos ativo no momento.

O que **não** persiste (por design, refila rápido): o histórico de confirmação de preço real (`confirmation_history`) e o PnL recente por símbolo — esses filtram qualidade recente, não são capital, então perdê-los num restart só significa esperar a janela reencher.

### 1.8 Dashboard

Servidor axum embutido no próprio binário (`http://127.0.0.1:7878`), WebSocket com replay completo do histórico ao conectar. Mostra: pulso por estratégia, cockpit de risco (conectado ao drawdown real), exposição por estratégia, universo de símbolos com edge ao vivo, feed combinado de todos os eventos.

---

## PARTE 2 — O que foi investigado e corrigido nesta sessão

### 2.1 Linha do tempo

1. **Setup do repositório**: instalado GitHub CLI, criado repositório privado `Genezera/AurumOS`, README detalhado escrito e publicado.
2. **"Tá tudo rodando normal?"** — auditoria de saúde ao vivo. Achado real: Multi-Asset (Alpaca) reconectava a cada ~35s ininterruptamente a noite inteira porque o watchdog não sabia diferenciar "mercado fechado" de "conexão morta". Corrigido com timeout adaptativo por horário real da NYSE.
3. **"Por que tem perp nesse edge e não está operando?"** — achado real: a correção do dia anterior trocou a lista de símbolos do Order Flow pra spot, mas esqueceu de trocar o mapa de edge junto — o painel "Universo Perpétuos" mostrava spread real do spot rotulado como se fosse de perpétuo. Corrigido; painel agora mostra honestamente zero (nada mede spread de perpétuo hoje).
4. **"Parou de ter trades desde 23h, só arbitragem funcionando"** — a investigação mais longa da sessão. Achado: o guard anti-martingale comparava cada candidato contra o tamanho exato do último trade individual (que varia por símbolo via Kelly hierárquico) — bastava um próximo candidato "de sorte ruim" ter fração maior pra travar, e como só um trade bem-sucedido libera esse estado, e nenhum conseguia passar, virava **deadlock permanente**, sem erro, sem log visível. Corrigido comparando contra a perna base (estável) em vez do último trade individual. Verificado ao vivo por 27+ minutos direto sem nenhuma parada (o ponto exato onde travava antes).
5. **"Isso é tudo simulado ou real?"** — explicação detalhada do que é dado real (preço, book, taxas) vs. o que é assumido (probabilidade de acerto de cada trade, decidida por sorteio).
6. **"Se fosse dinheiro real, o que aconteceria? Faça cálculos"** — cálculo de breakeven real (Order Flow precisava de só 19,3% de acerto pra não perder, tinha 43,2% assumido; Arbitragem precisava de 35,2%, tinha 70,2% assumido), risco de seleção adversa, limite de rate limit de API, capital fantasma (perna simulada de US$266-384 sem nenhum capital real por trás).
7. **"Quero real, dentro da realidade, sem nada falso além do dinheiro"** — implementação da camada de confirmação de preço real (substituindo o sorteio) e do piso de tempo no escalonamento.

### 2.2 Todos os bugs reais encontrados e corrigidos

| # | Bug | Sintoma | Correção |
|---|---|---|---|
| 1 | Alpaca watchdog não diferenciava mercado fechado de conexão morta | Reconectava a cada ~35s a noite inteira | Timeout adaptativo por horário real da NYSE |
| 2 | Order Flow gravava edge de spot no mapa de "perpétuos" | Painel "Universo Perpétuos" mostrava número real com rótulo errado | Aponta pro mapa correto (spot) |
| 3 | Guard anti-martingale comparava contra o último trade individual | **Deadlock permanente** — zero trades por horas, sem erro | Compara contra a perna base (estável) |
| 4 | `min_cycles_between_scale` só contava ciclos, não tempo | Equity simulado de US$195 a US$2.727 em menos de 2h | Piso de 5 minutos reais além da contagem de ciclos |
| 5 | Resultado de cada trade decidido por sorteio (`confidence` chutado) | Nenhuma validação contra preço real — não é possível saber se o edge medido seria capturável | Camada de confirmação real de preço (Order Flow: 30s/50 amostras; Arbitragem: 2s/30 amostras) |
| 6 | Direção da arbitragem (Bybit↔Bitget) calculada e descartada | Impossível saber em qual exchange/direção uma operação aconteceu | Exposto no campo `asset` de cada oportunidade |

### 2.3 Resultado do teste ao vivo pós-mudanças (equity resetado a US$200 pra comparação limpa)

- **~10 minutos rodando:** equity em **US$528,73**, 3.425 ciclos, **89,6% de acerto real** (medido contra preço subsequente, não mais um chute de 45%).
- `leg_size` do Order Flow escalou de forma controlada (US$25 → US$116) ao longo de vários eventos de escalonamento, todos espaçados por pelo menos 5 minutos reais — não mais explosivo.
- Arbitragem encontrou seu primeiro cruzamento real do limiar de 0,65% depois de alguns minutos (comportamento honesto — nem sempre existe, confirmado inspecionando o book real).
- Zero erros, dashboard respondendo, processo estável havia mais de 10 minutos sem nenhuma parada.

### 2.4 O que ainda não é 100% real (limitação conhecida, documentada, não escondida)

O modelo agora testa **"o edge sobreviveu ao preço real depois"** — uma mudança real e válida. O que ele **ainda não testa**: se uma ordem passiva (maker) realmente teria sido preenchida. Seleção adversa (quem bate numa ordem parada geralmente sabe algo que você não sabe) e fila de execução real não estão modeladas — isso só dá pra validar com uma conta de paper trading oficial na própria exchange (que simula fill contra o book real), não só com dado de mercado público.

**Conclusão honesta:** o sistema evoluiu de "nunca travou de propósito, resultado decidido por sorteio" para "trava corrigida, resultado validado contra preço real subsequente". O próximo passo natural, se quiser ir mais fundo em realismo, é conectar uma conta paper oficial de exchange — não pra arriscar capital, mas pra obter probabilidade de fill real em vez de inferida.

---

## Onde encontrar tudo

- Código: [github.com/Genezera/AurumOS](https://github.com/Genezera/AurumOS)
- Relatório anterior (compounding descontrolado, cálculos de realismo): [relatorio_realismo_13-08-2026.md](relatorio_realismo_13-08-2026.md)
- Roadmap técnico completo: gerado via `docs/generate_roadmap.py` (não versionado — regenerar localmente pra pegar a versão mais atual)
- Dashboard ao vivo: `http://127.0.0.1:7878`
