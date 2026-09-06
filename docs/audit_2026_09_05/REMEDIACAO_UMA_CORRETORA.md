# Remediação: US$ 200 em uma única corretora

**Venue de execução:** Bybit  
**Produto:** perpétuos lineares USDT  
**Modos atuais:** shadow por padrão e Bybit Demo autenticado opcional  
**Estratégia autorizada a produzir PnL:** microestrutura direcional de 5, 15, 30 e 60 segundos

## Decisão de arquitetura

O capital inteiro foi modelado como uma única carteira de US$ 200 na Bybit. A arbitragem Bybit–Bitget foi retirada do runtime: embora estivesse apresentada como estratégia rápida, ela exigia caixa e inventário nas duas corretoras, duas ordens, reconciliação das duas pernas e custo de rebalanceamento. Isso contradizia a restrição de uma única corretora.

O sistema ainda pode observar dados externos como contexto, mas só uma fonte pode reservar capital: `OrderFlow`, quando classificada como `ExecutableQuoted`. News, Launch, Pump Exhaustion, Whale Watch, Macro e Liquidation Hunter permanecem `ObservationOnly`; não alteram equity, win rate, profit factor ou Kelly. O feed Alpaca foi retirado do runtime para não criar dependência de uma segunda corretora.

## Fluxo executável

1. O universo consulta os perpétuos USDT negociáveis e líquidos na Bybit.
2. O WebSocket assina `orderbook.1` e `publicTrade` em lotes de dez tópicos.
3. Um candidato exige book recente, profundidade mínima, desequilíbrio do topo e negócios agressivos alinhados na mesma direção.
4. Long marca entrada no ask e saída no bid; short marca entrada no bid e saída no ask.
5. A saída é observada em 5, 15, 30 e 60 segundos. Cada par símbolo+horizonte é validado separadamente, e o cálculo deduz 0,055% taker em cada lado, total de 0,11% por ciclo.
6. Cada resultado entra somente no histórico daquele símbolo. O mesmo símbolo não abre janelas sobrepostas.
7. São necessárias 200 amostras executáveis. A série é dividida cronologicamente em treino e validação; o menor LCB95, inclusive após remover o melhor trade da validação, precisa exceder 0,01% líquido.
8. Sinal, decisão, execução e PnL carregam o mesmo `signal_id`. Um relatório ausente, inválido ou sem ID correspondente não produz trade.
9. O tamanho é limitado pela menor profundidade observada entre entrada e saída, arredondado por `qtyStep` e validado contra `minOrderQty` e `minNotionalValue` atuais da Bybit.

## Por que o projeto falhava

A versão auditada escolhia aleatoriamente um retorno histórico e o aplicava a uma oportunidade nova. Assim, uma pequena quantidade de observações podia virar milhares de “trades”, e o próprio PnL falso alimentava Kelly e escalonamento. Order Flow também tratava preço parado como captura do spread sem demonstrar fill. A velocidade do loop multiplicava o erro contábil.

Outros defeitos confirmados incluíam: risco de duas pernas contado como uma; perda realizada sem relação com o risco reservado; confirmação de Pump destruída antes de maturar; tópico Ethereum inválido; assinatura Bitget grande demais; reset de drawdown que se retravava; depósitos de whales vencidos que permaneciam ativos; e vagas de exploração eliminadas por truncamento.

## Correções implantadas

| Área | Estado após a remediação |
|---|---|
| Resultado | Retorno histórico aleatório removido; PnL vem apenas da cotação de saída vinculada ao sinal. |
| Migração | Estado, eventos e ranking antigos foram isolados por arquivos v2; PnL/edge inválidos não são reidratados. |
| Uma corretora | Arbitragem excluída da compilação/runtime e risk engine aceita somente `OrderFlow` da Bybit. |
| Custos | Entrada e saída taker cobradas; um movimento bruto de 0,05% continua negativo após 0,11% de taxas. |
| Evidência | Histórico v2 por símbolo, 200 amostras, corte temporal e LCB95 positivo sem o melhor trade da validação. |
| Liquidez | Notional preenchível usa a menor profundidade de entrada/saída. |
| Filtros | Falha fechada sem filtro de instrumento; quantidade arredondada para baixo. |
| Risco | Exposição é reservada até o relatório; `unfilled` libera a reserva sem fabricar resultado. |
| Drawdown | Reset manual estabelece uma nova linha de base e evita relatch imediato. |
| Universo | 220 símbolos com pelo menos 50 vagas realmente não comprovadas. |
| Pump | Pendências sobrevivem à rotação; funding negativo não reforça short. |
| Whale | Depósitos vencidos são removidos na leitura. |
| DEX | Hash completo do evento `PairCreated` corrigido. |
| Monitoramento | Eventos de oportunidade, decisão, execução e PnL expõem `signal_id`; dashboard identifica shadow. |

## Economia de uma banca de US$ 200

Com notional inicial de US$ 25, a taxa taker padrão de ida e volta custa aproximadamente US$ 0,0275 por tentativa preenchida. Para ganhar US$ 0,05 líquidos, o preço precisa entregar cerca de 0,31% bruto dentro do horizonte escolhido: 0,11% paga taxas e 0,20% permanece como retorno. Na Innovation Zone, só as taxas taker de ida e volta custam 0,22%. Antes de composição, US$ 200 adicionais a US$ 0,05 por operação exigiriam cerca de 4.000 operações líquidas vencedoras, sem contar perdas.

Isso torna o objetivo dependente de três fatores mensuráveis: vantagem líquida por operação, frequência de fills e preservação de capital. Aumentar a frequência quando a expectativa é negativa acelera a perda. Por isso o runtime pode passar longos períodos sem executar; ausência de edge é uma resposta correta do sistema.

O escalonamento continua sem teto nominal, mas não é automático por desejo de crescimento. Ele exige amostra recente, profit factor mínimo, resultado positivo sem o melhor trade, recuperação do drawdown e intervalo mínimo entre aumentos. A perna também permanece limitada a 30% da equity e o notional agregado a 1x.

## Estrutura equivalente a uma operação profissional

| Função | Responsabilidade no AurumOS |
|---|---|
| Market Data | WebSockets, frescor, normalização, logs brutos e rotação de universo. |
| Pesquisa Quantitativa | Hipótese, custos, amostras por ativo, validação temporal e análise por regime. |
| Estratégia | Regras determinísticas que geram sinal e horizonte. |
| Portfolio & Risk | Sizing, correlação, exposição, drawdown, leverage e kill-switch. |
| Execution | Shadow e Bybit Demo; ordens idempotentes, stop na venue, fills, `reduceOnly`, posição e reconciliação. |
| Contabilidade | Equity, reservas, PnL por trade e persistência. |
| Operações | Dashboard, heartbeats, alertas, recuperação de conexão e trilha de eventos. |
| Governança | Critérios objetivos para promover shadow → demo → capital limitado. |

## Portões para chegar a dinheiro real

O estágio atual prova coleta e contabilidade shadow, não lucratividade. A sequência técnica é:

1. Coletar amostras em vários horários e regimes sem mudar os limiares com base no período de avaliação.
2. Separar treino, validação e teste final em ordem temporal.
3. Medir expectativa líquida, LCB95, fill ratio, slippage p95, drawdown e concentração por símbolo.
4. Executar campanha com o adaptador Bybit Demo já implantado, usando `signal_id` como chave idempotente.
5. Validar em operação prolongada a reconciliação de saldo, posição, ordens e fills; adicionar stream privado como otimização, mantendo REST como fallback.
6. Rodar shadow e Demo simultaneamente e comparar preço, fill, quantidade e latência.
7. Promover uma alocação real pequena somente depois de resultado fora da amostra positivo e estável. A promoção precisa de decisão explícita; o código atual não possui rota de ordem real.

## Verificação executada após a correção

- `cargo test -p orchestrator --locked`: 49 testes aprovados.
- `cargo clippy --locked --all-targets -- -D warnings`: aprovado.
- Execução conectada aos endpoints públicos: 220 perpétuos no universo, 440 canais de book/trades assinados em lotes, eventos e históricos gravados.
- A coleta inicial apresentou médias líquidas próximas de −0,10% em vários símbolos. Nenhum foi promovido artificialmente e não houve PnL inventado.
- O modelo ativo registra taxa por grupo público, exclui TradFi e mantém coortes por força do sinal. Um pesquisador maker separado exige consumo futuro da fila visível e permanece sem caminho de execução.

Referências oficiais: [estrutura de taxas da Bybit](https://www.bybit.com/en/help-center/article/Trading-Fee-Structure), [grupos públicos de contratos](https://bybit-exchange.github.io/docs/v5/market/fee-group-info), [orderbook público](https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook), [negócios públicos](https://bybit-exchange.github.io/docs/v5/websocket/public/trade), [instrumentos e filtros](https://bybit-exchange.github.io/docs/v5/market/instrument), [ambiente Demo](https://bybit-exchange.github.io/docs/v5/demo) e [stop da posição](https://bybit-exchange.github.io/docs/v5/position/trading-stop).
