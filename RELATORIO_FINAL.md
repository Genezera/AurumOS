# AurumOS — Relatório final da sessão (11-12/08/2026)

Sessão longa, autônoma, cobrindo: aplicação da marca oficial, framework de
backtesting (Fase 10), correção de um bug real de corrupção de dados,
reescrita completa do roadmap (v1 → v2.1) em duas rodadas de auditoria, e
implementação de tudo que era tecnicamente possível construir sem esperar
tempo real passar ou sem exigir que o usuário crie uma conta.

## 1. O que foi entregue nesta sessão

- **Brand kit aplicado** no dashboard (fontes, cores, ícones, logo) e no
  roadmap PDF (capa com o símbolo oficial, tipografia, diagrama de
  arquitetura vetorial, sumário com números de página).
- **Bug real corrigido**: `events.jsonl` corrompia linhas quando dois
  módulos gravavam eventos ao mesmo tempo (duas escritas separadas em vez
  de uma) — descoberto ao construir a ferramenta de backtest, corrigido em
  `events.rs`.
- **Framework de backtesting (Fase 10)** — `backtests/`:
  - `live_performance_report.py` — métricas reais por estratégia a partir
    de `events.jsonl`.
  - `pump_exhaustion_historical.py` — backtest histórico de verdade contra
    dado real da Bybit (klines + funding rate, 60 dias, 30 símbolos).
- **Log bruto contínuo** (`data/raw_arbitrage.jsonl`, `raw_order_flow.jsonl`,
  `raw_pump_exhaustion.jsonl`) — respondendo diretamente à pergunta "tudo
  que está rodando vai estar sempre coletando dado": agora sim, estado
  contínuo (spread, funding, book), não só o que já virou decisão.
- **Roadmap reescrito duas vezes** (v2 → v2.1) incorporando duas rodadas de
  auditoria técnica sua: modelo de maturidade por estratégia, notional vs.
  risco esclarecido, realismo de execução, critérios de aprovação
  padronizados, governança de IA, Launch Radar expandido com checklist
  honesto, Apêndice A com o mapa das fases antigas, critérios numéricos
  propostos para a Fase 11.
- **LLM local gratuito (Ollama + llama3.2:3b)** — instalado, testado,
  integrado no News Reactor. Classifica direção/confiança de cada filing
  8-K novo, sem chave, sem custo, sem dado saindo da máquina.
- **Kill-switch** (Fase 12) — manual (`data/KILL`) e automático (drawdown
  diário > 5%), verificado a cada ciclo antes de qualquer execução nova.
- **Realismo de execução** — preenchimento parcial de ordem e falha de
  segunda perna em arbitragem agora simulados (não é a lista completa da
  Seção 3 do roadmap, mas são os dois itens de maior impacto).
- **Multi-Asset (Fase 8)** — `MultiAssetSource` completo, conectando à
  Alpaca Markets (dado real IEX, gratuito). Só falta a chave — ver Seção 3
  abaixo.
- **Whale Watch / Launch Radar DEX** — URL de RPC agora configurável via
  variável de ambiente, para trocar do nó público gratuito para Alchemy
  sem mudar código.

## 2. Estado real do sistema agora

```
equity:            US$ 193,31
reserva protegida: US$ 0,47
patrimônio total:  US$ 193,78
ciclos:            2095
vitórias/derrotas: 939 / 1156
leg_size:          US$ 1,00 (no piso, por drawdown — motor de risco funcionando)
```

Achado honesto que se repete: **arbitragem acerta 56,6% das operações e
ainda assim perde dinheiro no acumulado** (perdas maiores que ganhos em
média) — exatamente o padrão que a Seção 3.1 do roadmap existe para
capturar. Nenhuma estratégia está perto de justificar capital real ainda.

## 3. O que NÃO foi — e não podia ser — terminado nesta sessão

Três categorias que nenhuma quantidade de autonomia muda:

1. **Passagem de tempo real.** A Fase 11 exige semanas consecutivas de
   paper trading. Não existe atalho de engenharia para isso.
2. **Contas que só você pode criar.** Alpaca (Multi-Asset) e Alchemy
   (Whale Watch em escala) — o código está pronto dos dois lados, falta
   só você criar a conta gratuita e colar a chave em uma variável de
   ambiente. Criar contas em seu nome está fora do que posso fazer, por
   regra de segurança permanente, não por limitação técnica.
3. **Capital real.** Fase 12 continua bloqueada. Nunca vou conectar
   dinheiro real sozinho, mesmo com autonomia total concedida.

Gaps menores, documentados no roadmap mas não implementados por escolha de
escopo (não por esquecimento): checklist completo de Launch Radar (holders,
honeypot, deployer), rotulagem de carteiras do Whale Watch, itens restantes
de realismo de execução (fila maker, rate limits, clock drift).

## 4. Arquivos-chave desta sessão

- [AurumOS_Roadmap.pdf](AurumOS_Roadmap.pdf) — v2.1, 17 páginas
- [docs/generate_roadmap.py](docs/generate_roadmap.py) — gerador do PDF (reexecute após qualquer mudança de estado)
- [backtests/live_performance_report.py](backtests/live_performance_report.py) e [pump_exhaustion_historical.py](backtests/pump_exhaustion_historical.py)
- `orchestrator/data/` — `portfolio_state.json`, `events.jsonl`, `raw_*.jsonl`, `KILL` (kill-switch manual)

## 5. Próxima vez que abrir isto

1. `cd orchestrator && cargo run --release` retoma exatamente de onde parou (equity, histórico, tudo).
2. Dashboard em `http://127.0.0.1:7878`.
3. Para ativar Multi-Asset: crie a conta gratuita em alpaca.markets, exporte `ALPACA_API_KEY_ID`/`ALPACA_API_SECRET_KEY`.
4. Para escalar Whale Watch: crie a conta gratuita na Alchemy, exporte `AURUMOS_ETH_WS_URL`/`AURUMOS_ETH_HTTP_URL`.
5. Para pausar tudo a qualquer momento: crie o arquivo `orchestrator/data/KILL` (vazio). Apague para retomar.

## 6. Addendum — análise de lucro acelerado (12/08/2026, sessão 2)

Máquina reiniciou (shutdown agendado da sessão anterior) — estado recuperado
normalmente do disco, sem perda. Pedido: analisar os relatórios coletados e
achar o que melhora o lucro.

**Causa raiz encontrada da perda líquida:** o próprio modelo de confiança
usado na simulação (`confidence = 0.5 + edge×25` para arbitragem, `0.45`
fixo para order flow) já implica um ponto de breakeven matemático — abaixo
dele, o valor esperado é negativo mesmo que o sinal "pareça" positivo.
Os limiares antigos (`MIN_NET_EDGE`) estavam muito abaixo desse breakeven:
0,06% vs. breakeven real de ~0,544% (arbitragem); 0,04% vs. ~0,367% (order
flow). Ou seja: o sistema estava, pelo seu próprio cálculo interno, indo
atrás de operações que já sabia serem ruins em média. Isso explica
exatamente o padrão visto: 56%+ de acerto em arbitragem e ainda assim
prejuízo líquido.

**Corrigido:** `MIN_NET_EDGE` elevado para 0,65% (arbitragem) e 0,45%
(order flow) — acima do breakeven, com margem de segurança. Ferramenta nova:
[backtests/edge_threshold_analysis.py](backtests/edge_threshold_analysis.py).

**Leitura honesta do resultado esperado:** o volume de operações dessas
duas estratégias deve cair muito (a maior parte dos cruzamentos de spread
observados no book real fica abaixo dos novos limiares). Isso não é um
defeito da correção — é o sinal real de que arbitragem/order-flow "crus"
nesses pares líquidos e populares (BTC, ETH, principais alts) provavelmente
não têm margem suficiente sobre custo de transação pra serem lucrativos:
é um mercado competido por firmas de alta frequência com infraestrutura que
este sistema não tem. **O caminho de maior potencial de lucro acelerado
continua sendo as estratégias de assimetria de informação/tempo** (Whale
Watch, News, Launch Radar, Pump Exhaustion) — exatamente a visão original
do PDF, não arbitragem estatística pura.

**Também implementado nesta rodada:** concentração de holders (top1/top5%)
no Launch Radar/DEX, via `eth_getLogs` no mesmo RPC gratuito — sem precisar
de indexador pago, viável porque o token é recém-lançado (pouco histórico
de blocos). Fecha mais um item do checklist que a v1 do roadmap listava
como bloqueado.

## 7. Addendum — 3ª auditoria técnica externa (13/08/2026)

Revisão técnica externa apontou 10 pontos concretos mais um erro matemático
real. Todos endereçados — ver tabela completa na resposta ao usuário desta
sessão; resumo aqui:

**Erro matemático corrigido:** o breakeven de arbitragem citado na Seção 6
deste relatório (~0,544%) estava errado — o termo constante da equação
quadrática usava `-max_loss_pct` em vez de `-0,5×max_loss_pct`. Breakeven
real: **0,297%**. Verificado por álgebra simbólica (sympy). O limiar
`MIN_NET_EDGE=0,65%` já deployado continua válido (margem ainda maior sobre
o breakeven corrigido do que se pensava) — nenhuma mudança de config
necessária, só a matemática documentada estava errada.

**Kill-switch redesenhado:** de gatilho único (5% diário) para 4 camadas —
preventivo 1% (aviso, não bloqueia), rígido 2% diário, 4% semanal, 7% de
drawdown total desde o pico. Dashboard ganhou indicador visual de status
(pílula âmbar/vermelha).

**Escalonamento agora exige profit factor real** (≥1,1 numa janela móvel de
30 trades), não só ciclos + drawdown baixo.

**Fase 11 com critério por estratégia** em vez de piso único de 150
operações (arbitragem/order flow: 150; pump exhaustion: 20 — mesmo piso da
própria camada de confirmação de preço).

**`events.jsonl` mais robusto:** writer único protegido por mutex, arquivo
aberto uma vez — não depende mais só da atomicidade de append do SO.

**LP "queimado" separado de "travado":** endereço de queima (burn) e
contrato de locker de terceiros (Unicrypt, Team Finance) são coisas
diferentes — só o primeiro está implementado, agora documentado sem ambiguidade.

**Multi-Asset com escopo corrigido:** documentação dizia "ativo" de forma
ampla; na prática só cobre 6 ações/ETFs dos EUA via Alpaca — forex,
dólar e índices futuros não têm fonte nem implementação ainda.

**Bug real encontrado ao restart (não fazia parte da auditoria):** o
`.env` com as chaves da Alpaca nunca era carregado quando o binário rodava
a partir da raiz do workspace — `dotenvy::dotenv()` busca a partir do
diretório de trabalho do processo, não do diretório do crate, e não subia
para dentro de `orchestrator/`. Corrigido para caminho absoluto via
`CARGO_MANIFEST_DIR`, mesmo padrão já usado pros caminhos de dado.
Confirmado ao vivo: Multi-Asset agora está genuinamente conectado à Alpaca
pela primeira vez desde que a chave foi configurada.

**Permanece em aberto, honestamente:** os 35%/3% de fill parcial/falha de
segunda perna na simulação continuam placeholders arbitrários, não
calibrados contra dado real de latência/book/rejeição — calibrar isso
exigiria execução real ou uma fonte de dado institucional, nenhuma das duas
disponível nesta fase. O PDF agora diz isso explicitamente, sem meias
palavras.

Recomendação de capital do revisor (US$100 Bybit + US$100 Bitget, real
desativado, cross-exchange só em shadow/paper) registrada no roadmap como
recebida — decisão de quando/se seguir continua do usuário.

---

O processo do orquestrador continua rodando (paper trading, dado real, sem
dinheiro real conectado).
