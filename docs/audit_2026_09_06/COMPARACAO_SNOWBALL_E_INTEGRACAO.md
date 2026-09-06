# Auditoria Snowball × AurumOS e integração segura

Data da auditoria: 06/09/2026  
Snowball observado em: `C:\Users\Renan\Projetos\Snowball`, branch `main`, commit `323ae99`  
AurumOS observado em: `C:\Users\Renan\Documents\GitHub\AurumOS`, branch `codex/realistic-execution-core`

## Veredito

O Snowball não era um robô conectado a corretoras. Era um laboratório paper em Node/TypeScript com coleta pública via CCXT e um livro-caixa local. Ele tem ideias úteis de persistência, WAL, reconciliação e cash-and-carry, mas seus números não demonstram uma estratégia pronta para dinheiro real.

A junção correta não é copiar o Snowball inteiro. O motor principal dele exige Bybit e Bitget e divide o capital entre duas contas, contrariando a exigência atual de US$ 200 em uma só corretora. A ideia aproveitável é spot+perp dentro da própria Bybit. Ela foi portada ao AurumOS como radar observacional com dados e gates mais estritos; nenhuma rota desse módulo consegue enviar ordem.

Também não existe evidência de que o objetivo de aumentar US$ 200 em segundos ou minutos seja atingível. O sistema agora mede microestrutura em 5, 15, 30 e 60 segundos e carry em horas. Cada par símbolo+horizonte precisa se validar separadamente; o histórico anterior de 5 segundos foi preservado. Nenhum candidato tinha sido aprovado no momento desta revisão. Rodar mais rápido sem edge líquido apenas multiplica taxas.

## O que foi verificado no Snowball

### Código e credenciais

- 389 arquivos rastreados pelo Git, 117 arquivos TypeScript/CJS/MJS e 23 arquivos de teste ou pastas de testes no inventário observado.
- `npm test`: 100 testes passaram.
- Não havia `AGENTS.md` nem instrução adicional de repositório.
- O `.env` continha somente Telegram. Não havia API key/secret de Bybit, Bitget ou outra corretora.
- O README, o contexto e o código afirmam de forma consistente que nenhuma ordem real era emitida.
- O `.env` e `dashboard-v2/.env` estavam corretamente ignorados pelo Git.

### Resultado econômico gravado

O estado final de `auditoria/progression/compete/snowball-2ex` foi atualizado pela última vez em 11/08/2026. A amostra operacional observada no feed spot-perp cobria cerca de 49 horas, de 09/08 a 11/08.

| Medida | Resultado paper |
|---|---:|
| Aberturas | 29 |
| Fechamentos | 27 |
| Ganhos | 3 |
| Perdas | 24 |
| Taxa de acerto | 11,1% |
| PnL fechado agregado | +US$ 0,5834 |
| PnL vitalício atribuído à Bybit | +US$ 2,4377 |
| PnL vitalício atribuído à Bitget | −US$ 1,8475 |
| Spot-perp fechadas | 8 |
| Spot-perp vencedoras | 0 |
| Spot-perp PnL | aproximadamente −US$ 0,38 |

O lucro agregado não é validação. Ele depende de três fechamentos tardios positivos; 24 dos 27 fechamentos perderam. O próprio gate econômico intermediário registrou `LIBERADO=false`, falta de duas janelas/regimes e PnL negativo no checkpoint. Outro relatório declarou que a escolha de duas exchanges era insuficiente, mas o README posterior a apresentou como decisão tomada.

### Por que a simulação não transfere diretamente para dinheiro real

1. **Não havia fills.** O modelo debitava uma taxa fixa de maker, mas não criava ordem, não media fila, rejeição, cancelamento, latência ou seleção adversa.
2. **“Funding real” era uma aproximação.** `boundariesCruzados` contava fronteiras de tempo alinhadas ao epoch e multiplicava pela última taxa observada. O código não consultava o histórico liquidado nem o crédito recebido na conta. Símbolos podem ter intervalos diferentes e a taxa pode mudar antes do settlement.
3. **A perna spot-perp não reconciliava preços futuros.** O PnL considerava funding e custos constantes, mas não o basis de saída, divergência de quantidade, arredondamento de lote, `minNotional`, partial fill nem risco de uma perna ficar aberta.
4. **A fonte podia ficar velha.** O coletor só anexava ao arquivo quando existiam oportunidades. Um ciclo vazio não criava watermark; o leitor podia continuar vendo o último ciclo antigo. Uma falha parcial também podia parecer desaparecimento de funding.
5. **A “reconciliação” não travava.** Em caso de erro maior que um centavo, `reconciliar()` apenas escrevia `RECONCILIATION_BREAK` e imprimia erro; o processo seguia. Ela reconciliava o livro local, não o saldo/posição da venue.
6. **Yield da reserva era sintético.** O motor somava 6% ao ano sobre saldo livre sem provar que esse saldo estava aplicado num produto resgatável e simultaneamente disponível para margem.
7. **Amostra curta e seleção posterior.** A escolha Bybit+Bitget e os levers foram ajustados na mesma amostra curta usada para descrevê-los. Não havia campanha autenticada fora da amostra.
8. **Duas corretoras.** O cross-exchange exige US$ 100 em cada venue, expõe a transferências, divergência de margem e risco operacional duplo. Isso não atende à regra atual.

## Diferenças relevantes

| Tema | Snowball | AurumOS após a integração |
|---|---|---|
| Corretora | Bybit + Bitget no motor principal | Somente Bybit |
| Capital | US$ 100 por exchange | US$ 200 em uma conta |
| Execução | Paper local, sem API de trading | Shadow ou Bybit Demo autenticada |
| Produção | Não existia | Continua estruturalmente ausente |
| PnL | Fórmula local | Fill por `signal_id`; no Demo usa `execValue` e `execFee` |
| Proteção | Modelo local | Stop confirmado na venue, `reduceOnly`, `OpenRisk` fail-closed |
| Estratégia rápida | Funding, horizonte de horas | Microestrutura de 5/15/30/60 s, ainda sem edge aprovado |
| Carry | Taxa atual + custo fixo | Ticker V5, quatro lados L1, turnover, basis, `nextFundingTime` e histórico liquidado |
| Persistência | Checkpoint/WAL era uma boa direção | Estado v1 com checksum SHA-256, troca atômica e backup recuperável |
| Validação | Gates extensos, mas decisões posteriores os contrariaram | Gate no código; módulo carry não possui executor |
| Segredos | Só Telegram | Arquivo local ignorado; chave Bybit Demo autenticada |

## O que foi incorporado

O novo módulo `orchestrator/src/sources/funding_carry.rs` faz duas consultas públicas simultâneas: tickers spot e linear da Bybit. Para cada símbolo USDT comum ele mede:

- melhor bid, ask e tamanho das duas pernas;
- capacidade L1 mínima para abrir e fechar spot e perp;
- turnover de 24 horas nos dois mercados;
- funding atual, intervalo individual e próximo horário de funding;
- spread executável agregado;
- basis de entrada sem creditar prêmio positivo como lucro garantido;
- custo VIP 0 base e cenário estressado das zonas especiais;
- buffer adverso de basis e multiplicador de segurança de custo;
- payback em períodos e horas;
- histórico dos últimos 60 fundings efetivamente liquidados para os dez melhores candidatos;
- blocos não sobrepostos de três settlements, LCB95 e validação sem o melhor bloco.

O módulo escreve:

- `orchestrator/data/funding_carry_validation_v1.json`: relatório atual;
- `orchestrator/data/raw_funding_carry_bybit.jsonl`: observações para replay futuro;
- `orchestrator/data/funding_carry_forward_v1.jsonl`: WAL de previsões congeladas até 30 minutos antes do funding e posterior taxa liquidada;
- heartbeat `funding_carry` no dashboard.

O estado de promoção é fixo em `BLOCKED_NEEDS_FORWARD_SETTLEMENTS_AND_TWO_LEG_EXECUTOR`. Passar no filtro atual não cria `Opportunity`, reserva capital, gera PnL ou envia ordem.

O coletor forward deduplica cada par símbolo/settlement, preserva a taxa prevista e o custo conservador daquele instante e, dois minutos após o horário, procura o `fundingRateTimestamp` exato no histórico público liquidado. O relatório passa a mostrar quantidade pendente/liquidada, erro médio, erro absoluto e quantas taxas liquidadas cobririam o custo de três períodos se fossem repetidas. Isso elimina a aproximação temporal do Snowball; a amostra precisa ser acumulada com o processo rodando e ainda não representa crédito efetivo numa posição da conta.

A parte útil da disciplina de persistência do Snowball também foi incorporada ao estado crítico do AurumOS. Cada save agora contém versão de schema e checksum SHA-256, é escrito e sincronizado primeiro em `.tmp`, move o estado anterior para `.prev` e só então promove o novo arquivo. No boot, o motor valida números financeiros e checksum; se o principal estiver incompleto ou corrompido, recupera o backup válido. O histórico de eventos só é descartado quando nem o primário nem o backup são recuperáveis. Isso impede um crash de restaurar silenciosamente US$ 200 e apagar o drawdown real da sessão.

## Primeiro scan real da integração

| Medida | Resultado em 06/09/2026 |
|---|---:|
| Pares comuns spot/perp | 290 |
| Funding atual positivo | 247 |
| Passaram custo/liquidez/payback atual | 0 |
| Históricos liquidados consultados | 10 |
| Passaram o filtro histórico | 0 |
| Melhor payback estressado | 12,8566 períodos |
| Teto pré-registrado | 3 períodos |

Logo, o mercado observado não oferecia carry rápido que cobrisse o cenário de custo e basis. Isso melhora o sistema porque impede transformar uma taxa nominal alta em uma operação negativa, mas não cria lucro quando a oportunidade não existe.

## O que ainda falta antes de executar spot-perp

1. Manter o coletor rodando até acumular settlements forward em horários e regimes diferentes. A comparação automática entre taxa prevista e liquidada já existe; ainda falta comparar com o crédito da conta Demo quando houver posições carry controladas.
2. Construir um executor separado de duas pernas. Ele precisa controlar IDs por perna, quantidade base idêntica, arredondamento, partial fills, timeout, cancelamento e hedge/resgate quando só uma perna encher.
3. Consultar a taxa efetiva da conta autenticada e classificar pares de zona especial; o radar atual usa cenário estressado público por segurança.
4. Reconciliar saldo spot, posição perp, ordens abertas, execuções, funding e fees diretamente com a Bybit.
5. Medir basis de saída e mark-to-market durante toda a posição. Delta nominal zero não elimina basis, liquidação, ADL ou falha operacional.
6. Executar uma campanha Demo fora da amostra e exigir resultado líquido positivo sem o melhor trade/bloco.
7. Só depois decidir se esse executor merece existir em produção. O binário atual não contém endpoint mainnet.

## Credencial Demo

`orchestrator/.env` foi criado e confirmado como ignorado pelo Git. A autenticação em `https://api-demo.bybit.com` passou no preflight. O arquivo não é mostrado em logs ou documentação. A chave exposta na conversa deve permanecer apenas em Demo; qualquer chave destinada a saldo real precisa ser nova e ter somente as permissões mínimas necessárias.

## Fontes oficiais

- [Bybit Demo Trading Service](https://bybit-exchange.github.io/docs/v5/demo)
- [Bybit V5 Get Tickers](https://bybit-exchange.github.io/docs/v5/market/tickers)
- [Bybit V5 Get Instruments Info](https://bybit-exchange.github.io/docs/v5/market/instrument)
- [Bybit V5 Get Funding Rate History](https://bybit-exchange.github.io/docs/v5/market/history-fund-rate)
- [Bybit Trading Fee Structure](https://www.bybit.com/en/help-center/article/Trading-Fee-Structure)
