# Auditoria técnica e financeira do AurumOS

**Revisão auditada:** `01913ef`  
**Data:** 05/09/2026  
**Escopo:** arquitetura, fontes de dados, geração e seleção de sinais, execução simulada, risco, persistência, backtests, dashboard e adequação ao objetivo de transformar US$ 200 em capital crescente por operações pequenas e rápidas.

> Este documento preserva o diagnóstico da revisão original. As correções
> aplicadas depois da auditoria estão em
> [`REMEDIACAO_UMA_CORRETORA.md`](REMEDIACAO_UMA_CORRETORA.md).

## Veredito

O AurumOS compila e várias fontes realmente recebem dados públicos de mercado. Entretanto, ele ainda não demonstra uma estratégia lucrativa e não pode cumprir o objetivo financeiro descrito no estado atual.

O motivo principal é estrutural: a operação que aparece no painel não é resolvida contra o que aconteceu com aquela operação. Arbitragem, Order Flow e Pump Exhaustion sorteiam um retorno retirado de um histórico agregado de observações anteriores. Poucas observações podem ser reutilizadas milhares de vezes, e cada reutilização é contabilizada como um novo trade independente, alimentando o PnL, o profit factor e o Kelly. Portanto, os resultados citados no README não medem lucro executável.

Além disso, seis dos nove módulos nunca operam, o Pump Exhaustion não consegue completar sua janela de confirmação, o radar DEX usa um tópico Ethereum inválido e a assinatura inicial da Bitget já excede o tamanho máximo documentado. O produto é hoje um coletor experimental de dados com um simulador otimista, não um orquestrador validado para crescimento de capital.

## O que o projeto realmente faz

1. Cria dois universos baseados somente na Bybit: spot e perpétuos lineares.
2. Executa duas estratégias simuladas de spot: Arbitragem Bybit–Bitget e Order Flow na Bybit.
3. Tenta habilitar uma terceira estratégia simulada, Pump Exhaustion, após acumular confirmações.
4. Usa Whale Watch e Liquidation Hunter apenas como contexto para Pump Exhaustion.
5. Mantém News, Macro, Launch e Multi-Asset como sensores informativos com `net_edge = 0`.
6. Aprova posições por limites de risco calculados a partir de perdas máximas fixas e encerra essas posições com retornos reamostrados do passado.

Não existem envio de ordem, conta demo autenticada, livro de saldos por exchange, fila maker, reconciliação de fills, cancelamento, mark-to-market ou PnL ligado a IDs de ordens.

## Falhas críticas confirmadas

| Severidade | Falha | Evidência | Consequência |
|---|---|---|---|
| Crítica | PnL não pertence ao trade aberto | `orchestrator.rs:401-410`; amostras escolhidas em `arbitrage.rs:621-624`, `order_flow.rs:383-385` e `pump_exhaustion.rs:493-496` | O painel pode enriquecer reutilizando desfechos antigos; win rate, PF e Kelly ficam inválidos. |
| Crítica | Order Flow presume captura do spread sem testar fills | `order_flow.rs:237-241` calcula `entry_edge - abs(movimento do mid)`; não há ordem, trade print, fila ou duas pernas | Preço parado vira “ganho” mesmo que nenhuma ordem tivesse sido executada. Seleção adversa e fill de apenas um lado não existem no modelo. |
| Crítica | Arbitragem não mantém saldo nem estoque por exchange | `risk.rs:740` contabiliza uma vez `order_size`, apesar de a estratégia usar duas pernas | Sete arbitragens de US$ 25 são contabilizadas como US$ 175, embora exijam US$ 350 somando as duas pernas e estoque adequado em cada venue. |
| Crítica | Risco reservado não limita a perda realizada | `risk.rs:813` reserva `order_size × max_loss_pct`; `orchestrator.rs:402` aceita qualquer `sampled_return` histórico | O probe aceitou perda de US$ 1,25 após reservar apenas US$ 0,075. Não existe stop que imponha `max_loss_pct`. |
| Crítica | Pump Exhaustion perde toda confirmação antes de maturar | rotação em `symbol_universe.rs:33`; janela de 20 min em `pump_exhaustion.rs:103`; pendências locais em `pump_exhaustion.rs:232`; reconexão em `pump_exhaustion.rs:191` | A rotação ocorre a cada 15 min e destrói pendências que precisam de 20 min. A estratégia tende a ficar eternamente com zero trades. |
| Crítica | Radar DEX assina um tópico Ethereum inválido | `dex_launch_radar.rs:25` tem 63 dígitos hex; `web3_sha3` retorna hash de 64 dígitos terminado em `d0e9` | O RPC responde `invalid argument ... hex string of odd length`; nenhum `PairCreated` é recebido. |
| Alta | Assinatura Bitget excede o limite antes mesmo da expansão | `arbitrage.rs:345-350` envia todos os símbolos numa mensagem | A semente de 72 pares produz 4.240 bytes, acima dos 4.096 permitidos. O alvo de 220 pares é muito maior; o feed pode rejeitar ou operar incompleto. |
| Alta | Reinício apaga posições abertas e evidência recente | `risk.rs:383-423` reidrata estado com exposição, notional, PnL recente e histórico por símbolo vazios | Reiniciar pode apagar perdas ainda abertas, liberar risco que continua existindo numa exchange e alterar Kelly/PF. |
| Alta | Reset do halt total religa o próprio halt imediatamente | `risk.rs:569-586` limpa a trava e logo recalcula o mesmo drawdown | A confirmação manual é consumida, mas o sistema volta a travar no mesmo tick. Como o halt bloqueia novas operações, não há mecanismo interno de recuperação. |
| Alta | Reiniciar durante aviso de drawdown reduz as pernas novamente | `orchestrator.rs:122` inicia `was_warned=false`; `orchestrator.rs:181-198` reduz todas as pernas | Cada restart acima de 1% de drawdown aplica outra divisão por dois. Ao cair abaixo do mínimo da exchange, nenhum trade pode completar os ciclos necessários para recuperar o tamanho. |
| Alta | Whale signal vencido nunca expira se não entrar novo depósito | limpeza só ocorre em `fusion.rs:66-70`; leitura em `fusion.rs:74` soma tudo | Um depósito de uma hora continua reforçando uma janela declarada como 30 minutos. O probe reproduziu US$ 3 milhões vencidos ainda ativos. |
| Alta | Um único ranking mistura métricas incompatíveis | Arbitragem e Order Flow recebem o mesmo `edge_scores_spot` em `main.rs:197` e `main.rs:230` | Spread interno maker e spread cross-exchange são misturados na mesma média, contaminando a seleção de símbolos das duas estratégias. |
| Alta | As 50 vagas de exploração desaparecem quando há muitos “comprovados” | `symbol_universe.rs:276-293` adiciona exploração no fim e trunca para 220 | Com 220 comprovados, ficam zero exploratórios; com 200, ficam apenas 20. O sistema pode se prender ao universo antigo. |
| Alta | Backtest do Pump não corresponde ao código atual | script diz “EXATAMENTE”, mas usa 0,10% funding, +15% em 24h e horizonte de 24h; o Rust usa 0,015%, +3%, score com OI/fusão e 20 min | Não existe backtest válido da estratégia que está hoje no binário. |
| Alta | Não há testes do produto | `cargo test --locked` executou zero testes | Regressões nas regras de risco e nos feeds chegam ao runtime sem detecção. Os sete probes desta auditoria reproduzem defeitos; eles não são testes de correção. |

## Outras falhas materiais

- A arbitragem calcula a capacidade usando somente a quantidade do lado comprado (`arbitrage.rs:551` e `:554`). A quantidade disponível no bid da perna vendida é ignorada.
- Livros de até 30 segundos são aceitos para uma estratégia cuja janela é de 2 segundos (`arbitrage.rs:56`). Não há limite de diferença entre timestamps das exchanges nem uso de sequence IDs.
- A confirmação de arbitragem não revalida frescor do livro antes de salvar o resultado (`arbitrage.rs:465-480`).
- O histórico empírico é agregado entre símbolos, direções e regimes. Um resultado de um token ilíquido pode ser sorteado como desfecho de outro ativo.
- Os limiares de 30/50 amostras contam eventos correlacionados como evidência. O Kelly trata cada reamostragem como um novo trade independente.
- O score divide pela necessidade de capital (`types.rs:141`) mesmo quando o motor reduz o tamanho efetivo. Isso favorece oportunidades pequenas sem maximizar o lucro em dólares do portfólio.
- `pick_best` descarta candidatos reprovados pelo risco sem aumentar `portfolio.rejections` ou emitir o evento correspondente (`orchestrator.rs:300-309`). O dashboard subconta as rejeições.
- O funding usa valor absoluto (`pump_exhaustion.rs:332`). Funding extremamente negativo, normalmente associado a shorts congestionados, reforça uma operação short de “exaustão de alta”, sinal economicamente invertido em parte dos casos.
- Os parâmetros de fill parcial de 35% e falha da segunda perna de 3% são constantes arbitrárias (`orchestrator.rs:31-37`). Eles não são medidos e não dependem de ativo, liquidez, latência ou horário.
- O estado de edge persistido não tem timestamps. Dados antigos podem continuar “comprovados” depois de o regime desaparecer.
- O universo spot nasce apenas da lista da Bybit e não faz a interseção com símbolos negociáveis na Bitget antes de assinar o feed.
- O radar DEX não faz backfill após desconexões e sua concentração de holders reconstrói somente transfers posteriores à criação do par, o que não representa a distribuição completa de tokens pré-existentes.
- O News Reactor classifica somente o título do 8-K, não mapeia o filing ao ticker e usa um contato placeholder no User-Agent exigido pela SEC.
- Macro usa datas manuais e presume NFP sempre na primeira sexta-feira, sem feriados ou adiamentos.
- Multi-Asset observa somente seis ações/ETFs e nunca opera. A variante `Forex` nem é construída, como o compilador avisou.
- O arquivo de eventos cresce sem compactação. O limite de 50.000 vale apenas para memória, não para disco.

## O README contém afirmações incompatíveis com o código

- “Só 3 executam trade de verdade”: as três apenas executam dentro do simulador local.
- “Todo o estado de risco (posições ... histórico de confirmação) é persistido”: posições abertas são descartadas, PnL recente e Kelly por símbolo são apagados; Pump Exhaustion nem persiste sua confirmação.
- “Posições resolvem no máximo em 30s”: Pump Exhaustion declara 20 minutos.
- “Arbitragem 87,9% / +US$1.002,79 e Order Flow 90,9% / +US$2.268,81”: esta cópia não contém `orchestrator/data`, e os resultados foram produzidos pelo mecanismo de reamostragem, não por fills correspondentes.
- O dashboard marca cada fonte como “DADO REAL”, enquanto o rodapé chama o resultado de sintético. O dado de entrada pode ser real; execução, fills e PnL não são.

## Por que US$ 200 não crescem rapidamente por este desenho

Com a configuração atual:

| Item | Valor com equity de US$ 200 |
|---|---:|
| Risco simultâneo total declarado | US$ 1,00 |
| Halt diário declarado | US$ 3,00 |
| Halt semanal declarado | US$ 8,00 |
| Halt total declarado | US$ 14,00 |
| Perna inicial | US$ 25,00 |
| Ganho mínimo teórico por captura de Arbitragem a 0,65% | US$ 0,1625 |
| Ganho mínimo teórico por captura de Order Flow a 0,45% | US$ 0,1125 |

Para acrescentar US$ 200 sem aumentar a perna seriam necessárias aproximadamente 1.231 capturas perfeitas de arbitragem ou 1.778 capturas perfeitas de Order Flow. Isso ignora fills ausentes, falhas de uma perna, inventário, rebalanceamento, slippage, impostos e mudanças de regime.

O obstáculo não é a velocidade do loop. Para Order Flow, o spread bruto precisa superar aproximadamente 0,65% depois de duas taxas maker de 0,10%; para arbitragem, precisa superar aproximadamente 0,95% depois das taxas fixas usadas pelo código. Spreads assim tendem a aparecer em mercados ilíquidos, atrasados ou arriscados, exatamente onde fila, slippage e seleção adversa são maiores.

A Bybit publica 0,10% maker e taker para spot não-VIP, com variação por conta/região: https://www.bybit.com/en/help-center/article/Bybit-Spot-Fees-Explained. A documentação da Bitget limita uma mensagem de assinatura a 4.096 bytes e recomenda menos de 50 canais por conexão: https://www.bitget.com/api-doc/classic/quickStart/websocket-intro. A documentação da Bybit confirma que o Level 1 é snapshot e fornece timestamps/sequence IDs que este projeto não usa para sincronizar venues: https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook.

Não definir um teto de lucro é compatível com composição contínua. Não definir uma perda máxima não é: sem um drawdown tolerável e uma probabilidade de ruína aceitável, não existe função objetiva para escolher sizing. A própria configuração já tenta usar US$ 3/US$ 8/US$ 14 como limites, mas os bugs acima impedem que esses números sejam garantias. A SEC descreve day trading como extremamente arriscado e capaz de gerar perdas substanciais em pouco tempo: https://www.investor.gov/introduction-investing/investing-basics/glossary/day-trading.

## Plano de recuperação recomendado

### 1. Tornar o resultado mensurável

Suspender os números de PnL atuais como evidência. Cada sinal deve gerar um `trade_id` e um ciclo completo de ordens: submissão, ack, posição na fila, fill por perna, cancelamento, inventário, fees, mark-to-market e fechamento. O PnL deve ser calculado apenas a partir dos fills daquele `trade_id`.

Para Order Flow, usar trades públicos e evolução de fila como aproximação conservadora, ou preferencialmente uma conta demo oficial. Exigir fill das duas pontas; fill unilateral deve abrir risco direcional real. Para arbitragem, registrar saldo de USDT e do ativo em cada exchange, usar a menor profundidade das duas pernas e cobrar rebalanceamento.

### 2. Corrigir bloqueios operacionais

Corrigir o hash DEX, fracionar a assinatura Bitget em múltiplas mensagens/conexões, preservar confirmações do Pump entre rotações, expirar Whale Flow na leitura, separar os rankings de Arbitragem e Order Flow, reservar de fato as vagas exploratórias e fazer interseção Bybit–Bitget.

### 3. Fazer o risco representar perdas possíveis

Persistir posições e ordens abertas. Reconciliar o estado com a venue ao iniciar. Definir stop/saída por estratégia e usar a pior perda observada com margem no sizing. Contabilizar as duas pernas da arbitragem e saldo por venue. Corrigir o reset do halt e impedir reduções repetidas após restart.

### 4. Validar uma estratégia por vez

Começar por arbitragem taker, pois o critério de fill é observável. Separar dados por símbolo, direção e regime; usar períodos de treino, validação e teste em ordem temporal; manter o teste final intocado; medir expectativa líquida, intervalo de confiança, fill ratio, slippage p95, drawdown e probabilidade de ruína. Kelly só deve entrar depois de a expectativa fora da amostra permanecer positiva.

Order Flow maker deve vir depois, pois fila e seleção adversa são mais difíceis. News, Macro, Launch e Multi-Asset ainda precisam de hipóteses negociáveis e backtests próprios; não devem competir por capital enquanto forem apenas sensores.

### 5. Critério mínimo para qualquer capital real

- Conta demo oficial e shadow mode produzindo o mesmo lifecycle de ordens do executor final.
- PnL fora da amostra positivo após todas as taxas, slippage e rebalanceamento.
- Limite inferior do intervalo de confiança da expectativa acima de zero.
- Profit factor líquido maior que 1,3 em regimes distintos, sem depender de um símbolo ou melhor trade.
- Drawdown máximo e probabilidade de ruína explicitamente aceitos antes do sizing.
- Testes de reinício, desconexão, fill parcial, perna órfã, dados atrasados e corrupção de estado.

## Verificações executadas

- `cargo test --locked`: compilação concluída; **0 testes do projeto**.
- Probes isolados executados durante a auditoria: **7/7 defeitos reproduzidos** na revisão `01913ef`.
- RPC Ethereum `web3_sha3`: tópico correto de `PairCreated(address,address,address,uint256)` = `0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9`; o código usa uma string de tamanho inválido.
- Payload Bitget calculado com a semente atual: 72 canais, 4.240 bytes; limite documentado: 4.096 bytes.
- API pública atual: Bybit BTCUSDT informa mínimo de US$ 5; Bitget BTCUSDT informa mínimo de US$ 1 e taxa pública de 0,20% neste endpoint.
- `orchestrator/data` está ausente nesta cópia; os resultados históricos do README não puderam ser reprocessados.

## Conclusão operacional

O projeto “falha” porque otimiza frequência e composição em cima de PnL que não corresponde a fills. A velocidade amplifica o erro de medição. A próxima meta correta não é fazer mais trades; é provar que uma única classe de trade possui expectativa líquida positiva, executável e reproduzível. Só depois a frequência e o escalonamento passam a ajudar.
