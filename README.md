<div align="center">

<img src="docs/brand/aurumos-mark.svg" width="96" height="96" alt="AurumOS" />

# AurumOS

**Laboratório quantitativo para uma banca inicial de US$ 200 em uma única corretora.**

![status](https://img.shields.io/badge/status-shadow%20%2B%20Bybit%20Demo-f5c453?style=flat-square&labelColor=0c1117)
![venue](https://img.shields.io/badge/venue-Bybit%20linear-18d7ff?style=flat-square&labelColor=0c1117)
![capital real](https://img.shields.io/badge/capital%20real-desativado-ff4d67?style=flat-square&labelColor=0c1117)
![core](https://img.shields.io/badge/core-Rust-f5c453?style=flat-square&labelColor=0c1117)

</div>

## Objetivo

O AurumOS pesquisa operações curtas e repetíveis em perpétuos USDT da Bybit. O scanner acompanha um universo amplo, mas o executor só aloca a banca a uma hipótese que tenha demonstrado retorno líquido no próprio símbolo. A frequência desejada é de segundos; a frequência realizada depende da existência de vantagem depois de spread e taxas.

Não existe método legítimo que garanta transformar US$ 200 em um valor maior em minutos. O software trata crescimento como resultado de uma expectativa líquida comprovada, controle de perda e composição. A velocidade do loop não é usada como substituto de vantagem estatística.

## Estado atual

- **Uma corretora:** Bybit, mercado linear USDT. A arbitragem Bybit–Bitget foi retirada do runtime porque exigiria capital em duas venues.
- **Um executor com dois backends:** Microestrutura multi-horizonte de 5, 15, 30 e 60 segundos (`Strategy::OrderFlow`) roda em shadow por padrão e pode enviar ordens exclusivamente à Bybit Demo quando ativada com credenciais Demo.
- **Sete módulos de inteligência/pesquisa:** Funding Carry, News, Launch, Pump Exhaustion, Whale Watch, Macro e Liquidation Hunter. Eles ampliam a pesquisa, mas não geram PnL nem ordens.
- **Capital real estruturalmente desligado:** o único hostname autenticado compilado é `api-demo.bybit.com`; não há rota para o endpoint de produção.

Os resultados antigos de “Arbitragem +US$ 1.002,79” e “Order Flow +US$ 2.268,81” foram removidos. Eles vinham de reamostragem de retornos históricos e não correspondiam aos fills da oportunidade contabilizada.

O projeto Snowball foi auditado e parcialmente incorporado. A ideia aproveitada foi o cash-and-carry spot+perp dentro da própria Bybit; o motor de duas corretoras, os saldos paper e as premissas de fill maker não foram importados. O novo radar usa bid/ask e tamanho L1 das quatro pontas, turnover das duas pernas, `fundingIntervalHour`, `nextFundingTime` e histórico de funding efetivamente liquidado. Ele permanece bloqueado para execução até existir histórico forward e um executor reconciliado de duas pernas. Veja [`docs/audit_2026_09_06/COMPARACAO_SNOWBALL_E_INTEGRACAO.md`](docs/audit_2026_09_06/COMPARACAO_SNOWBALL_E_INTEGRACAO.md).

Estado e eventos usam arquivos separados por backend. O sinal OFI dinâmico usa histórico/ranking `microstructure_v9`; resultados dos modelos anteriores permanecem como baseline e não entram na decisão atual. A v9 forma a janela exclusivamente com `cts`/`T` do matching engine, guarda junto de cada retorno a força do sinal, OFI, agressão, notional negociado, fila e spread, e publica coortes de pesquisa que não autorizam ordens.

## Como uma operação shadow nasce

1. O feed recebe cada alteração do melhor bid/ask e os negócios públicos de um perpétuo.
2. O scanner calcula OFI dinâmico a partir de mudanças de preço/quantidade causadas por ordens e cancelamentos, exigindo alinhamento com a agressão negociada, book recente e profundidade mínima.
3. A entrada é marcada no ask para long ou no bid para short.
4. A saída é marcada no lado oposto do book em 5, 15, 30 e 60 segundos.
5. O retorno aplica a classe pública atual do contrato: 0,055% taker por lado em Major/Altcoin, 0,10% em pré-listagem e 0,11% na Innovation Zone. Símbolo sem classificação usa 0,11%; TradFi não entra no universo cripto.
6. Cada candidato alimenta separadamente o histórico do próprio símbolo+horizonte. Janelas sobrepostas da mesma combinação são bloqueadas.
7. A combinação só fica executável depois de 200 amostras válidas. A série é dividida cronologicamente entre treino e validação; o menor LCB95, inclusive após remover o melhor trade da validação, precisa exceder 0,01% líquido.
8. O resultado carrega o mesmo `signal_id` da oportunidade. Sem relatório correspondente, a reserva expira como `unfilled` e o PnL permanece inalterado.

No modo shadow, bid/ask público não prova fill real, latência do gateway ou slippage além do topo. O backend Demo mede esses itens pelos fills da conta de teste: abre uma ordem market com tolerância de slippage, instala um stop market de posição inteira na venue, espera o horizonte aprovado de 5 a 60 segundos, fecha com `reduceOnly` e calcula o PnL absoluto por `execValue` e `execFee`. Se o stop não puder ser confirmado, tenta zerar imediatamente. Se o estado ficar incerto, a exposição permanece reservada e novas ordens são bloqueadas.

## Banca e risco

A configuração inicial fica em [`orchestrator/config/risk.toml`](orchestrator/config/risk.toml):

| Regra | Valor inicial |
|---|---:|
| Equity operacional | US$ 200 |
| Notional pretendido por operação | até US$ 25 |
| Alavancagem do executor | 1x |
| Risco simultâneo total | 0,50% da equity |
| Aviso de perda diária | 1,00% |
| Bloqueio diário | 1,50% |
| Bloqueio semanal | 4,00% |
| Bloqueio desde o pico | 7,00% |

O sizing arredonda a quantidade pelo `qtyStep`, verifica `minOrderQty` e `minNotionalValue` obtidos do endpoint oficial de instrumentos. O reset manual do drawdown cria uma nova linha de base; ele não religa imediatamente a mesma trava. Resultados `unfilled` liberam toda a exposição sem alimentar wins, losses ou Kelly.

## Arquitetura de uma operação profissional

```text
Market Data       Bybit book + public trades + sensores externos
      ↓
Research          amostra por símbolo, custo completo, LCB95, logs brutos
      ↓
Strategy          microestrutura direcional com janelas de 5/15/30/60 segundos
      ↓
Portfolio/Risk    sizing, limites simultâneos, drawdown, rate limit, lote
      ↓
Execution         shadow ou Bybit Demo; signal_id → ordem/fill → resultado
      ↓
Accounting        equity, reservas, exposição e histórico persistente
      ↓
Monitoring        dashboard, heartbeats, eventos e kill-switch
```

Os sensores cobrem mais mercados; o executor permanece estreito. Uma banca de US$ 200 não consegue operar o “universo inteiro” com qualidade ao mesmo tempo, mas consegue pesquisar amplamente e concentrar capital onde a evidência líquida é melhor.

## Executar

```powershell
cargo test --locked
cargo run --locked -p orchestrator
```

O padrão é shadow. Para usar uma conta de teste, copie [`orchestrator/.env.example`](orchestrator/.env.example) para `orchestrator/.env`, defina `AURUMOS_EXECUTION_MODE=demo` e informe chaves criadas no ambiente Bybit Demo. No boot, o processo sincroniza o relógio, exige pelo menos US$ 200 disponíveis e recusa contas com posições ou ordens lineares abertas. Antes de cada entrada, confirma modo one-way e alavancagem 1x no símbolo. A contabilidade local continua limitada à banca inicial de US$ 200, mesmo que o saldo fictício da conta Demo seja maior.

Dashboard: `http://127.0.0.1:7878`.

O relatório vivo do radar de carry fica em `orchestrator/data/funding_carry_validation_v1.json`; as observações completas ficam em `orchestrator/data/raw_funding_carry_bybit.jsonl`. Previsões congeladas até 30 minutos antes do funding e a posterior taxa liquidada são mantidas no WAL `orchestrator/data/funding_carry_forward_v1.jsonl`. Os três arquivos são pesquisa, não autorização de ordem.

O arquivo `orchestrator/data/KILL` pausa novas aberturas. O arquivo `orchestrator/data/RESET_DRAWDOWN_HALT` reconhece e redefine a linha de base da trava de drawdown total.

Em paralelo, `strategy_validation_maker_entry_v3.json` mede uma alternativa de menor custo sem enviar ordens: assume 250 ms de latência de colocação, exige que o preço ainda seja o melhor nível na chegada e só conta fill quando negócios públicos futuros consomem a fila visível inteira e a ordem de US$ 25 até 500 ms após o sinal; a saída permanece taker. Esse relatório mantém `execution_enabled=false` mesmo se uma coorte passar o portão de pesquisa.

## Critérios para avançar

1. Coletar amostras em dias, horários e regimes diferentes.
2. Preservar o corte temporal de treino/validação já implantado e reservar um teste final nunca usado para escolher limiares.
3. Confirmar retorno líquido positivo, LCB95 positivo, drawdown compatível e estabilidade sem o melhor trade em vários regimes.
4. Executar uma campanha na Bybit Demo; o adaptador com IDs idempotentes, fills, posição e reconciliação já está implantado.
5. Comparar shadow e Demo por fill, preço e latência e guardar o teste final sem ajuste posterior.
6. Qualquer decisão futura sobre capital real deve ser explícita e posterior a essa validação; o binário atual não contém endpoint real.

Documentação oficial usada no modelo: [taxas da Bybit](https://www.bybit.com/en/help-center/article/Trading-Fee-Structure), [grupos públicos de contratos](https://bybit-exchange.github.io/docs/v5/market/fee-group-info), [tickers spot/linear](https://bybit-exchange.github.io/docs/v5/market/tickers), [instrumentos e intervalo de funding](https://bybit-exchange.github.io/docs/v5/market/instrument), [histórico de funding liquidado](https://bybit-exchange.github.io/docs/v5/market/history-fund-rate), [orderbook WebSocket](https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook), [public trades](https://bybit-exchange.github.io/docs/v5/websocket/public/trade), [ambiente Demo](https://bybit-exchange.github.io/docs/v5/demo), [criação de ordens](https://bybit-exchange.github.io/docs/v5/order/create-order), [modo da posição](https://bybit-exchange.github.io/docs/v5/position/position-mode), [alavancagem](https://bybit-exchange.github.io/docs/v5/position/leverage), [stop da posição](https://bybit-exchange.github.io/docs/v5/position/trading-stop), [histórico de execuções](https://bybit-exchange.github.io/docs/v5/order/execution), [posições](https://bybit-exchange.github.io/docs/v5/position) e [saldo](https://bybit-exchange.github.io/docs/v5/account/wallet-balance).

O diagnóstico completo e o relatório de remediação estão em [`docs/audit_2026_09_05/RELATORIO_AUDITORIA.md`](docs/audit_2026_09_05/RELATORIO_AUDITORIA.md).
