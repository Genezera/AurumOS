# Contexto completo de continuidade — AurumOS

## Atualização de 06/09/2026 — auditoria e integração do Snowball

- Repositório auditado: `C:\Users\Renan\Projetos\Snowball`, branch `main`, commit observado `323ae99`.
- O Snowball não tinha chave de corretora nem executor: somente Telegram no `.env`; todos os resultados eram paper.
- Seus 100 testes unitários passaram, mas o estado final observado cobre aproximadamente dois dias. Foram 29 aberturas, 27 fechamentos, 3 ganhos e 24 perdas; PnL paper fechado agregado de +US$ 0,5834, concentrado em três pagamentos grandes. O submotor spot-perp fechou 8 operações, ganhou 0 e perdeu aproximadamente US$ 0,38.
- A tese principal Bybit+Bitget foi rejeitada para o AurumOS porque viola a regra de uma corretora. A parte reaproveitável foi o cash-and-carry spot+perp dentro da Bybit.
- Foi criado `orchestrator/src/sources/funding_carry.rs`: coleta tickers spot/linear da Bybit, cruza pares, mede quatro lados do L1, turnover, basis, custo base e estressado, horário/intervalo real e funding liquidado histórico. É `research_observation_only`; não possui caminho de ordem.
- O mesmo módulo agora mantém `funding_carry_forward_v1.jsonl`: congela uma previsão por símbolo/settlement quando falta no máximo 30 minutos e reconcilia automaticamente a taxa prevista com o `fundingRateTimestamp` liquidado. O relatório vivo expõe contagens e erros; a amostra começa vazia e precisa atravessar settlements reais.
- Primeiro scan real: 290 pares comuns spot/perp, 247 com funding positivo, 0 passaram o filtro atual, 10 históricos consultados, 0 passaram o filtro histórico. O melhor payback estressado era 12,8566 períodos, acima do teto pré-registrado de 3.
- A conta Bybit Demo foi configurada em `orchestrator/.env`, que está ignorado pelo Git. O preflight autenticado passou. Nunca copiar os valores para documentação, logs, comandos ou commits.
- A persistência do portfólio agora usa envelope v1 com checksum SHA-256, escrita sincronizada em temporário, rotação do estado anterior para `.prev` e recuperação automática do backup. Valores financeiros não finitos/negativos ou checksum divergente são rejeitados. O histórico de eventos só é limpo quando nenhuma cópia do estado é recuperável.
- Em 06/09, a medição de 5 segundos tinha 8 de 197 símbolos com ao menos 200 amostras e zero aprovados; os melhores LCB95 líquidos estavam próximos de −0,11%, o custo taker completo. O Order Flow passou a medir 5/15/30/60 segundos em paralelo, com histórico v3 e gate independente por símbolo+horizonte. O v2 é migrado somente para a faixa de 5 segundos e permanece preservado.
- A primeira leitura multi-horizonte v3 confirmou retorno médio perto de −0,11% também em 15/30/60 segundos: o saldo estático da fila não previa movimento bruto. O piloto v4 implementou OFI dinâmico do melhor bid/ask. A v8 acrescentou custo por classe pública e rótulos completos, mas ainda podava partes da janela contra o relógio local. O modelo ativo v9 usa exclusivamente book `cts` e trade `T` do matching engine na janela; v2–v8 ficaram preservados como baselines e não promovem ordens v9.
- A API Demo não oferece `/v5/account/fee-rate`, fato confirmado tanto pelo `retCode=10001` observado quanto pela lista oficial de endpoints Demo. A v8 usa `/v5/market/fee-group-info`: Major/Altcoin 0,055% taker por lado, Pre-listing 0,10% e Innovation Zone 0,11%; desconhecidos usam 0,11% e TradFi é excluído do universo cripto.
- Foi criado o pesquisador maker `maker_entry_research.rs`. Ele não envia oportunidades ou ordens. A v3 assume 250 ms de latência de colocação, exige o mesmo melhor preço na chegada e só considera fill quando trades públicos futuros do lado oposto consomem a fila visível na chegada mais toda a ordem de US$ 25 até 500 ms após o sinal. Depois mede saída taker em 5/15/30/60 segundos e mantém `execution_enabled=false`.
- Runtime atual: modo `bybit_demo`, dashboard `http://127.0.0.1:7878`, equity local US$ 200, 0 wins, 0 losses e nenhuma ordem/fill contabilizado na largada.
- Relatório detalhado: `docs/audit_2026_09_06/COMPARACAO_SNOWBALL_E_INTEGRACAO.md`.

### Snapshot operacional de 06/09/2026 após a v8

- Um único `orchestrator.exe` estava ativo e o dashboard respondia HTTP 200.
- O universo continha 220 contratos cripto válidos, sem TradFi; `BONKUSDT`/`FLOKIUSDT` inválidos foram substituídos por `1000BONKUSDT`/`1000FLOKIUSDT` na semente.
- `microstructure_v8`: 2.410 amostras distribuídas por 463 pares símbolo+horizonte, maior série com 66/200 e zero aprovados.
- As coortes taker ainda mostravam retorno líquido negativo. O grupo mais promissor no snapshot era força ≥0,90 a 60 segundos, com média líquida positiva em apenas seis observações e LCB95 negativo; isso não é evidência suficiente e não alterou os limiares.
- `maker_entry_v1`: 302 tentativas, 26 fills conservadores, fill rate de 8,61%, 84 saídas resolvidas e zero gates de pesquisa aprovados.
- Equity local permaneceu US$ 200, sem ciclos, wins, losses ou fills Demo. O runtime está acelerando coleta e comparação de modelos; ele ainda não encontrou vantagem líquida autorizada para começar a enviar ordens.
- Depois desse snapshot foi descoberta a mistura parcial de `cts`/`T` com relógio local na poda da janela. A v8 foi congelada como exploratória; v9 e maker v2 começaram limpos, com o relógio da exchange usado de ponta a ponta. Na primeira gravação: 49 amostras taker v9, duas saídas maker v2, zero gates aprovados e equity ainda em US$ 200. O maker v2 foi em seguida congelado porque não modelava a latência de colocação; maker v3 é o ativo.

### O que continuar

1. Manter a v9 e o maker v3 rodando até formar séries de ao menos 200 amostras por símbolo+horizonte em vários horários e regimes.
2. Usar as coortes rotuladas para pré-registrar um modelo sucessor somente depois de volume suficiente, preservando a v9 como conjunto exploratório e reservando um teste temporal final sem ajustes.
3. Comparar maker e taker por retorno líquido, fill rate, latência estimada, concentração e estabilidade sem o melhor trade. Não promover maker apenas por taxa menor.
4. Quando uma série taker passar automaticamente o LCB95, acompanhar a primeira campanha Demo por fill real, `execFee`, slippage, stop e reconciliação. O PnL local só muda a partir desses fills.
5. Se nenhum horizonte superar o custo, testar uma nova hipótese separada, como OFI multinível ou TTLs maker pré-registrados, em novo schema; não reduzir amostra mínima nem apagar taxas para forçar operações.

### Leitura antecipada da v9 (Claude Code, 06/09 ~17h) — decomposição spread vs. previsão

Enquanto a coleta v9 ainda estava fina (3.506 amostras, maior série 39/200, nenhum
gate elegível), esta sessão fez uma análise **agregada** (todos os símbolos juntos)
só para saber se o sinal novo tem poder preditivo — não para promover nada.

Verificação de integridade primeiro: 532 séries no histórico bruto batem com 532
linhas no relatório, zero divergência de contagem; 3.506 amostras, nenhuma com taxa
implícita ≤ 0; as taxas implícitas observadas são exatamente 0,11% e 0,22%,
confirmando que a classificação por classe pública de taxa está sendo aplicada de
verdade sobre dado real (Innovation Zone cobrada em dobro, como deveria).

Decomposição (mid ≈ retorno bruto + spread cruzado, já que a entrada cruza o book):

| Horizonte | n | Spread médio | Bruto | Mid-a-mid | LCB95 mid | t |
|---|---:|---:|---:|---:|---:|---:|
| 5s | 1.510 | 0,0352% | −0,0200% | **+0,0152%** | +0,0090% | 4,84 |
| 15s | 1.034 | 0,0381% | −0,0326% | +0,0056% | −0,0043% | 1,10 |
| 30s | 768 | 0,0397% | −0,0251% | +0,0146% | +0,0001% | 1,97 |
| 60s | 532 | 0,0416% | −0,0173% | +0,0243% | +0,0008% | 2,03 |

**O sinal OFI prevê de verdade.** O bruto negativo era o spread, não erro de direção:
em 5s o movimento do mid é +0,0152% com LCB95 positivo e t=4,84. O problema é de
magnitude: o custo taker é ~0,145% (0,035% spread + 0,11% taxa) e mesmo o melhor caso
maker-entrada/taker-saída fica em ~0,09%. O sinal entrega ~1/6 a 1/10 do necessário.

Coortes por força do sinal (5s / 60s), para testar se sinal forte prevê mais:

| Faixa de força | mid 5s | t | mid 60s | t |
|---|---:|---:|---:|---:|
| 0,0–0,3 | +0,0177% | 3,72 | +0,0341% | 1,74 |
| 0,3–0,5 | +0,0205% | 4,50 | +0,0248% | 1,55 |
| 0,5–0,7 | +0,0015% | 0,20 | +0,0399% | 1,65 |
| 0,7–0,9 | +0,0265% | 4,30 | +0,0305% | 2,12 |
| 0,9–1,0 | −0,0003% | −0,01 | −0,2066% (n=23) | −1,29 |

**Não há relação monotônica entre força e movimento previsto** — a faixa mais fraca
prevê tanto quanto a mais forte, e o teto de magnitude fica em ~0,02–0,03% em toda
coorte. A coorte "força ≥0,90 a 60s" que parecia promissora no snapshot v8 com seis
observações aparece agora em −0,21% com 23: miragem de amostra pequena, exatamente
como se temia ao não mexer nos limiares na época.

Ressalvas honestas: isto é agregado (o gate real é por símbolo+horizonte, e um
símbolo isolado ainda poderia destoar); as coortes foram fatiadas depois de ver o
dado, então servem como **geração de hipótese, não validação** — o caminho correto
continua sendo o item 2 acima (pré-registrar e testar em período novo e intocado).

Implicação para o item 5: a evidência inicial aponta para um gap **estrutural** de
magnitude (sinal ~6–10× menor que o custo), não para um problema de ajuste fino de
limiar ou de escolha de horizonte. Trocar taker por maker reduz o custo, mas não
fecha uma lacuna dessa ordem sozinha.

### Continuação (Claude Code) — revisão de correção em 7 ângulos + correções aplicadas

Com o Codex parado, esta sessão rodou uma revisão multi-ângulo (linha-por-linha, comportamento removido, rastreamento cross-file, reuso, simplificação, eficiência, altitude) sobre o núcleo de execução (`main.rs`, `orchestrator.rs`, `risk.rs`, `types.rs`, `bybit_demo.rs`, `events.rs`, `sources/order_flow.rs`, `sources/funding_carry.rs`, `exchange_filters.rs`, `config/risk.toml`), verificou cada achado manualmente (lendo o código atual e, num caso, a API pública real da Bybit) e corrigiu os confirmados. `orchestrator.exe` (PID ao vivo) nunca foi tocado — os efeitos só valem no próximo restart.

**Correções aplicadas (todas com teste novo, 41/41 passando, fmt/clippy limpos):**

1. **`bybit_demo.rs` — entrada sem fill no `execution/list` podia abandonar posição real.** Se `poll_fills` não via o fill de entrada dentro da janela de 3s mas a ordem tinha sido preenchida de verdade na venue, o código declarava `Unfilled` e liberava a reserva de risco — deixando uma posição real, sem stop, sem nenhum acompanhamento, só descoberta no próximo boot. Agora confere `position_size` antes de declarar unfilled; se a posição existir (ou a checagem falhar), vira `OpenRisk` em vez de ser abandonada.
2. **`bybit_demo.rs` — posição já flat na venue virava "risco aberto" permanente.** Se `/v5/execution/list` ainda não tinha propagado os fills de saída no instante da checagem (posição já zerada de verdade), o código dava `bail!`, caindo no handler genérico que manda `OpenRisk` com notional=0. Como o `OpenRisk` nunca é removido de `open_positions` em `orchestrator.rs` (só reagenda o próprio prazo pra sempre, sem nunca reconsultar a venue), isso travava `effective_halt` — e portanto TODA nova ordem — permanentemente, até reinício manual, além de vazar a reserva de risco daquele trade pra sempre. Agora tenta de novo (mesmo orçamento do poll de entrada) antes de desistir; se ainda assim não reconciliar, reporta com o notional real de entrada em vez de zero.
3. **Dois vazamentos de memória sem poda** (mesmo padrão que a auditoria original de 05/09 já tinha criticado): `reported_rejections` (`orchestrator.rs`) crescia um `u64` por sinal rejeitado pro resto da vida do processo; `ForwardTracker.records` (`funding_carry.rs`) crescia ~10 registros a cada 5 min pra sempre, em memória E no WAL em disco. Ambos agora podados — o primeiro junto com `pending`, o segundo por idade (30 dias) depois de liquidado, reescrevendo o WAL.
4. **`bybit_demo.rs::persist_api_health` gravava em disco em toda chamada autenticada** (código desta própria sessão, achado pela revisão): um único round-trip de trade faz 20-30+ chamadas privadas, cada uma disparando um write síncrono. Agora só grava na primeira observação ou perto do teto de rate limit.
5. **Comentários desatualizados em `risk.toml`/`risk.rs`**: `drawdown_halve_threshold_pct` e `leverage.launch_max` ainda descreviam mecanismos de redução de perna/trava de alavancagem que foram removidos numa correção anterior (auditoria externa de 12/08) e hoje só alimentam valores de exibição no dashboard — a proteção real é o `drawdown_ceiling_multiplier` (teto não-destrutivo, tiers fixos no código). O comentário em `risk.rs` também citava `RejectReason::NoLeverageOnLaunch`, que não existe mais. Comentários corrigidos; nenhum comportamento mudou.
6. **`exchange_filters.rs` — fallback silencioso pra 0.0** se `minNotionalValue`/`minOrderQty`/`qtyStep`/`tickSize` vier ausente da Bybit. Confirmado contra a API real: presente em 855/855 símbolos lineares ativos hoje, então não é bug ativo — mas ficaria silencioso se a Bybit mudar o schema, desabilitando a trava de "falha fechada sem filtro" pra todos os símbolos de uma vez. Agora loga aviso quando cai no fallback.

**Achados verificados mas não corrigidos nesta passada** (documentados, baixo risco/latentes, não exploráveis hoje):
- `wilson_lower_bound`/LCB95 (limite inferior de confiança) implementados de forma independente em `risk.rs`, `order_flow.rs` e `funding_carry.rs` — mesma fórmula, três cópias; uma correção futura ao método precisa lembrar de aplicar nas três.
- A regra "quem pode executar" (`strategy == OrderFlow`) está codificada separadamente em `risk::evaluate`, `pick_best` e no loop de rejeição de `orchestrator.rs` — hoje as três concordam, mas nada garante que continuem concordando se uma segunda estratégia ganhar executor.
- O loop multi-venue em `risk::evaluate` (herança da era multi-exchange) sobrescreveria silenciosamente `order_qty`/`price_tick` se `strategy_venues` um dia retornasse 2+ venues pra uma estratégia executável — hoje sempre retorna 0 ou 1, então não é bug ativo.
- `settle_execution` trata `ExecutionStatus` por `if` sequencial em vez de `match` exaustivo — uma variante nova de status cairia no caminho de PnL real sem ninguém perceber.
- Preflight do Demo valida saldo contra `total_equity_start` (constante fixa de US$200), não contra o equity real persistido — impacto prático baixo porque o saldo fictício da conta Demo tende a ser bem maior que o necessário.

### Continuação (Claude Code) — observabilidade de rate limit na API privada

Depois desta leva, o Codex ficou ~20 min sem alterar arquivos (`orchestrator.exe` já rodando ao vivo em `bybit_demo`, PID observado 11192). Nada foi interrompido; a sessão do Claude Code verificou tudo de forma independente (39 testes já existentes + os do funding carry, todos passando, Clippy limpo, sem segredos, `.env` fora do Git) e então implementou o item 17.4 da lista de revisão futura: observabilidade de rate limit/rejeição na API privada da Bybit Demo.

- `orchestrator/src/bybit_demo.rs` ganhou `ApiHealthCounters`: lê os headers `X-Bapi-Limit`, `X-Bapi-Limit-Status` e `X-Bapi-Limit-Reset-Timestamp` (nomes confirmados na documentação oficial, não por suposição) em toda chamada privada, avisa no log quando o restante cai a 20% ou menos do teto, e classifica rejeições por `retCode` — `10006` ("Too many visits!") conta como rate limit; HTTP 403 conta separadamente como bloqueio por IP.
- Persiste um resumo em `orchestrator/data/bybit_demo_api_health_v1.json` (schema `aurumos.bybit_demo.api_health.v1`), no mesmo padrão dos outros relatórios do projeto.
- É estritamente aditivo: os pontos de decisão de sucesso/falha continuam sendo só `ensure_success`/`ensure_success_or`, inalterados; a nova lógica só observa e loga, nunca decide ordem, posição, risco ou PnL. Cinco testes novos cobrem parsing de header ausente, mínimo histórico, classificação de rejeição e o limiar de 20%.
- `cargo fmt`, `cargo test --locked` (39 aprovados) e `cargo clippy --all-targets -D warnings` confirmados depois da mudança.
- Como é só código-fonte, o processo `orchestrator.exe` já em execução não foi tocado nem reiniciado — o efeito só existe a partir do próximo boot.
- Ainda não commitado. Segue pendente de aprovação do usuário para commit/push, junto com o restante das mudanças desta branch.

**Atualizado em:** 6 de setembro de 2026  
**Workspace:** `C:\Users\Renan\Documents\GitHub\AurumOS`  
**Branch de trabalho:** `codex/realistic-execution-core`  
**Commit-base observado:** `01913ef`  
**Estado do trabalho:** alterações locais ainda não commitadas  
**Banca modelada:** US$ 200 em uma única corretora  
**Corretora:** Bybit  
**Mercado executor:** perpétuos lineares liquidados em USDT  
**Backend padrão:** shadow  
**Backend autenticado disponível:** Bybit Demo  
**Dinheiro real:** estruturalmente desabilitado

## 1. Propósito deste documento

Este arquivo permite retomar o trabalho sem depender do histórico da conversa. Ele registra:

1. o objetivo pedido pelo usuário;
2. o que o repositório fazia antes da auditoria;
3. por que os resultados anteriores eram incorretos;
4. as decisões de arquitetura tomadas;
5. tudo que já foi modificado;
6. o estado atual do executor shadow e Bybit Demo;
7. os testes já executados;
8. o que ainda precisa ser validado;
9. a ordem recomendada para continuar;
10. os limites práticos e financeiros que não podem ser confundidos com defeitos de software.

Este documento deve ser atualizado sempre que uma etapa de execução, validação ou promoção de capital mudar.

## 2. Objetivo original e interpretação operacional

O pedido é construir um sistema completo de mercado financeiro que comece com US$ 200 depositados em uma única corretora e procure fazer o capital crescer por operações rápidas, potencialmente em segundos ou minutos, repetindo e aumentando o tamanho conforme o patrimônio cresce.

O usuário não definiu uma perda máxima nem um teto de lucro. A implementação não interpreta essa ausência como autorização para risco ilimitado. Sem limite de perda, uma estratégia de alta frequência pode transformar qualquer pequena expectativa negativa em ruína rápida. Por isso o sistema adota limites explícitos no código e em `orchestrator/config/risk.toml`.

O objetivo técnico adotado é:

- pesquisar um universo amplo de instrumentos;
- concentrar a execução em uma única venue e uma única carteira;
- operar somente sinais cuja vantagem líquida sobreviva a custos e validação temporal;
- usar ciclos curtos quando o mercado produzir evidência suficiente;
- compor resultados positivos sem teto nominal predefinido;
- reduzir ou bloquear a operação quando a evidência ou a execução se degradar;
- nunca apresentar retorno simulado ou reamostrado como dinheiro ganho;
- separar shadow, Demo e qualquer possível capital real em estados e arquivos diferentes.

Não existe garantia legítima de transformar US$ 200 em mais dinheiro em minutos. A velocidade do software aumenta a quantidade de decisões; ela não cria vantagem estatística. Com 0,055% de taxa taker por lado, um ciclo completo tem custo-base aproximado de 0,11% do notional antes de slippage. Uma estratégia que não supera esse custo perde mais depressa quando o loop é acelerado.

## 3. Restrições decididas

### 3.1 Uma única corretora

O executor usa apenas a Bybit. A antiga arbitragem Bybit–Bitget foi removida porque arbitragem entre corretoras exige saldo, inventário e reconciliação em duas venues. Isso violava a restrição explícita de manter os US$ 200 em uma única corretora.

### 3.2 Um único mercado executor

O mercado escolhido é `category=linear`, perpétuos USDT da Bybit. O scanner não mistura book spot com ranking de perpétuos.

### 3.3 Um único módulo autorizado a gerar PnL

Somente `Strategy::OrderFlow`, implementada como microestrutura direcional multi-horizonte de 5, 15, 30 e 60 segundos, pode ser marcada como executável. Cada símbolo+horizonte mantém amostra e gate próprios. News, Launch Radar, Pump Exhaustion, Whale Watch, Macro e Liquidation Hunter são sensores `ObservationOnly` e não alteram equity, wins, losses, Kelly ou escalonamento.

### 3.4 Sem endpoint de produção

O adaptador autenticado contém uma constante fixa:

```text
https://api-demo.bybit.com
```

Não existe configuração que troque esse hostname pelo domínio de produção. Os únicos valores aceitos por `AURUMOS_EXECUTION_MODE` são `shadow`, `demo` e o alias `bybit_demo`. Valores como `mainnet` falham no boot.

### 3.5 Estado independente por backend

Shadow e Demo nunca compartilham contabilidade:

- shadow: `events_shadow_v2.jsonl` e `portfolio_state_shadow_v2.json`;
- Demo: `events_demo_v1.jsonl` e `portfolio_state_demo_v1.json`.

Os arquivos vivem em `orchestrator/data/` e não são versionados.

## 4. Diagnóstico da versão anterior

### 4.1 PnL fabricado por reamostragem

O defeito central era usar retornos históricos sorteados para decidir o resultado de uma oportunidade nova. O orquestrador aprovava uma posição e depois escolhia um retorno antigo/probabilístico que não correspondia àquela ordem, àquele preço ou àquele instante.

Consequências:

- uma pequena amostra histórica podia produzir milhares de trades artificiais;
- os mesmos retornos eram reutilizados repetidamente;
- PnL fictício alimentava wins, losses, profit factor, Kelly e escalonamento;
- o dashboard apresentava crescimento que não vinha de fills;
- aumentar a frequência multiplicava o erro contábil.

### 4.2 Captura de spread sem fill demonstrável

O Order Flow anterior tratava preço parado e spread como lucro capturável. Isso supunha execução maker ou fill favorável sem demonstrar posição na fila, ack, fill, latência ou risco de seleção adversa.

### 4.3 Arbitragem incompatível com US$ 200 em uma venue

A arbitragem Bybit–Bitget precisava de capital nas duas corretoras e duas pernas sincronizadas. Ela não cabia na restrição financeira nem no modelo operacional definido pelo usuário.

### 4.4 Custos incompletos

Os cálculos não garantiam cobrança consistente de taxa taker nos dois lados. Um pequeno movimento bruto podia ser classificado como vantagem mesmo sem pagar entrada, saída e slippage.

### 4.5 Mistura de mercados e históricos

O sistema combinava universo linear, book spot e rankings persistidos de origens diferentes. Também carregava equity e edge antigos produzidos pelo simulador inválido.

### 4.6 Risco e exposição incorretos

Foram encontrados problemas como:

- risco de duas pernas contado uma vez;
- exposição aberta e fechada no mesmo instante, impedindo limites simultâneos reais;
- perdas realizadas sem relação estrita com o risco reservado;
- filtros de lote/notional falhando abertos quando ausentes;
- rejeições repetidas inflando contador;
- reset de drawdown que podia relatar novamente a mesma trava;
- concorrência artificial por tabela fixa em vez do orçamento de risco.

### 4.7 Defeitos adicionais nos sensores

Também foram corrigidos:

- confirmação de Pump Exhaustion destruída antes de maturar;
- funding negativo reforçando incorretamente um short;
- hash incompleto do evento Ethereum `PairCreated`;
- depósitos de whales vencidos permanecendo ativos;
- exploração de símbolos eliminada pelo truncamento do ranking;
- dados observacionais influenciando confluência executável sem dois executores reais;
- tópicos e assinaturas incompatíveis no caminho removido de múltiplas venues.

## 5. Arquitetura atual

```text
Bybit public linear WebSocket
  ├─ orderbook.1
  └─ publicTrade
          │
          ▼
Scanner de microestrutura por símbolo
  ├─ book recente
  ├─ profundidade mínima
  ├─ desequilíbrio do topo
  ├─ agressão negociada alinhada
  └─ uma janela pendente por símbolo
          │
          ▼
Histórico v2 por símbolo
  ├─ somente saídas executáveis
  ├─ retorno bruto
  ├─ taxa de ida e volta
  ├─ retorno líquido
  ├─ timestamp
  ├─ preços de entrada/saída
  └─ capacidade observada
          │
          ▼
Gate temporal de pesquisa
  ├─ mínimo 200 amostras
  ├─ primeira metade = treino
  ├─ segunda metade = validação
  ├─ LCB95 nas duas metades
  └─ validação novamente sem o melhor trade
          │
          ▼
Risk engine
  ├─ banca local US$ 200
  ├─ qtyStep/minQty/minNotional atuais
  ├─ alavancagem do executor 1x
  ├─ risco total simultâneo
  ├─ risco por estratégia/grupo
  ├─ uma posição por símbolo
  └─ kill-switch de drawdown/manual
          │
          ├───────────────┐
          ▼               ▼
Shadow quote          Bybit Demo REST
bid/ask após horizonte market entry → fills → 5/15/30/60s → reduceOnly close
          │               │
          └──────┬────────┘
                 ▼
ExecutionReport com o mesmo signal_id
  ├─ filled
  ├─ unfilled
  └─ open_risk
                 │
                 ▼
Contabilidade, persistência, dashboard e escalonamento
```

## 6. Estratégia executável de microestrutura

### 6.1 Entrada do candidato

Para cada símbolo, o scanner exige:

- melhor bid e ask válidos;
- book atualizado há no máximo 3,5 segundos;
- `ask > bid`;
- quantidade positiva nos dois lados;
- pelo menos três eventos de mudança do topo na janela de um segundo;
- OFI dinâmico normalizado absoluto de pelo menos 0,10;
- desequilíbrio absoluto dos negócios de pelo menos 0,55;
- notional negociado mínimo de US$ 2.000 na janela de um segundo;
- OFI de eventos e negócios apontando na mesma direção;
- pelo menos US$ 50 no topo usado para entrada;
- ausência de outra janela pendente na mesma combinação símbolo+horizonte;
- cooldown de um segundo por símbolo.

Long entra no ask e marca saída no bid. Short entra no bid e marca saída no ask.

### 6.2 Horizonte e custo

- janelas de holding pesquisadas em paralelo: 5, 15, 30 e 60 segundos;
- taxa taker padrão Major/Altcoin: 0,055% na entrada + 0,055% na saída;
- taxa taker especial por lado: 0,10% em Pre-listing e 0,11% na Innovation Zone;
- símbolo desconhecido usa 0,11% por lado; TradFi não entra no universo cripto;
- capacidade: menor notional entre o topo de entrada e o topo de saída;
- notional pretendido inicial: até US$ 25, sujeito ao risk engine.

### 6.3 Histórico v9

Arquivo:

```text
orchestrator/data/confirmation_history_microstructure_v9.json
```

Cada registro contém:

- `observed_at_ms`;
- `gross_return`;
- `net_return`;
- `round_trip_fee` e `direction_sign`;
- `signal_strength`, `book_event_imbalance` e `book_event_count`;
- `trade_imbalance` e `trade_notional_usd`;
- `queue_imbalance` e `spread_bps`;
- `entry_price`;
- `exit_price`;
- `capacity_usd`.

Uma saída sem book executável vira `unfilled` no shadow quando ligada a um sinal já aprovado, mas não vira amostra de retorno zero. Isso evita contaminar a distribuição com um número que não representa um round trip cotável.

### 6.4 Validação temporal

O símbolo precisa de no mínimo 200 amostras. A série permanece em ordem cronológica e é dividida ao meio:

- 100 ou mais amostras antigas para treino;
- 100 ou mais amostras recentes para validação.

O sistema calcula:

- limite inferior de 95% da média no treino;
- limite inferior de 95% da média na validação;
- limite inferior de 95% da validação depois de remover o melhor trade;
- limite inferior de Wilson da taxa de acerto nas duas metades;
- pior perda observada, com piso conservador de 0,5% para sizing.

O edge usado é o menor dos três limites inferiores da média. A execução só é liberada se esse valor exceder 0,01% líquido.

Relatório legível por máquina:

```text
orchestrator/data/strategy_validation_microstructure_v9.json
```

Esse relatório informa contagem, métricas e `approved` por símbolo+horizonte. As 20 coortes de força agregadas são diagnóstico de pesquisa e não alteram `approved`.

### 6.5 Pesquisa de entrada maker

O arquivo `orchestrator/data/strategy_validation_maker_entry_v3.json` mede tentativas, rejeições porque o preço mudou durante os 250 ms de colocação, fills conservadores, expiradas, taxa de fill e retorno maker+taker por símbolo+horizonte. Os eventos completos ficam em `raw_maker_entry_v3.jsonl` e as amostras em `confirmation_history_maker_entry_v3.json`. O relatório fixa `execution_enabled=false`; promover esse caminho exige amostra própria e implementação separada de ordem post-only, cancelamento, fill parcial e reconciliação.

## 7. Backend shadow

O shadow é o modo padrão e não requer chave:

```powershell
$env:AURUMOS_EXECUTION_MODE='shadow'
cargo run --locked -p orchestrator
```

Comportamento:

1. o sinal executável recebe `signal_id`;
2. o risk engine reserva exposição;
3. o Order Flow mede a saída no lado agressor do book após 5, 15, 30 e 60 segundos;
4. um `ExecutionReport` com o mesmo ID é enviado;
5. apenas `filled` válido altera PnL;
6. `unfilled`, relatório inválido ou timeout liberam a exposição sem criar trade.

O shadow continua sendo uma estimativa. Ele não prova posição na fila, fill, latência privada, rejeição da conta ou slippage além do topo.

## 8. Backend Bybit Demo

### 8.1 Ativação

Copiar `orchestrator/.env.example` para `orchestrator/.env` e preencher:

```dotenv
AURUMOS_EXECUTION_MODE=demo
BYBIT_DEMO_API_KEY=...
BYBIT_DEMO_API_SECRET=...
```

As chaves devem ser criadas numa conta Bybit Demo dedicada. A conta não deve conter posições manuais nem ordens de outro robô.

### 8.2 Preflight obrigatório

Antes de iniciar fontes ou dashboard, o cliente:

1. consulta o relógio do servidor;
2. calcula o offset do relógio local;
3. autentica usando HMAC SHA-256;
4. consulta saldo da conta UNIFIED;
5. exige pelo menos US$ 200 disponíveis;
6. consulta todas as posições lineares USDT;
7. recusa qualquer posição com quantidade positiva;
8. consulta ordens lineares abertas;
9. recusa qualquer ordem existente.

Sem chave, segredo, saldo ou conta limpa, o processo termina antes de enviar ordem.

### 8.3 Entrada

Para cada oportunidade aprovada:

- o símbolo enviado é o símbolo puro, por exemplo `BTCUSDT`, sem a anotação usada no dashboard;
- a quantidade vem do `order_qty` já arredondado pelo `qtyStep` da Bybit;
- o ID é determinístico e ligado ao sinal: `aur-{signal_id}-e`;
- a ordem é `Market`;
- `positionIdx=0` pressupõe modo one-way;
- tolerância máxima de slippage configurada: 0,50%;
- um erro de ack é tratado como estado incerto e reconciliado pelo mesmo `orderLinkId`.

Antes da entrada, o adaptador envia configuração específica do símbolo para modo one-way (`mode=0`) e alavancagem de compra/venda em 1x. Os códigos `110025` e `110043` são aceitos apenas nesses dois endpoints porque a documentação os define como “configuração não modificada”, isto é, o valor já estava aplicado. Outros códigos interrompem a operação antes da entrada.

Depois do fill, a média de entrada e o `tickSize` do instrumento definem um stop de posição inteira. O stop usa `MarkPrice`, ordem market e o percentual de perda usado pelo risk engine. Se `/v5/position/trading-stop` não confirmar a proteção, o executor não espera o horizonte selecionado: começa a zerar a posição imediatamente.

### 8.4 Fills e fechamento

O adaptador consulta `/v5/execution/list`, agrega execuções parciais e deduplica por `execId`. Depois do holding:

- fecha no lado oposto;
- usa `reduceOnly=true`;
- usa a quantidade efetivamente preenchida na entrada;
- permite até três tentativas de fechamento parcial;
- consulta `/v5/position/list` ao terminar;
- reconcilia também uma saída produzida pelo stop da venue;
- calcula PnL absoluto por `execValue` de entrada/saída e soma `execFee` dos dois lados;
- envia esse PnL absoluto ao orquestrador, evitando reconstrução aproximada a partir do notional cotado.

### 8.5 Estados de execução

`Filled`:

- posição reconciliada como zerada;
- fills de entrada e saída conhecidos;
- PnL contabilizado;
- exposição liberada.

`Unfilled`:

- nenhuma entrada confirmada;
- nenhum PnL criado;
- exposição liberada.

`OpenRisk`:

- existe fill possível, posição residual ou falha de reconciliação;
- a posição lógica permanece aberta;
- a exposição não é liberada;
- o dashboard mostra `RISCO ABERTO`;
- novas ordens são bloqueadas globalmente;
- o operador precisa reconciliar a conta Demo.

Timeout no modo Demo nunca é convertido automaticamente em `unfilled`, porque um timeout de rede pode ocorrer depois de a corretora aceitar a entrada.

### 8.6 Concorrência

O adaptador pode executar símbolos diferentes em paralelo. O orquestrador impede abrir duas operações simultâneas no mesmo símbolo, evitando que fechamentos `reduceOnly` de tarefas concorrentes interfiram numa posição one-way compartilhada.

## 9. Risk engine e banca de US$ 200

Configuração atual relevante:

| Regra | Valor |
|---|---:|
| Equity operacional inicial | US$ 200 |
| Perna inicial | US$ 25 |
| Máximo da perna | 30% da equity |
| Alavancagem do executor | 1x |
| Alavancagem agregada máxima | 1x |
| Risco simultâneo total | 0,50% da equity |
| Risco simultâneo Order Flow | 0,25% da equity |
| Aviso diário | 1,00% |
| Bloqueio diário | 1,50% |
| Bloqueio semanal | 4,00% |
| Bloqueio desde o pico | 7,00% |
| Lucro para reserva protegida | 20% |
| Lucro reinvestido no executor | 80% |

O saldo fictício da conta Demo pode ser muito maior que US$ 200. Isso não altera o limite local: `PortfolioState` começa em US$ 200 e o sizing usa esse estado.

Filtros de instrumento são buscados em `/v5/market/instruments-info`. Sem filtro do símbolo, o risco falha fechado. A quantidade é arredondada para baixo pelo `qtyStep` e verificada contra `minOrderQty` e `minNotionalValue`.

Escalonamento não é acionado apenas por saldo maior. Ele exige, entre outros critérios:

- intervalo mínimo de 50 ciclos desde o aumento anterior;
- profit factor recente mínimo para crescer;
- resultado positivo sem o melhor trade;
- drawdown compatível;
- Kelly fracionário;
- limite de 30% da equity por perna;
- orçamento de exposição disponível.

## 10. Persistência e isolamento de dados

Arquivos antigos continham métricas derivadas do simulador inválido. Para impedir contaminação silenciosa, foram criados nomes novos:

- `events_shadow_v2.jsonl`;
- `portfolio_state_shadow_v2.json`;
- `edge_scores_microstructure_v9.json`;
- `confirmation_history_microstructure_v9.json`;
- `strategy_validation_microstructure_v9.json`;
- `confirmation_history_maker_entry_v3.json` e `strategy_validation_maker_entry_v3.json`;
- `events_demo_v1.jsonl`;
- `portfolio_state_demo_v1.json`.

Se o arquivo de portfólio de um backend não existir, o histórico de eventos daquele backend é removido no boot. Isso evita exibir equity antiga ao lado de estado inicial novo.

## 11. Dashboard e observabilidade

O dashboard roda em:

```text
http://127.0.0.1:7878
```

Ele recebe um evento `execution_backend` e informa dinamicamente:

- shadow: nenhuma ordem enviada;
- Bybit Demo: ordens na conta de teste e ausência de endpoint de produção.

O feed apresenta o mesmo `signal_id` em oportunidade, decisão, execução e resultado. O evento de execução distingue:

- `filled`;
- `unfilled`;
- `open_risk`.

Heartbeats mostram que o scanner continua vivo mesmo quando nenhuma oportunidade passa o gate. O dashboard também expõe limites de risco, exposição por estratégia, drawdown, kill-switch, universo e ranking.

## 12. Módulos observacionais mantidos

Os módulos abaixo continuam úteis para pesquisa e contexto, mas não executam:

| Módulo | Dados | Estado |
|---|---|---|
| News Reactor | SEC EDGAR e classificação local quando disponível | `ObservationOnly` |
| Launch Radar | novos instrumentos Bybit e Uniswap V2 | `ObservationOnly` |
| Pump Exhaustion | funding, variação, OI e fusão | `ObservationOnly` |
| Whale Watch | transferências Ethereum e rótulos | `ObservationOnly` |
| Macro Engine | agenda FOMC/CPI/NFP | `ObservationOnly` |
| Liquidation Hunter | `allLiquidation` Bybit | `ObservationOnly` |

Eles não devem ser promovidos apenas porque detectam eventos. Cada um precisa de executor próprio, custo completo e validação fora da amostra ligada a fills.

## 13. Arquivos principais alterados

### Execução e tipos

- `orchestrator/src/bybit_demo.rs`: cliente autenticado restrito à Demo.
- `orchestrator/src/types.rs`: `ExecutionBackend`, `ExecutionStatus`, `ExecutionRequest` e `signal_id`.
- `orchestrator/src/main.rs`: seleção de backend, preflight, canais e persistência separada.
- `orchestrator/src/orchestrator.rs`: lifecycle por relatório, risco aberto e despacho Demo.

### Estratégia e risco

- `orchestrator/src/sources/order_flow.rs`: scanner Bybit linear, custo taker, histórico v2 e validação temporal.
- `orchestrator/src/risk.rs`: sizing, quantidade, filtros, reservas e correções de drawdown/exposição.
- `orchestrator/src/exchange_filters.rs`: filtros lineares atuais da Bybit.
- `orchestrator/src/symbol_universe.rs`: universo linear e vagas reais de exploração.
- `orchestrator/config/risk.toml`: parâmetros da banca e dos kill-switches.

### Observabilidade

- `orchestrator/src/events.rs`: eventos com `signal_id`, status e backend.
- `orchestrator/dashboard/index.html`: modo dinâmico e destaque de risco aberto.
- `orchestrator/src/dashboard.rs`: entrega do painel e eventos.
- `orchestrator/src/raw_log.rs`: logs brutos.

### Sensores corrigidos

- `orchestrator/src/sources/pump_exhaustion.rs`;
- `orchestrator/src/sources/whale_watch.rs`;
- `orchestrator/src/sources/dex_launch_radar.rs`;
- `orchestrator/src/fusion.rs`;
- demais fontes observacionais ajustadas ao contrato novo.

### Arquivos removidos

- `orchestrator/src/sources/arbitrage.rs`;
- `orchestrator/src/sources/multi_asset.rs`.

### Dependências adicionadas

- `hmac = "0.12"`;
- `sha2 = "0.10"`.

`Cargo.lock` foi atualizado para refletir essas dependências.

## 14. Testes implantados

A suíte cobre, entre outros pontos:

- HMAC SHA-256 contra vetor conhecido;
- aceitação restrita dos códigos oficiais “modo/alavancagem já configurados”;
- quantidade sem notação científica ou zeros finais;
- agregação e deduplicação de fills parciais;
- PnL calculado por valores e taxas da venue, com prioridade sobre retorno reconstruído;
- stop protetor arredondado pelo `tickSize`;
- reconciliação de fills de saída por lado, incluindo stop;
- parser de backend recusando `mainnet`;
- `signal_id` representável exatamente no JavaScript;
- PnL somente com relatório do ID correspondente;
- relatório inválido tratado como `unfilled`;
- `open_risk` preservando posição e exposição;
- símbolo puro enviado ao adaptador Demo;
- observação incapaz de reservar capital;
- falha fechada sem filtro Bybit;
- cancelamento de `unfilled` liberando reservas;
- contagem correta de risco de duas pernas;
- mínimo de 200 amostras por símbolo;
- cobrança de taxa nos dois lados;
- necessidade de margem sobre ruído;
- impossibilidade de um único outlier de validação criar edge;
- correções de Pump, Whale, DEX e rotação de universo.

Verificação final executada depois das mudanças:

```powershell
cargo fmt --all
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Resultado: **27 testes aprovados, 0 falhas; Clippy sem avisos**. O JavaScript embutido no dashboard passou em `node --check`.

## 15. Verificação ao vivo desta sessão

O smoke test shadow foi executado contra os endpoints públicos e confirmou:

1. dashboard em HTTP 200, com 422.087 bytes;
2. 855 filtros de perpétuos lineares carregados;
3. 450 candidatos líquidos recebidos para rotação;
4. universo ativo de 220 símbolos;
5. 440 canais públicos de `orderbook.1` e `publicTrade` conectados;
6. 22 lotes de tickers do Pump Exhaustion;
7. 22 lotes do Liquidation Hunter;
8. arquivos raw e histórico v2 recebendo dados;
9. relatório de validação com mínimo de 200 amostras;
10. equity mantida em US$ 200 e zero ciclos inventados durante a calibração.

O modo Demo também foi iniciado deliberadamente sem credenciais. Ele terminou antes das fontes e antes de qualquer ordem com `BYBIT_DEMO_API_KEY ausente para modo demo`, confirmando falha fechada.

O endpoint `https://api-demo.bybit.com/v5/market/time` respondeu `retCode=0` e `retMsg=OK`. O valor inválido `AURUMOS_EXECUTION_MODE=mainnet` terminou com código 1 e a mensagem `use shadow ou demo`. Não havia `orchestrator/.env` local durante esses testes.

A busca estática confirmou que `/v5/order/create` aparece somente em `bybit_demo.rs`, sob a constante fixa `https://api-demo.bybit.com`. Os domínios `api.bybit.com` restantes são exclusivamente endpoints públicos de instrumentos/tickers.

O único bloqueio externo para uma campanha autenticada é fornecer chaves de uma conta Bybit Demo dedicada. Sem essas credenciais, não é possível validar respostas privadas e fills reais da conta de teste.

### 15.1 Verificação independente (Claude Code, mesma sessão de trabalho)

Depois que o Codex ficou 5 minutos sem alterar arquivos, esta sessão assumiu a fila pendente e reproduziu cada etapa de forma independente, sem confiar apenas neste relatório:

- `cargo fmt --all -- --check`: sem diferenças.
- `cargo test --locked`: 27 aprovados, 0 falhas (mesma contagem).
- `cargo clippy --locked --all-targets -- -D warnings`: sem avisos.
- Busca estática por `arbitrage`, `Bitget`, `multi_asset`, `sampled_return`/`resampl`, `mainnet` e `alpaca` em `orchestrator/src` e `orchestrator/Cargo.toml`: nenhuma ocorrência funcional restante — apenas comentários históricos e o teste que rejeita `mainnet`. `sources/mod.rs` não referencia mais `arbitrage`/`multi_asset`.
- **Corrigido:** comentário desatualizado em `symbol_universe.rs` (função `run` da rotação de universo) ainda listava "Arbitragem" entre os assinantes do `watch::Sender`; o módulo não existe mais e nunca foi de fato assinante. Atualizado para citar só `Order Flow`, `Pump Exhaustion` e `Liquidation Hunter`, que são os três `use`rs reais do canal.
- JavaScript embutido em `dashboard/index.html` extraído e validado com `node --check`: sintaxe válida.
- Reprodução do fail-closed do modo Demo sem credenciais: mesma mensagem exata, `Error: BYBIT_DEMO_API_KEY ausente para modo demo`, saída antes de qualquer fonte iniciar.
- Reprodução do smoke test shadow (28s, endpoints públicos reais): 855 filtros de perpétuos, 450 candidatos, universo de 220, 440 canais `orderbook.1`/`publicTrade`, 22 lotes em Pump Exhaustion e 22 em Liquidation Hunter — todos idênticos aos números já registrados acima. Dashboard respondeu HTTP 200 com **422.087 bytes**, o mesmo tamanho exato relatado nesta sessão.
- Varredura do diff (`git diff`) e dos arquivos novos (`.env.example`, `bybit_demo.rs`, docs desta pasta) por padrões de chave/segredo: nenhum valor de credencial encontrado; as únicas ocorrências de `ALPACA_API_KEY_ID`/`ALPACA_API_SECRET_KEY` são linhas removidas (nomes de variável do módulo Multi-Asset excluído, não valores). `.gitignore` já cobre `orchestrator/.env` e `orchestrator/data/`.

Conclusão desta verificação: os critérios da seção 19 seguem válidos de forma independente. Nada foi revertido; a única mudança de código feita nesta passada foi o comentário citado acima.

## 16. O que falta depois desta sessão

Mesmo com o software pronto para Demo, lucratividade ainda precisa ser demonstrada. O trabalho operacional posterior é:

1. coletar pelo menos 200 amostras válidas por símbolo em horários e regimes diferentes;
2. não alterar limiares olhando o resultado da validação;
3. reservar um terceiro período como teste final intocado;
4. rodar uma campanha autenticada na Bybit Demo;
5. medir fill ratio, slippage mediano/p95, rejeições, latência e diferença shadow–Demo;
6. verificar estabilidade por dia, horário, volatilidade e símbolo;
7. medir drawdown e concentração;
8. exigir resultado líquido positivo depois de todas as taxas;
9. manter o melhor trade removido e testar sensibilidade a outliers;
10. observar falhas de rede/restart e validar reconciliação na prática;
11. só discutir capital real depois desses portões.

O projeto ainda não deve receber um cliente de produção enquanto esses dados não existirem. A implementação atual deliberadamente torna impossível ativar mainnet por variável de ambiente.

## 17. Pontos que merecem revisão futura

### 17.1 Stream privado

O executor Demo atual reconcilia fills por REST. Uma evolução operacional pode adicionar o WebSocket privado de execution/order para reduzir latência de observação, mantendo REST como fonte de reconciliação após reconnect. O REST já impede criar PnL sem fills.

### 17.2 Persistência de posições lógicas em aberto

Exposição e a lista `OpenPosition` são estados transitórios e não são reidratados após restart. O preflight recusa iniciar o modo Demo se houver posição real aberta, evitando que o processo trate uma posição da venue como inexistente. Uma evolução deve persistir o journal completo de ordens/posições lógicas para reconciliação automática após restart, sem relaxar esse bloqueio.

### 17.3 Modo de posição e alavancagem da conta

As ordens usam `positionIdx=0`. O executor força one-way e 1x por símbolo antes da entrada. A campanha ainda deve usar uma conta Demo dedicada para impedir que configurações ou operações manuais concorram com o robô.

### 17.4 Limites de taxa e concorrência da API privada

O risk engine possui controle local, mas uma campanha prolongada deve registrar headers de rate limit, backoff e códigos de rejeição privados. Símbolos iguais já são serializados pelo orquestrador.

### 17.5 Teste final imutável

O corte treino/validação já está no runtime. Ainda falta congelar limiares e avaliar num terceiro intervalo que não participe de seleção de símbolo nem tuning.

## 18. Comandos de retomada

Na raiz do repositório:

```powershell
git status --short
git branch --show-current
cd orchestrator
cargo fmt --all
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Boot shadow:

```powershell
$env:AURUMOS_EXECUTION_MODE='shadow'
cargo run --locked
```

Prova de falha fechada no Demo sem credenciais:

```powershell
$env:AURUMOS_EXECUTION_MODE='demo'
Remove-Item Env:BYBIT_DEMO_API_KEY -ErrorAction SilentlyContinue
Remove-Item Env:BYBIT_DEMO_API_SECRET -ErrorAction SilentlyContinue
cargo run --locked
```

O segundo comando deve terminar com mensagem de chave ausente. Ele não deve iniciar fontes nem enviar ordens.

Campanha Demo autenticada, somente quando as credenciais existirem:

```powershell
Copy-Item orchestrator/.env.example orchestrator/.env
# editar orchestrator/.env com AURUMOS_EXECUTION_MODE=demo e chaves Demo
cargo run --locked -p orchestrator
```

## 19. Critério de pronto para a implementação atual

A implementação desta etapa está pronta quando:

- testes e Clippy passam;
- dashboard tem JavaScript válido;
- shadow conecta a dados públicos reais;
- Demo sem credenciais falha fechado;
- não existe hostname de produção no código;
- não existe gerador aleatório de PnL;
- histórico v1 inválido não entra no gate v2;
- shadow e Demo usam arquivos de estado diferentes;
- `open_risk` bloqueia novas ordens sem liberar exposição;
- documentação descreve fielmente o código;
- diff final não contém segredos.

Isso significa “executor Demo implementado e seguro para campanha de teste”. Não significa “estratégia comprovadamente lucrativa” nem “pronto para dinheiro real”.

## 20. Referências oficiais usadas

- Bybit Demo: <https://bybit-exchange.github.io/docs/v5/demo>
- Autenticação V5: <https://bybit-exchange.github.io/docs/v5/guide>
- Criar ordem: <https://bybit-exchange.github.io/docs/v5/order/create-order>
- Modo da posição: <https://bybit-exchange.github.io/docs/v5/position/position-mode>
- Alavancagem: <https://bybit-exchange.github.io/docs/v5/position/leverage>
- Histórico de execuções: <https://bybit-exchange.github.io/docs/v5/order/execution>
- Posições: <https://bybit-exchange.github.io/docs/v5/position>
- Saldo da carteira: <https://bybit-exchange.github.io/docs/v5/account/wallet-balance>
- Instrumentos e filtros: <https://bybit-exchange.github.io/docs/v5/market/instrument>
- Orderbook público: <https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook>
- Negócios públicos: <https://bybit-exchange.github.io/docs/v5/websocket/public/trade>
- Estrutura de taxas: <https://www.bybit.com/en/help-center/article/Trading-Fee-Structure>
- Grupos públicos de contratos: <https://bybit-exchange.github.io/docs/v5/market/fee-group-info>

## 21. Regra de continuidade

Ao retomar, não reintroduzir:

- PnL por sorteio;
- retorno histórico aplicado a uma oportunidade nova;
- arbitragem entre corretoras com os mesmos US$ 200;
- execução de sensores sem fills vinculados;
- endpoint de produção configurável;
- promoção com menos de 200 amostras válidas;
- seleção e avaliação no mesmo período sem corte temporal;
- liberação de exposição quando a posição Demo é incerta;
- mistura de estado shadow, Demo e real;
- escalonamento baseado apenas em velocidade ou desejo de lucro.

Toda nova estratégia precisa apresentar o caminho completo `dados → sinal → ordem → fill → PnL → risco`, com o mesmo identificador e custos reais, antes de poder alterar a banca.
