# Relatório: por que o equity simulado subiu de US$195 para US$2.727 em ~2h, e o que estava (e não estava) real nisso

**Data:** 13/08/2026
**Janela analisada:** 08:39 UTC (restart pós-correção do bug de deadlock) até ~10:40 UTC

---

## 1. Linha do tempo do que aconteceu

1. **02:36–03:03 UTC:** sistema operando normalmente após um restart, depois **parou completamente de executar trades** (Order Flow e Arbitragem) sem nenhum erro visível — só Arbitragem esporadicamente.
2. **Causa raiz encontrada e corrigida:** o guard "nunca aumentar o tamanho da ordem logo após uma perda" (proteção anti-martingale, em `risk::evaluate`) comparava cada novo candidato contra o tamanho exato do **último trade individual**, que varia por símbolo (Kelly hierárquico, fração 0,10–2,00). Bastava o próximo candidato pertencer a um símbolo com fração maior que o do trade perdido para ser bloqueado. Como só um trade **bem-sucedido** libera esse estado, e nenhum conseguia passar, virava um **deadlock permanente** — sem erro, sem log visível, sem kill-switch acionado.
3. **Correção:** a comparação passou a usar a perna **base** da estratégia (`portfolio.leg_size`, estável) em vez do último trade individual (ruidoso). Verificado ao vivo por 27+ minutos direto (o ponto exato onde travava antes) sem nenhuma parada.
4. **Efeito colateral revelado:** com o deadlock removido, o sistema passou a executar em ritmo altíssimo (~400-430 trades/minuto, combinando Order Flow + Arbitragem) — ritmo que sempre existiu na configuração, mas nunca tinha sido observado de verdade porque o bug travava tudo antes.
5. Resultado: equity simulado saltou de US$195 → US$380 (28min) → US$2.727 (1h25min), um crescimento composto muito mais rápido do que o parâmetro de escalonamento (`min_cycles_between_scale = 50`) foi pensado para permitir — porque ele conta **ciclos**, não **tempo**, e com centenas de ciclos por minuto, 50 ciclos passam em segundos.

## 2. Por que esse crescimento não prova nada sobre lucratividade real

### 2.1 O que é real vs. simulado nesse número

| Real | Simulado / assumido |
|---|---|
| Preço, book, spread medido no instante do sinal | Se aquele spread teria realmente sido capturado |
| Taxas da exchange descontadas do edge (Bybit taker 0,10%, Bitget taker 0,10%, Bybit maker 0,08%) | A probabilidade de acerto (`confidence`) — fixa em 0,45 pro Order Flow, fórmula arbitrária (`0,5 + edge×25`) pra Arbitragem |
| Limiar mínimo de edge calibrado por análise de breakeven real | O resultado de cada trade — decidido por sorteio (`rng.gen_bool`), não por confirmação de preço real subsequente |
| — | Fill parcial e falha de 2ª perna também sorteados, não medidos |

### 2.2 Os números exatos (calculados com dado real do próprio sistema, ~2.795 trades numa janela de 404s)

| | Order Flow | Arbitragem |
|---|---|---|
| Volume no período | 2.368 trades (85%) | 427 trades (15%, = 854 ordens reais, 1 por exchange) |
| Tamanho médio | US$21,17 | US$41,30 |
| Taxa de acerto **assumida** | 43,2% | 70,2% |
| Ganho médio quando acerta | +US$0,2324 | +US$0,3282 |
| Perda média quando erra | −US$0,0554 | −US$0,1781 |
| **Taxa de acerto mínima pra não perder dinheiro** | 19,3% | 35,2% |

A margem sobre o breakeven parece confortável (~2x), mas a pergunta certa não é "43% é boa margem sobre 19%" — é **"por que 43% e não 15%, e ninguém verificou"**. Esse número nunca foi validado contra o mercado real.

### 2.3 Por que a taxa real é provavelmente mais baixa que a assumida

- **Seleção adversa (estrutural, não hipótese):** Order Flow captura spread agindo como *maker* — a ordem fica parada esperando alguém bater nela. Quando alguém bate, estatisticamente é porque o preço estava indo pra lá. Isso não está modelado.
- **Latência e competição:** o sistema roda numa conexão comum, olhando o mesmo book público que bots profissionais colocados fisicamente ao lado dos servidores da exchange também olham, com latência sub-milissegundo. Se o spread ainda está lá quando este sistema reage, é sinal de que ninguém mais rápido quis pegar.
- **Ritmo de execução (~415 trades/min, ~8 ordens/seg contando as duas pernas da arbitragem):** provavelmente esbarraria em rate limit de API de conta de varejo antes mesmo da questão financeira.
- **Capital fantasma:** o design original assumia US$100 pré-financiados fixos por exchange (comentário no próprio `arbitrage.rs`). Hoje a perna simulada já passa de US$266-384 por estratégia — nenhum desse capital existe de verdade; o número compõe porque é grátis compor um número, não porque há capital real acompanhando.

## 3. O que estava genuinamente bugado vs. o que é lacuna de realismo conhecida

- **Bugado (corrigido nesta sessão):** o deadlock do guard anti-martingale. Isso não tinha nada a ver com realismo — era um erro de lógica que travava o sistema inteiro silenciosamente.
- **Bugado (corrigido nesta sessão):** rotulagem errada de edge medido em spot sendo salvo como se fosse de perpétuo.
- **Não é bug, é lacuna conhecida e documentada no próprio código:** a probabilidade de acerto usada pra decidir cada trade é um número assumido, não medido. O comentário em `order_flow.rs` já admitia isso: *"não modela fila de execução real... é uma aproximação inicial"*.

## 4. Mudança em andamento (a partir deste relatório)

1. **Escalonamento por tempo, não por ciclos** — `min_cycles_between_scale` deixa de contar ciclos e passa a exigir uma janela real de tempo decorrido, evitando composição artificialmente acelerada quando o ritmo de trades é alto.
2. **Confirmação de preço real para Order Flow e Arbitragem** — mesmo mecanismo que já existe e funciona para o Pump Exhaustion: em vez de sortear ganhou/perdeu no instante do sinal, o sistema registra o preço de entrada, espera uma janela real, e confere contra o preço/book real subsequente se o edge realmente sobreviveu. A taxa de acerto e o edge usados pra decidir cada trade passam a vir desse histórico real medido — não de um número escolhido à mão. Antes de existir amostra real suficiente, a estratégia fica informativa (`net_edge=0`), do mesmo jeito que Whale Watch/News/Macro já ficam hoje.

O objetivo é responder, com dado real e não suposição: será que esse ritmo de crescimento se sustenta quando o resultado de cada trade é validado contra o que o mercado realmente fez depois — ou ele desaparece (ou vira prejuízo) assim que a seleção adversa e a competição real entram na conta.
