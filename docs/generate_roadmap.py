# -*- coding: utf-8 -*-
"""
Gera AurumOS_Roadmap.pdf a partir de conteudo estruturado abaixo, com a
identidade visual oficial da marca (AurumOS-Brand-Kit): paleta ouro/carvao/
ciano, tipografia Space Grotesk/Inter/JetBrains Mono, logo, sumario com
numeros de pagina e um diagrama de arquitetura.

Uso: python generate_roadmap.py
"""
import os

from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT, TA_CENTER
from reportlab.lib.pagesizes import LETTER
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas as canvas_module
from reportlab.platypus import (
    BaseDocTemplate, Frame, PageTemplate, Paragraph, Spacer, Table,
    TableStyle, PageBreak, ListFlowable, ListItem,
)
from reportlab.platypus.tableofcontents import TableOfContents
from reportlab.graphics.shapes import Drawing, Rect, String, Line, Group, Path
from reportlab.graphics import renderPDF
from svglib.svglib import svg2rlg

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT_PATH = os.path.join(ROOT, "AurumOS_Roadmap.pdf")

# ---------------------------------------------------------------- paleta ---
GOLD = colors.HexColor("#B87916")      # gold-700, contraste AA em fundo branco
GOLD_BG = colors.HexColor("#F5C453")   # gold-500, uso em blocos/realces
GOLD_SOFT = colors.HexColor("#FBEBC7")
INK = colors.HexColor("#0C1117")       # ink-900, texto principal
INK_HEADER = colors.HexColor("#151C24")  # ink-800, fundo de cabecalho de tabela
CYAN = colors.HexColor("#0E7C90")      # cyan escurecido p/ contraste em texto
CYAN_BG = colors.HexColor("#18D7FF")
SLATE = colors.HexColor("#5B6673")
WHITE = colors.HexColor("#FFFFFF")
RISK_RED = colors.HexColor("#C23A50")
POSITIVE = colors.HexColor("#1C9E76")
PAPER = colors.HexColor("#FFFFFF")

# ------------------------------------------------------------- tipografia --
pdfmetrics.registerFont(TTFont("SpaceGrotesk", os.path.join(HERE, "fonts", "SpaceGrotesk.ttf")))
pdfmetrics.registerFont(TTFont("Inter", os.path.join(HERE, "fonts", "Inter.ttf")))
pdfmetrics.registerFont(TTFont("JetBrainsMono", os.path.join(HERE, "fonts", "JetBrainsMono.ttf")))

styles = {
    "Title": ParagraphStyle("Title", fontName="SpaceGrotesk", fontSize=30, leading=34, textColor=INK, spaceAfter=6),
    "Subtitle": ParagraphStyle("Subtitle", fontName="Inter", fontSize=12.5, leading=17, textColor=SLATE, spaceAfter=4),
    "H1": ParagraphStyle("H1", fontName="SpaceGrotesk", fontSize=17, leading=21, textColor=INK, spaceBefore=18, spaceAfter=8),
    "H2": ParagraphStyle("H2", fontName="SpaceGrotesk", fontSize=12.5, leading=16, textColor=GOLD, spaceBefore=12, spaceAfter=6),
    "Body": ParagraphStyle("Body", fontName="Inter", fontSize=9.6, leading=14, textColor=INK, spaceAfter=6, alignment=TA_LEFT),
    "BodySmall": ParagraphStyle("BodySmall", fontName="Inter", fontSize=8.6, leading=12.5, textColor=SLATE, spaceAfter=4),
    "Callout": ParagraphStyle("Callout", fontName="Inter", fontSize=9.4, leading=13.5, textColor=INK, spaceAfter=4),
    "TableHeader": ParagraphStyle("TableHeader", fontName="SpaceGrotesk", fontSize=8.6, leading=11, textColor=WHITE),
    "TableCell": ParagraphStyle("TableCell", fontName="Inter", fontSize=8.6, leading=11.5, textColor=INK),
    "TableCellMono": ParagraphStyle("TableCellMono", fontName="JetBrainsMono", fontSize=8.2, leading=11, textColor=INK),
    "TOCHeading": ParagraphStyle("TOCHeading", fontName="SpaceGrotesk", fontSize=10.5, leading=15, textColor=INK),
    "TOCSub": ParagraphStyle("TOCSub", fontName="Inter", fontSize=9.5, leading=13.5, textColor=SLATE, leftIndent=12),
    "Mono": ParagraphStyle("Mono", fontName="JetBrainsMono", fontSize=9, leading=13, textColor=INK),
    "TagOK": ParagraphStyle("TagOK", fontName="Inter", fontSize=8, leading=11, textColor=POSITIVE),
    "TagGap": ParagraphStyle("TagGap", fontName="Inter", fontSize=8, leading=11, textColor=RISK_RED),
}


def P(text, style="Body"):
    return Paragraph(text, styles[style])


def bullets(items, style="Body", bullet_color=GOLD):
    return ListFlowable(
        [ListItem(P(i, style), bulletColor=bullet_color) for i in items],
        bulletType="bullet", start="circle", leftIndent=14, bulletFontSize=6, spaceBefore=2, spaceAfter=6,
    )


def section_table(rows, col_widths, header=True):
    data = []
    for r_idx, row in enumerate(rows):
        if header and r_idx == 0:
            data.append([Paragraph(c, styles["TableHeader"]) for c in row])
        else:
            data.append([Paragraph(c, styles["TableCell"]) if not c.startswith("§MONO§") else
                         Paragraph(c[6:], styles["TableCellMono"]) for c in row])
    t = Table(data, colWidths=col_widths, repeatRows=1 if header else 0)
    style_cmds = [
        ("GRID", (0, 0), (-1, -1), 0.5, colors.HexColor("#E3E7EC")),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("LEFTPADDING", (0, 0), (-1, -1), 6),
        ("RIGHTPADDING", (0, 0), (-1, -1), 6),
        ("TOPPADDING", (0, 0), (-1, -1), 5),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]
    if header:
        style_cmds += [
            ("BACKGROUND", (0, 0), (-1, 0), INK_HEADER),
            ("ROWBACKGROUNDS", (0, 1), (-1, -1), [WHITE, colors.HexColor("#F7F8FA")]),
        ]
    t.setStyle(TableStyle(style_cmds))
    return t


def phase_banner(title, status_text, status_color):
    inner = Table(
        [[Paragraph(f'<b>{title}</b>', ParagraphStyle("pt", fontName="SpaceGrotesk", fontSize=11, textColor=WHITE)),
          Paragraph(status_text, ParagraphStyle("ps", fontName="Inter", fontSize=8.3, textColor=WHITE, alignment=TA_CENTER))]],
        colWidths=[380, 110],
    )
    inner.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (0, 0), INK_HEADER),
        ("BACKGROUND", (1, 0), (1, 0), status_color),
        ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("LEFTPADDING", (0, 0), (0, 0), 10),
        ("TOPPADDING", (0, 0), (-1, -1), 7),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
        ("ALIGN", (1, 0), (1, 0), "CENTER"),
    ]))
    return inner


def callout_box(text, border_color=GOLD, bg=GOLD_SOFT):
    t = Table([[P(text, "Callout")]], colWidths=[490])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), bg),
        ("BOX", (0, 0), (-1, -1), 1, border_color),
        ("LEFTPADDING", (0, 0), (-1, -1), 10),
        ("RIGHTPADDING", (0, 0), (-1, -1), 10),
        ("TOPPADDING", (0, 0), (-1, -1), 8),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 8),
    ]))
    return t


def load_brand_mark():
    """Carrega marks/aurumos-mark.svg (o simbolo geometrico oficial, sem o
    texto 'AurumOS') via svglib. svglib nao resolve o gradiente dourado do
    SVG original, entao recolorimos manualmente: paths so-preenchimento
    viram ouro solido, o path so-contorno mantem o ciano que ja veio certo."""
    mark_path = os.path.join(HERE, "brand", "aurumos-mark.svg")
    drawing = svg2rlg(mark_path)

    def recolor(node):
        if isinstance(node, Path):
            if node.strokeColor is not None:
                node.strokeColor = CYAN_BG
            else:
                node.fillColor = GOLD_BG
        if isinstance(node, Group):
            for child in node.contents:
                recolor(child)

    recolor(drawing)
    return drawing


def architecture_diagram():
    d = Drawing(490, 150)
    boxes = [
        (10, 80, 150, 55, "Execucao / Orquestrador", "Rust", GOLD),
        (170, 80, 150, 55, "Pesquisa / Sinais / ML", "Python", CYAN),
        (330, 80, 150, 55, "Dashboard / Observabilidade", "TypeScript", GOLD),
    ]
    for x, y, w, h, label, sub, accent in boxes:
        d.add(Rect(x, y, w, h, rx=8, ry=8, fillColor=colors.HexColor("#F7F8FA"), strokeColor=accent, strokeWidth=1.6))
        d.add(String(x + w / 2, y + 32, label, fontName="SpaceGrotesk", fontSize=8.6, fillColor=INK, textAnchor="middle"))
        d.add(String(x + w / 2, y + 16, sub, fontName="JetBrainsMono", fontSize=8, fillColor=accent, textAnchor="middle"))
    # setas horizontais entre as 3 camadas
    for x0, x1 in [(160, 170), (320, 330)]:
        d.add(Line(x0, 107, x1, 107, strokeColor=SLATE, strokeWidth=1.4))
    d.add(String(245, 60, "Opportunity (canal assincrono) → scorer + risk engine → execucao → eventos", fontName="Inter", fontSize=7.6, fillColor=SLATE, textAnchor="middle"))
    d.add(String(245, 15, "score = (vantagem_liquida × confianca) ÷ risco_de_cauda ÷ capital_necessario", fontName="JetBrainsMono", fontSize=7.6, fillColor=GOLD, textAnchor="middle"))
    return d


def maturity_ladder_diagram():
    d = Drawing(490, 70)
    stages = ["Pesquisa", "Backtest / Replay", "Paper Trading", "Shadow-live", "Micro-capital", "Escalonamento"]
    n = len(stages)
    box_w = 78
    gap = 4
    total_w = n * box_w + (n - 1) * gap
    x0 = (490 - total_w) / 2
    for i, s in enumerate(stages):
        x = x0 + i * (box_w + gap)
        fill = GOLD_SOFT if i <= 1 else colors.HexColor("#F7F8FA")
        d.add(Rect(x, 25, box_w, 30, rx=5, ry=5, fillColor=fill, strokeColor=GOLD if i <= 1 else SLATE, strokeWidth=1.2))
        d.add(String(x + box_w / 2, 36, s, fontName="Inter", fontSize=6.6, fillColor=INK, textAnchor="middle"))
        if i < n - 1:
            d.add(Line(x + box_w, 40, x + box_w + gap, 40, strokeColor=SLATE, strokeWidth=1.2))
    return d


# ------------------------------------------------------- doc + TOC + pagina
class RoadmapDoc(BaseDocTemplate):
    def afterFlowable(self, flowable):
        if isinstance(flowable, Paragraph):
            style_name = flowable.style.name
            text = flowable.getPlainText()
            if text == "Sumario":
                return
            if style_name == "H1":
                self.notify("TOCEntry", (0, text, self.page))
                key = f"h1-{self.page}-{abs(hash(text)) % 10000}"
                self.canv.bookmarkPage(key)
                self.canv.addOutlineEntry(text, key, level=0, closed=False)
            elif style_name == "H2":
                self.notify("TOCEntry", (1, text, self.page))


def draw_page_frame(canv, doc):
    canv.saveState()
    page_w, page_h = LETTER
    # barra superior fina
    canv.setFillColor(GOLD)
    canv.rect(0, page_h - 4, page_w, 4, fill=1, stroke=0)
    # cabecalho
    canv.setFont("SpaceGrotesk", 8.5)
    canv.setFillColor(SLATE)
    canv.drawString(20 * mm, page_h - 14 * mm, "AurumOS")
    canv.setFont("Inter", 7.5)
    canv.drawRightString(page_w - 20 * mm, page_h - 14 * mm, "Roadmap tecnico — orquestrador multiativo")
    canv.setStrokeColor(colors.HexColor("#E3E7EC"))
    canv.line(20 * mm, page_h - 16 * mm, page_w - 20 * mm, page_h - 16 * mm)
    # rodape com numero de pagina
    canv.setFont("JetBrainsMono", 8)
    canv.setFillColor(SLATE)
    canv.drawCentredString(page_w / 2, 12 * mm, f"{canv.getPageNumber()}")
    canv.setStrokeColor(colors.HexColor("#E3E7EC"))
    canv.line(20 * mm, 16 * mm, page_w - 20 * mm, 16 * mm)
    canv.restoreState()


def draw_cover(canv, doc):
    canv.saveState()
    page_w, page_h = LETTER
    canv.setFillColor(INK)
    canv.rect(0, 0, page_w, page_h, fill=1, stroke=0)
    # faixa dourada
    canv.setFillColor(GOLD_BG)
    canv.rect(0, page_h - 10, page_w, 10, fill=1, stroke=0)
    canv.setFillColor(CYAN_BG)
    canv.rect(0, 0, page_w, 4, fill=1, stroke=0)

    mark = load_brand_mark()
    mark_size = 22 * mm
    scale = mark_size / mark.width
    canv.saveState()
    canv.translate(24 * mm, page_h - 68 * mm)
    canv.scale(scale, scale)
    renderPDF.draw(mark, canv, 0, 0)
    canv.restoreState()

    canv.setFont("SpaceGrotesk", 46)
    canv.setFillColor(GOLD_BG)
    canv.drawString(24 * mm, page_h - 80 * mm, "Aurum")
    w = canv.stringWidth("Aurum", "SpaceGrotesk", 46)
    canv.setFillColor(CYAN_BG)
    canv.drawString(24 * mm + w, page_h - 80 * mm, "OS")

    canv.setFont("Inter", 13)
    canv.setFillColor(colors.HexColor("#C6CDD6"))
    canv.drawString(24 * mm, page_h - 92 * mm, "Roadmap tecnico completo — orquestrador multiativo de oportunidades")
    canv.drawString(24 * mm, page_h - 99 * mm, "(cripto, acoes, forex, indices)")

    canv.setFont("JetBrainsMono", 9.5)
    canv.setFillColor(GOLD_BG)
    canv.drawString(24 * mm, page_h - 115 * mm, "v3.0 — revisado apos quatro auditorias tecnicas, 11-13/08/2026")

    canv.setFont("Inter", 8.5)
    canv.setFillColor(colors.HexColor("#8B96A3"))
    canv.drawString(24 * mm, 20 * mm, "Documento de referencia interno — paper trading, sem dinheiro real conectado")
    canv.restoreState()


# ------------------------------------------------------------------- build
def build():
    doc = RoadmapDoc(
        OUT_PATH,
        pagesize=LETTER,
        leftMargin=20 * mm, rightMargin=20 * mm, topMargin=20 * mm, bottomMargin=20 * mm,
        title="AurumOS Roadmap",
    )
    frame_cover = Frame(0, 0, LETTER[0], LETTER[1], id="cover")
    frame_content = Frame(20 * mm, 18 * mm, LETTER[0] - 40 * mm, LETTER[1] - 38 * mm, id="content")
    doc.addPageTemplates([
        PageTemplate(id="Cover", frames=[frame_cover], onPage=draw_cover),
        PageTemplate(id="Content", frames=[frame_content], onPage=draw_page_frame),
    ])

    from reportlab.platypus.doctemplate import NextPageTemplate
    story = [NextPageTemplate("Content"), PageBreak()]

    # sumario
    story.append(P("Sumario", "H1"))
    toc = TableOfContents()
    toc.levelStyles = [styles["TOCHeading"], styles["TOCSub"]]
    story.append(toc)
    story.append(PageBreak())

    # ---------------------------------------------------- metadados / capa2
    story.append(P("AurumOS — Roadmap Tecnico", "Title"))
    story.append(P("Orquestrador multiativo de oportunidades: cripto, acoes, forex e indices, coordenados por um nucleo central de risco.", "Subtitle"))
    story.append(Spacer(1, 8))
    story.append(section_table([
        ["Campo", "Valor"],
        ["Data do documento", "13 de agosto de 2026 (v3.0 — historico de revisoes na Secao 0 abaixo)"],
        ["Capital inicial de referencia", "US$ 200 (US$100 Bybit + US$100 Bitget) — ver decisao pendente na Secao 8"],
        ["Status atual", "Fase 0 concluida; Fase 1/2 em paper trading com dado real; Fase 10 (backtest formal) iniciada em paralelo"],
        ["Modo de operacao", "Paper trading — sem dinheiro real conectado"],
        ["Regra inegociavel", "O assistente nunca ativa nem conecta capital real sozinho — ver Secao 10"],
    ], [160, 330]))
    story.append(Spacer(1, 10))
    story.append(P("Historico de revisoes", "H2"))
    story.append(section_table([
        ["Versao", "Data", "O que mudou"],
        ["v1", "10/08/2026", "Roadmap original — fases sequenciais, sem auditoria tecnica ainda."],
        ["v2", "11/08/2026", "1a auditoria: corrige descricoes desatualizadas (escalonamento por "
         "degraus ja implementado, texto ainda descrevia ideia descartada), notional vs. risco, "
         "checklist real do Launch Radar, secoes de realismo de execucao/governanca de IA/criterios "
         "padronizados, modelo de maturidade por estrategia, identidade visual oficial."],
        ["v2.1", "12/08/2026", "2a auditoria: modelo de maturidade refinado, notas de risco "
         "adicionais, Apendice A com mapa historico das fases."],
        ["v3.0 (esta)", "13/08/2026", "3a e 4a auditorias, combinadas: correcao do erro matematico "
         "do breakeven, kill-switch em 4 camadas (depois endurecido: 1,5% diario, trava manual no "
         "nivel total), separacao burn/locker, calibracao do Pump Exhaustion por percentil real "
         "(depois corrigida pra evitar contar o mesmo evento varias vezes), escalonamento mais "
         "rigoroso (PF>=1.2 + robustez a outlier), Fase 11 com exigencia de distribuicao temporal, "
         "universo de simbolos ampliado, cockpit de risco no dashboard, e esta consolidacao de "
         "contradicoes internas que se acumularam entre v2/v2.1/v3.0."],
    ], [70, 60, 350]))
    story.append(Spacer(1, 6))
    story.append(callout_box(
        "Nota da propria auditoria que motivou esta tabela: seções anteriores (1.3, 5, 6) ainda "
        "traziam descrições da v2 que uma seção 12.x posterior já havia corrigido — a v3.0 "
        "consolida essas contradições diretamente nas seções originais, em vez de deixar a correção "
        "só no final. Onde uma seção antiga ainda é citada por completude histórica, isso está "
        "marcado explicitamente no texto.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(PageBreak())

    # ---------------------------------------------------------------- Sec0
    story.append(P("0. Regras que nao mudam, independente da fase", "H1"))
    story.append(callout_box(
        "Estas regras valem para todas as fases e nao podem ser puladas para 'acelerar'. Elas existem "
        "porque o objetivo e capital real, e capital real nao tolera atalhos.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(Spacer(1, 6))
    story.append(bullets([
        "<b>Nenhuma execucao automatica com dinheiro real iniciada pelo assistente.</b> O codigo pode ser escrito e testado ate o ponto de enviar ordens, mas a conexao com chaves de API reais e o disparo da primeira ordem real e sempre um ato manual do usuario. Redacao completa desta regra na Secao 10.",
        "<b>Nenhuma fase avanca sem paper trading validado.</b> Cada modulo novo entra primeiro em simulacao (dados reais, execucao simulada) antes de cotar capital de verdade.",
        "<b>Sem martingale, sem aumento de aposta apos perda.</b> Implementado no motor de risco (bloqueio automatico em <font face='JetBrainsMono'>risk.rs::evaluate</font>).",
        "<b>Sem alavancagem em lancamentos de tokens.</b> Livro de ofertas nao confiavel na abertura — risco de gap/slippage descontrolado.",
        "<b>Limite de risco simultaneo total de 0,50% do capital</b>, dividido por estrategia, com trava adicional por grupo de correlacao (nunca duas apostas que sao, na pratica, a mesma aposta).",
        "<b>Toda 'oportunidade de ouro' (pump, baleia, noticia) e tratada como hipotese, nao fato.</b> Precisa de confirmacao de preco/livro/volume antes de virar ordem — nenhum sinal isolado dispara execucao sozinho. E por isso que Whale Watch, News, Macro, Launch Radar, Pump Exhaustion e Liquidation Hunter emitem <font face='JetBrainsMono'>net_edge = 0.0</font> ate terem uma camada de confirmacao com vantagem numerica validada por backtest.",
    ]))

    # ---------------------------------------------------------------- Sec1
    story.append(P("1. Arquitetura geral", "H1"))
    story.append(P(
        "O AurumOS e dividido em tres camadas que rodam com linguagens diferentes, escolhidas pela "
        "exigencia de cada uma — nao existe uma linguagem unica 'melhor' para o sistema inteiro.", "Body"))
    story.append(architecture_diagram())
    story.append(Spacer(1, 6))
    story.append(P("1.1 Fluxo de dados", "H2"))
    story.append(P(
        "Modulos de dados (Rust, hoje — Python nas fases de pesquisa/ML mais pesadas) emitem "
        "<font face='JetBrainsMono'>Opportunity</font> em formato estruturado atraves de um canal "
        "assincrono. O Orquestrador aplica scorer + risk engine, escolhe as N melhores oportunidades "
        "aprovadas por ciclo — N escala com o capital total, direto de "
        "<font face='JetBrainsMono'>orchestrator.rs::max_concurrent_trades</font> — executa (real ou "
        "simulado), atualiza o <font face='JetBrainsMono'>PortfolioState</font> e publica tudo num "
        "barramento de eventos persistido em disco, consumido ao vivo pelo dashboard.", "Body"))
    story.append(Spacer(1, 4))
    story.append(section_table([
        ["Patrimonio total", "Operacoes simultaneas permitidas"],
        ["ate US$ 499", "§MONO§1"],
        ["US$ 500 – 999", "§MONO§2"],
        ["US$ 1.000 – 2.499", "§MONO§3"],
        ["US$ 2.500 ou mais", "§MONO§5"],
    ], [245, 245]))
    story.append(Spacer(1, 4))
    story.append(P(
        "Mais concorrencia nao significa mais risco por si so: cada oportunidade ainda passa pelo "
        "mesmo limite por estrategia/grupo de correlacao/total da Secao 9 antes de ser aprovada — o "
        "que muda e quantas oportunidades DIFERENTES podem virar ordem no mesmo ciclo em vez de "
        "expirar esperando a vez.", "BodySmall"))

    story.append(P("1.2 Notional vs. risco — esclarecimento explicito", "H2"))
    story.append(P(
        "Um ponto de confusao real na v1 deste documento: <font face='JetBrainsMono'>leg_size</font> "
        "(hoje US$25 iniciais) e o <b>notional</b> — o tamanho da ordem. O limite de 0,35% para "
        "arbitragem, por exemplo, e um limite de <b>risco</b> — a pior perda estimada, nao o tamanho "
        "da posicao. O codigo ja separa os dois corretamente:", "Body"))
    story.append(callout_box(
        "capital_at_risk = order_size &#215; opp.max_loss_pct<br/>"
        "strategy_limit = strategy_risk_pct &#215; equity<br/>"
        "aprovado apenas se: exposicao_atual + capital_at_risk &#8804; strategy_limit",
        border_color=CYAN, bg=colors.HexColor("#E7F8FC"),
    ))
    story.append(Spacer(1, 4))
    story.append(P(
        "Ou seja: US$25 de notional com <font face='JetBrainsMono'>max_loss_pct</font> de 2% "
        "representa US$0,50 de risco estimado (0,25% de US$200) — dentro do limite de 0,35% da "
        "estrategia, nao 12,5%. <b>Gap real ainda aberto:</b> para arbitragem, o "
        "<font face='JetBrainsMono'>max_loss_pct</font> hoje e um valor generico por oportunidade; "
        "ele precisa passar a modelar explicitamente falha da segunda perna do hedge e divergencia "
        "de spread entre as duas exchanges durante a janela de execucao, nao so um stop tradicional "
        "de posicao unica. Isso fica registrado como item da Secao 3 (Realismo de execucao).", "Body"))

    story.append(P("1.3 Coleta de dados continua", "H2"))
    story.append(P(
        "Ate a v2, o sistema so persistia o que ja tinha cruzado um limiar de decisao — o estado "
        "'normal', sem sinal, nao ficava registrado em lugar nenhum, e recalibrar qualquer limiar "
        "exigia rebuscar dado historico externo do zero. Corrigido nesta revisao: Arbitragem, Order "
        "Flow e Pump Exhaustion agora gravam um snapshot bruto do estado inteiro (spread comparado das "
        "duas exchanges, book, funding, open interest — nao so o que disparou algo) a cada 60 segundos, "
        "em <font face='JetBrainsMono'>data/raw_arbitrage.jsonl</font>, "
        "<font face='JetBrainsMono'>data/raw_order_flow.jsonl</font> e "
        "<font face='JetBrainsMono'>data/raw_pump_exhaustion.jsonl</font>. E esse log bruto que tornou "
        "possivel a analise de breakeven da Secao 7.1 e a calibracao do Pump Exhaustion sem precisar "
        "buscar dado externo de novo.", "Body"))
    story.append(callout_box(
        "<b>Bug real corrigido no caminho:</b> a escrita do log de eventos (events.jsonl) fazia duas "
        "chamadas de escrita separadas por evento (conteudo, depois quebra de linha) sem lock — quando "
        "dois modulos emitiam eventos ao mesmo tempo, as vezes duas linhas JSON colavam sem separador, "
        "corrompendo aquela linha. Achado ao construir a ferramenta de backtest (linhas corrompidas "
        "apareciam como erro de parse). Corrigido em <font face='JetBrainsMono'>events.rs</font> "
        "montando a linha inteira (conteudo + quebra) antes de uma unica chamada de escrita. Linhas "
        "antigas ja corrompidas (de antes do fix) continuam sendo puladas silenciosamente pelo "
        "carregador, como sempre. <b>Nota: este era o estado do fix na v2 — reforcado depois (Secao "
        "12.6) pra writer unico com mutex, sem depender da atomicidade de write() do SO.</b>",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
    ))

    story.append(PageBreak())

    # ------------------------------------------------------------- Sec2 (maturidade)
    story.append(P("2. Modelo de maturidade por estrategia", "H1"))
    story.append(P(
        "A v1 deste roadmap organizava tudo em fases estritamente sequenciais (Fase 1 -> Fase 2 -> "
        "... -> Fase 12), o que sugeria — incorretamente — que nenhuma estrategia poderia chegar perto "
        "de capital real ate o sistema inteiro estar pronto. Isso nao combina com o objetivo de "
        "acelerar sem cortar caminho na seguranca. A partir desta revisao, cada estrategia avanca "
        "numa escada de maturidade <b>independente</b>:", "Body"))
    story.append(maturity_ladder_diagram())
    story.append(Spacer(1, 6))
    story.append(P(
        "Uma arbitragem comprovadamente positiva pode, em tese, comecar com microcapital sem esperar "
        "o Whale Watch ou o Multi-Asset existirem — desde que ela mesma tenha passado por replay/"
        "backtest formal, paper trading validado e (futuramente) shadow-live. As fases numeradas da "
        "v1 (Fase 0 a Fase 12) nao aparecem mais como estrutura principal deste documento — ficam "
        "resumidas no Apendice A, so como registro do que foi construido e em que ordem. O "
        "<b>criterio de avanco para capital real passa a ser por estrategia, nao por fase global</b>.", "Body"))
    story.append(Spacer(1, 6))
    story.append(section_table([
        ["Estrategia", "Estagio atual", "Evidencia"],
        ["Arbitragem", "Paper (dado real) — limiar corrigido p/ breakeven, sem operar ainda", "222 operacoes antes da correcao (PnL -US$3,96); 0 desde a correcao — spreads >0,65% sao raros (Secao 7.1)"],
        ["Order Flow", "Paper (dado real) — resultado pos-correcao positivo, amostra ainda concentrada numa unica semana", "profit factor >1,5 pos-correcao (ver fase11_progress.py — falta distribuicao em mais semanas, Secao 12.9); 130 operacoes pre-correcao com PnL -US$0,21 (Secao 7)"],
        ["Pump Exhaustion", "Paper com confirmacao de preco (Secao 2.1) — amostra insuficiente", "scoring com 3 dimensoes ponderadas; net_edge passa a ser real apos 20 confirmacoes medidas, ainda 0 ate la"],
        ["Whale Watch", "Observacao", "net_edge=0 por design; 11 enderecos de exchange rotulados e verificados individualmente (Secao 12.2)"],
        ["News Reactor", "Observacao — classificacao real ativa", "net_edge=0 por design (falta confirmacao de preco); classificador via Ollama local (Secao 12.1) gerando direction/confidence reais"],
        ["Macro Engine", "Pesquisa/Observacao", "calendario real carregado; sem modelo de reacao ainda"],
        ["Launch Radar (CEX+DEX)", "Observacao — checklist parcial", "cobre EVM/Uniswap V2, holders, LP queimado, candidato a sybil; gaps documentados na Secao 4"],
        ["Liquidation Hunter", "Observacao", "net_edge=0 por design"],
        ["Multi-Asset Volatility", "Ativa — observacao", "conectada a Alpaca (dado real IEX); net_edge=0 por design, sem estrategia/correlacao cross-asset ainda (Secao 12.3)"],
    ], [110, 190, 190]))

    story.append(P("2.1 Primeira camada de confirmacao de preco implementada", "H2"))
    story.append(P(
        "Ate aqui, todo modulo de evento saia sempre com net_edge=0.0 — nenhum tinha a 'camada de "
        "confirmacao de preco' que a Secao 0 exige antes de um sinal virar vantagem numerica de "
        "verdade. Pump Exhaustion e o primeiro a ter isso implementado: quando um candidato dispara, o "
        "modulo registra o preco de entrada e, 20 minutos depois, confere se o preco realmente caiu "
        "(a hipotese de exaustao) usando o mesmo feed de ticker ja em uso — sem chamada extra. Esse "
        "desfecho alimenta um histórico continuo (ate 500 amostras). So depois de 20 confirmacoes "
        "reais o modulo passa a emitir net_edge > 0 — e mesmo assim, calculado do proprio historico "
        "(retorno medio de uma posicao short = variacao de preco observada &#215; -1), nunca um numero "
        "escolhido a dedo. Se o historico mostrar que o padrao nao tem vantagem real, net_edge "
        "continua 0 — o modulo nao forca positividade.", "Body"))
    story.append(callout_box(
        "Isso muda o comportamento do sistema: pela primeira vez, um modulo de evento pode passar a "
        "efetivamente operar (ainda 100% paper trading) em vez de só observar. É a mudança de maior "
        "potencial de lucro identificada nesta revisão — e a de maior risco, porque é a primeira vez "
        "que a 'vantagem numérica' de um sinal de evento vem de dado medido em vez de ficar travada em "
        "zero. O mesmo padrão fica pronto pra ser replicado nos outros módulos de evento (Whale Watch, "
        "News, Launch Radar) — não feito ainda nesta revisão, por escopo.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(P("2.2 Limiares de gatilho recalibrados por percentil real (13/08/2026)", "H2"))
    story.append(P(
        "Achado honesto ao investigar por que o Pump Exhaustion não emitia nenhum sinal: os limiares "
        "de \"extremo\" (funding &gt;0,10%/8h, pump 24h &gt;15%, crescimento de OI em 1h &gt;20%, pelo "
        "menos 2 de 3 simultâneos) foram herdados de uma estimativa inicial, nunca confrontados com "
        "dado real de mercado. Analisando os 16.994 snapshots já acumulados em "
        "<font face='JetBrainsMono'>raw_pump_exhaustion.jsonl</font> (~17h, 30 símbolos): pump 24h "
        "nunca passou de 7,5% no período, crescimento de OI nunca passou de 5,2% — os limiares exigiam "
        "um regime de mercado bem mais volátil do que o observado. Zero sinais em 17h não era o "
        "detector funcionando de forma seletiva, era o detector inoperante para o regime atual.", "Body"))
    story.append(P(
        "Corrigido com o percentil 90 real de cada métrica na própria distribuição observada — não "
        "chute, não arredondamento por sensação: funding &gt;0,015%/8h, pump 24h &gt;3%, crescimento "
        "de OI em 1h &gt;1%. Combinados (&#8805;2 de 3), isso ocorre em 0,35% dos snapshots observados "
        "(~59 vezes no período de referência) — raro o suficiente pra ainda ser um sinal seletivo, "
        "frequente o suficiente pra a camada de confirmação de preço (Seção 2.1, mínimo de 20 "
        "confirmações) conseguir acumular amostra em tempo razoável.", "Body"))
    story.append(callout_box(
        "Isto não é definitivo — é uma recalibração pontual contra o regime de mercado observado numa "
        "janela de ~17h. Precisa ser revisitado à medida que mais dado (e mais regimes de mercado, "
        "inclusive períodos de alta volatilidade) acumular. O comentário no código já previa isso: "
        "\"insumo pra recalibrar com dado próprio no futuro\" — este foi o primeiro uso real desse "
        "insumo.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(PageBreak())

    # --------------------------------------------------------- Sec3 (realismo)
    story.append(P("3. Realismo de execucao — pre-requisito para qualquer avanco de estagio", "H1"))
    story.append(P(
        "A Fase 1 ja considera taxa maker/taker e profundidade de livro, mas isso nao e suficiente "
        "para confiar no resultado do paper trading como preditor de resultado real. Nenhuma "
        "estrategia deve subir de estagio na escada da Secao 2 sem que a simulacao de execucao "
        "modele explicitamente:", "Body"))
    story.append(bullets([
        "Posicao na fila de ordens maker (nao assumir preenchimento imediato/instantaneo).",
        "Preenchimentos parciais — uma ordem pode fechar so uma fracao do notional pedido.",
        "Falha da segunda perna em arbitragem/hedge (perna 1 executa, perna 2 falha ou atrasa).",
        "Ordens rejeitadas pela exchange (tamanho minimo, precisao, saldo insuficiente).",
        "Livros dessincronizados entre exchanges no instante da decisao.",
        "Clock drift entre o relogio local e o servidor da exchange.",
        "Rate limits de API (decisao pode nao conseguir ser enviada a tempo).",
        "Funding (perpetuos) cobrado/pago durante o periodo em que a posicao fica aberta.",
        "Tamanho e precisao minimos por simbolo (lot size, tick size).",
        "Reconciliacao apos desconexao — o que o motor assume sobre uma ordem que estava em voo quando a conexao caiu.",
        "Slippage durante saida de emergencia (kill-switch, Secao 9) sob livro fino.",
    ]))
    story.append(callout_box(
        "Sem isso, o paper trading pode mostrar lucro que nao existiria no mercado real. Este bloco "
        "passa a ser parte do 'Framework de Backtesting Formal' (Fase 10) e da definicao de "
        "'validado' usada na escada de maturidade da Secao 2 — nenhuma estrategia e promovida de "
        "Paper para Shadow-live sem uma simulacao de execucao que cubra os itens acima.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))

    story.append(P("3.1 Criterios de aprovacao padronizados", "H2"))
    story.append(P(
        "Criterios frouxos como '55% das sessoes positivas' nao bastam — uma estrategia pode acertar "
        "80% das vezes e ainda perder dinheiro se as perdas forem maiores que os ganhos. A partir "
        "desta revisao, todo modulo usa o mesmo template de criterio de saida, documentado com numero "
        "exato (nunca estimativa):", "Body"))
    story.append(bullets([
        "Resultado liquido apos todos os custos (taxas, funding, slippage estimado).",
        "Expectativa matematica positiva (ganho medio &#215; taxa de acerto &#8722; perda media &#215; taxa de erro &gt; 0).",
        "Profit factor minimo (soma dos ganhos / soma das perdas).",
        "Drawdown maximo dentro do limite configurado.",
        "Expected shortfall / CVaR da cauda de perdas.",
        "Numero minimo de operacoes (amostra grande o bastante pra nao ser ruido — ver achado da Secao 7 sobre Pump Exhaustion).",
        "Desempenho fora da amostra (dado nao usado para calibrar o modulo).",
        "Teste de estresse com latencia e slippage maiores que o observado.",
        "Simulacao Monte Carlo da sequencia de operacoes (reordenar os trades e checar se o resultado depende da ordem em que vieram).",
        "Resultado nao dependente de uma unica operacao excepcional (remover o melhor trade e checar se o resultado ainda e positivo).",
    ]))

    story.append(PageBreak())

    # --------------------------------------------------------------- Sec4
    story.append(P("4. Launch Radar — cobertura atual vs. checklist completo", "H1"))
    story.append(P(
        "O objetivo inclui moedas recem-criadas antes mesmo de chegarem as grandes exchanges — o "
        "<font face='JetBrainsMono'>DexLaunchRadarSource</font> ja escuta criacao de pares novos na "
        "Uniswap V2 (Ethereum) em tempo real, nao so listagens em exchange centralizada. Cobertura "
        "real hoje, por chamada direta ao contrato (sem block explorer pago):", "Body"))
    story.append(section_table([
        ["Item do checklist", "Estado"],
        ["Criacao de pool detectada em tempo real (Uniswap V2 / Ethereum)", "§MONO§Implementado"],
        ["Heuristica de autoridade de mint (selector de funcao)", "§MONO§Implementado"],
        ["owner() do contrato (controle privilegiado ainda ativo?)", "§MONO§Implementado"],
        ["Liquidez inicial real via getReserves()", "§MONO§Implementado"],
        ["Concentracao de holders (top1/top5 via eth_getLogs, sem indexador pago)", "§MONO§Implementado"],
        ["Liquidez QUEIMADA (% do LP em endereco de queima — nao locker de terceiros, ver nota)", "§MONO§Implementado"],
        ["Solana / outras chains EVM alem de Ethereum mainnet", "§MONO§Nao implementado"],
        ["Adicao e remocao de liquidez apos o lancamento (eventos subsequentes)", "§MONO§Nao implementado"],
        ["Autoridade de freeze", "§MONO§Nao implementado"],
        ["Impostos de compra e venda embutidos no contrato", "§MONO§Nao implementado (proposital — ver nota abaixo)"],
        ["Honeypot / bloqueio de venda", "§MONO§Nao implementado (proposital — ver nota abaixo)"],
        ["Carteira do deployer (historico, reputacao)", "§MONO§Nao implementado"],
        ["Distribuicao coordenada entre carteiras (sybil pattern)", "§MONO§Implementado (candidato heuristico)"],
        ["Deteccao de volume artificial (wash trading)", "§MONO§Nao implementado"],
    ], [340, 150]))
    story.append(Spacer(1, 6))
    story.append(callout_box(
        "<b>Honeypot e imposto de compra/venda ficaram deliberadamente de fora</b> — detectar isso "
        "direito exige simular uma compra+venda de verdade (ou uma heuristica de bytecode fraca "
        "demais pra um item de checklist de seguranca). Prefere-se mostrar 'nao implementado' a "
        "mostrar um numero que parece confiavel e nao e — o mesmo principio de nunca inventar "
        "identidade que rege a rotulagem de carteiras (Secao 12.2).",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "<b>Correcao de terminologia (revisao tecnica externa, 13/08/2026):</b> a v2.1 tratava "
        "'queimado' e 'travado' como sinonimos. Nao sao. O que o codigo verifica e apenas LP "
        "mandado pra endereco de queima (0x...dEaD ou zero) — irrecuperavel pra sempre, verificavel "
        "diretamente on-chain via balanceOf/totalSupply, sem confiar em terceiro nenhum. LP "
        "depositado num CONTRATO LOCKER (Unicrypt, Team Finance, PinkLock etc.) e uma coisa "
        "diferente: recuperavel apos um prazo, e a garantia depende de confiar naquele contrato "
        "especifico nao ter uma porta dos fundos. Essa segunda verificacao NAO esta implementada — "
        "exigiria reconhecer os contratos de locker mais comuns e ler o prazo de cada deposito, "
        "escopo maior que o resto do checklist.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "<b>Distribuicao coordenada</b> reusa o mesmo dado de Transfer ja buscado pra concentracao de "
        "holders (sem chamada extra): flags quando uma unica origem manda quantias com dispersao "
        "menor que 15% pra 5+ carteiras distintas. E um CANDIDATO, nao um veredito — um airdrop "
        "legitimo bate o mesmo padrao. Aparece no feed como aviso, nunca muda net_edge.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "Os itens nao implementados restantes exigem, em graus variados: um indexador de eventos historicos "
        "(para holders/deployer/distribuicao), simulacao de compra+venda no proprio contrato (para "
        "honeypot/impostos), e cobertura de uma segunda chain (Solana usa um modelo de conta "
        "totalmente diferente de EVM, e um modulo a parte, nao uma extensao do atual). Continua "
        "valendo: leverage=1.0 fixo e o menor teto de risco do sistema (0,10%) para esta estrategia, "
        "justamente porque a cobertura ainda e parcial.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    # --------------------------------------------------------------- Sec5 (IA)
    story.append(P("5. Governanca de IA/ML", "H1"))
    story.append(P(
        "O roadmap original menciona ML e LLM (classificador de noticias na Fase 3, scoring de "
        "exaustao na Fase 6) sem regras de governanca. <b>Atualizacao: isto mudou de estado.</b> O "
        "News Reactor hoje classifica direcao/confianca via Ollama local (Secao 12.1) — nao e mais "
        "so coleta. As regras abaixo deveriam ter valido <b>antes</b> desse modelo entrar em producao; "
        "valeram parcialmente: nenhum dado saiu da maquina e o custo e zero, mas versionamento formal, "
        "validacao walk-forward, calibracao de probabilidade e deteccao de drift — os itens da lista "
        "abaixo — ainda NAO foram aplicados ao classificador que ja esta rodando. Registrado aqui como "
        "divida tecnica explicita, nao como regra cumprida:", "Body"))
    story.append(bullets([
        "Versionamento dos dados usados para treinar/calibrar qualquer modelo.",
        "Registro de modelos (qual versao, quando treinada, com qual dado).",
        "Validacao walk-forward (nunca validar com dado que sobrepoe o periodo de treino).",
        "Calibracao das probabilidades emitidas (uma confianca de 70% precisa realmente acertar ~70% das vezes).",
        "Deteccao de drift (o modelo para de bater com o regime de mercado atual).",
        "Comparacao formal entre modelo ativo e modelo candidato antes de trocar.",
        "Fallback deterministico sempre disponivel se o modelo falhar ou ficar indisponivel.",
        "Proibicao explicita de um modelo novo entrar sozinho em producao sem essa comparacao.",
        "Registro de qual versao de modelo gerou cada decisao (rastreabilidade no proprio evento do dashboard).",
    ]))

    story.append(PageBreak())

    # --------------------------------------------------------------- Sec6
    story.append(P("6. Fontes de dados por modulo", "H1"))
    story.append(section_table([
        ["Modulo", "Fonte", "Tipo de acesso"],
        ["Arbitragem / Order Flow", "WebSocket publico Bybit e Bitget (order book, trades)", "Publico, sem API key"],
        ["Launch Radar (CEX)", "Bybit instruments-info (ContinuousTrading)", "Publico"],
        ["Launch Radar (DEX)", "PairCreated Uniswap V2 + eth_call direto ao contrato", "Publico (no RPC)"],
        ["Whale Watch", "JSON-RPC subscriptions (Ethereum) + rotulagem de carteiras publicas", "Publico (no proprio ou provedor RPC)"],
        ["News Reactor", "SEC EDGAR (filings)", "Publico (API oficial)"],
        ["Macro Engine", "Calendario BLS (CPI, payroll) e Federal Reserve (FOMC)", "Publico (calendarios oficiais)"],
        ["Pump Exhaustion", "Bybit linear tickers (funding, 24h) ao vivo; klines + funding history para backtest", "Publico"],
        ["Liquidation Hunter", "Bybit allLiquidation (perpetuos)", "Publico"],
        ["Multi-Asset", "Alpaca Markets IEX (acoes/ETFs dos EUA — ativo; forex/indices ainda sem fonte, Secao 12.3)", "Publico (API oficial, conta gratuita)"],
    ], [130, 250, 100]))

    story.append(P("7. Resultados reais ate agora", "H1"))
    story.append(P(
        "Numeros medidos, nao estimados — extraidos de <font face='JetBrainsMono'>orchestrator/data/"
        "events.jsonl</font> (2596 eventos) via <font face='JetBrainsMono'>backtests/"
        "live_performance_report.py</font>, do backtest historico via "
        "<font face='JetBrainsMono'>backtests/pump_exhaustion_historical.py</font> contra dado real "
        "da Bybit, e da analise de breakeven via "
        "<font face='JetBrainsMono'>backtests/edge_threshold_analysis.py</font> (Secao 7.1):", "Body"))
    story.append(section_table([
        ["Estrategia", "Operacoes", "Taxa de acerto", "PnL liquido", "Leitura"],
        ["Arbitragem", "222", "56,3%", "-US$3,96", "§MONO§negativo apesar do acerto > 50%"],
        ["Order Flow", "130", "42,3%", "-US$0,21", "§MONO§negativo, amostra pequena"],
    ], [90, 75, 90, 90, 145]))
    story.append(Spacer(1, 6))
    story.append(callout_box(
        "<b>Exatamente o padrao que a Secao 3.1 existe para prevenir:</b> arbitragem acerta a maioria "
        "das operacoes (56,3%) e ainda assim perde dinheiro no acumulado — as perdas sao maiores que "
        "os ganhos em media (razao risco/retorno medida: 0,23). Isso nao e um bug: e o motivo pelo "
        "qual nenhuma das duas esta perto de avancar na escada de maturidade da Secao 2.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(Spacer(1, 6))
    story.append(P("7.1 Causa raiz encontrada e corrigida — limiares abaixo do breakeven", "H2"))
    story.append(P(
        "Analise (<font face='JetBrainsMono'>backtests/edge_threshold_analysis.py</font>, 12/08/2026) "
        "sobre o log bruto continuo (Secao 12): o proprio modelo de confianca usado na simulacao ja "
        "implica um ponto de breakeven matematico. Para arbitragem, "
        "<font face='JetBrainsMono'>confidence = clamp(0.5 + edge&#215;25, 0.4, 0.9)</font> combinado "
        "com <font face='JetBrainsMono'>max_loss_pct</font> de 0,4% da um breakeven em "
        "<b>~0,297%</b> de edge liquido — o limiar antigo (0,06%) deixava passar operacoes que o "
        "PROPRIO sistema ja calculava como valor esperado negativo. Para order flow, a confianca e "
        "FIXA em 0,45 (abaixo de 50% de proposito, ver order_flow.rs) — breakeven em <b>~0,367%</b> "
        "contra um limiar antigo de 0,04%, ou seja, praticamente toda operacao tomada tinha EV "
        "negativo pelo proprio modelo. Isso explica exatamente o padrao observado: acerto acima de "
        "50% e ainda assim prejuizo liquido acumulado.", "Body"))
    story.append(callout_box(
        "<b>Correcao de um erro de calculo (revisao tecnica externa, 13/08/2026):</b> a v2.1 "
        "publicava 0,544% como o breakeven de arbitragem. Estava errado — o termo constante da "
        "equacao expandida e -0,5&#215;max_loss_pct, nao -max_loss_pct; o valor correto, reconferido "
        "simbolicamente, e 0,297%. <font face='JetBrainsMono'>backtests/"
        "edge_threshold_analysis.py::solve_breakeven_quadratic()</font> corrigido. O limiar de "
        "producao (0,65%) continua valido — fica com margem ainda maior sobre o breakeven real do "
        "que se pensava, entao a correcao nao muda a configuracao, so a matematica documentada.",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
    ))
    story.append(Spacer(1, 4))
    story.append(P("Definicoes separadas (revisao tecnica: 'multiplicar net_edge por confidence pode contar a incerteza duas vezes' se os termos nao forem precisos)", "H2"))
    story.append(bullets([
        "<b>Vantagem bruta</b> — spread observado no book, antes de qualquer desconto.",
        "<b>Custos</b> — taxas maker/taker de cada exchange, já descontadas dentro de net_edge (ROUND_TRIP_FEE em arbitrage.rs).",
        "<b>net_edge</b> — vantagem bruta menos custos, assumindo que a operação executa exatamente como observada. NÃO é probabilidade-ajustado — é um valor condicional (\"se a operação sair como visto no book, o retorno é este\").",
        "<b>confidence</b> — probabilidade estimada de que a operação realmente capture esse net_edge (falha de segunda perna, latência, seleção adversa). É aqui, não em net_edge, que a incerteza de execução entra.",
        "<b>max_loss_pct</b> — perda condicionada ao fracasso, como fração do notional.",
        "<b>EV final</b> — confidence&#215;net_edge &#8722; (1&#8722;confidence)&#215;max_loss_pct.",
    ]))
    story.append(callout_box(
        "Não há dupla contagem: net_edge é condicional (\"se der certo\"), confidence é a probabilidade "
        "separada de dar certo — é exatamente como EV = P(ganho)&#215;ganho + P(perda)&#215;perda deve "
        "ser calculado. O que falta, e continua faltando (ver Secao 12.5): confidence e max_loss_pct "
        "ainda são estimativas, não calibradas contra taxa de preenchimento/rejeição real medida.",
        border_color=CYAN, bg=colors.HexColor("#E7F8FC"),
    ))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "<b>Corrigido nesta revisao:</b> MIN_NET_EDGE elevado para 0,65% (arbitragem) e 0,45% (order "
        "flow) — acima do breakeven teorico, com margem para custos de execucao nao modelados. "
        "Efeito esperado: volume de operacoes cai drasticamente (a maioria dos cruzamentos de edge "
        "observados no book real fica abaixo desses novos limiares). Isso e o resultado honesto, nao "
        "um defeito da correcao: sinaliza que arbitragem/order-flow 'ingenuos' nesses pares liquidos e "
        "populares provavelmente nao tem margem suficiente sobre custo de transacao para serem "
        "lucrativos — mercado eficiente, competido por firmas de alta frequencia com infraestrutura "
        "que este sistema nao tem. O caminho de maior potencial de lucro, alinhado com a visao "
        "original do PDF, continua sendo as estrategias de assimetria de informacao/tempo (Whale "
        "Watch, News, Launch Radar, Pump Exhaustion) — nao arbitragem estatistica pura.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(callout_box(
        "<b>Ainda nao prova lucratividade</b> (revisao tecnica externa): aumentar o limiar nao "
        "demonstra por si so que existe vantagem estatistica real. A validacao correta precisa ser "
        "fora da amostra usada pra escolher o limiar, depois de custos, e sem reusar o mesmo periodo "
        "de dado pra calibrar E validar — isso ainda nao foi feito. O rastreador da Fase 11 (Secao "
        "8.2) mede o desempenho DEPOIS da correcao, mas o periodo observado ate agora e curto demais "
        "pra qualquer conclusao. Nenhuma tecnologia — deste sistema ou de qualquer outro — garante "
        "lucro rapido ou preve movimentos de mercado inesperados.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))

    story.append(P(
        "<b>Pump Exhaustion (backtest historico, 60 dias, 30 simbolos, dado real Bybit):</b> o gatilho "
        "exato do detector em producao (funding &gt;0,10%/8h E variacao de 24h &gt;+15%, simultaneos) "
        "<b>nao disparou nenhuma vez</b> nesta janela. Isso significa que o criterio de saida da Fase "
        "6/10 ainda nao pode ser avaliado — nao ha amostra para comparar contra o marcador aleatorio. "
        "Duas leituras possiveis, ainda em aberto: (a) o limiar esta calibrado forte demais para o "
        "regime de volatilidade atual, ou (b) o padrao de exaustao que ele busca e genuinamente raro "
        "nessa resolucao horaria.", "Body"))
    story.append(Spacer(1, 4))
    story.append(P(
        "<b>Calibracao feita (backtests/pump_exhaustion_calibration.py):</b> varredura de 25 "
        "combinacoes de limiares sobre o mesmo dado real. Resultado: o gargalo e o limiar de "
        "<b>funding</b>, nao o de pump — mesmo afrouxando o pump para &gt;3%, nenhum sinal aparece "
        "enquanto funding&gt;0,07%. Só a partir de funding&gt;0,05% um símbolo (SANDUSDT) começa a "
        "aparecer; em funding&gt;0,02% + pump&gt;3% (5x mais frouxo que produção nos dois eixos) "
        "chegam 78 sinais em 7 símbolos — ainda abaixo do minimo de operacoes exigido pela Secao 3.1. "
        "Leitura honesta: no regime de volatilidade de 2026, funding raramente ficou tao esticado "
        "quanto o limiar de producao pressupoe — nao e uma recomendacao de afrouxar o limiar de "
        "producao (mudaria o que 'exaustao' significa), e sim evidencia de que este detector "
        "especificamente precisa de uma janela de dado mais longa (meses, nao 60 dias) antes que a "
        "Fase 6/10 tenha amostra suficiente pra validar.", "Body"))

    story.append(PageBreak())

    # --------------------------------------------------------------- Sec8
    story.append(P("8. Decisoes pendentes (precisam de voce)", "H1"))

    story.append(P("8.1 Capital inicial — duas opcoes concretas", "H2"))
    story.append(P(
        "O sistema hoje monitora Bybit e Bitget simultaneamente para arbitragem, o que so faz sentido "
        "operacionalmente se as DUAS tiverem capital. Isso nao muda nada enquanto tudo e paper trading "
        "— so importa quando a Fase 12 (capital real) for cogitada. Duas opcoes validas, mutuamente "
        "exclusivas:", "Body"))
    story.append(section_table([
        ["Opcao", "Como funciona"],
        ["A — US$100 em cada exchange (US$200 total)", "Ambas as pontas do hedge de arbitragem ficam financiadas desde o primeiro dia real. E o que o codigo assume hoje (total_equity_start = US$200)."],
        ["B — US$100 total, uma exchange primeiro", "Bybit financiada, Bitget so monitorada (leitura publica, sem custo). Arbitragem cross-exchange continua em paper/shadow-live ate existir capital nas duas pontas; ativacao real da 2a perna e um evento explicito futuro, nao o boot inicial."],
    ], [220, 270]))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "Ainda sem resposta sua. Enquanto isso, o codigo continua com a Opcao A como padrao de "
        "configuracao (e o unico jeito de nao travar o desenvolvimento esperando), mas nenhuma linha "
        "disso e definitiva ate a Fase 12 — trocar depois e so mudar <font face='JetBrainsMono'>"
        "total_equity_start</font> e a logica de ativacao da 2a perna, nao redesenhar o sistema.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(Spacer(1, 4))
    story.append(P(
        "<b>Recomendacao externa recebida (13/08/2026), coerente com a Opcao A:</b> US$100 na Bybit + "
        "US$100 na Bitget, com operacoes REAIS inicialmente desativadas independente da opcao "
        "escolhida, e arbitragem cross-exchange em shadow/paper ate comprovar sincronizacao e "
        "execucao — nunca ativa so porque as duas contas existem. Com apenas US$100 numa unica "
        "plataforma, arbitragem entre exchanges nao pode ser executada corretamente (falta a "
        "segunda perna) — nesse caso o sistema so observaria ou faria trading direcional numa "
        "exchange so, nao arbitragem de verdade. Essa recomendacao nao muda nada em paper trading "
        "(ja e como o sistema opera hoje); passa a valer no momento em que a Fase 12 for cogitada.", "Body"))

    story.append(P("8.2 Criterios numericos da Fase 11 — proposta concreta e rastreador automatico", "H2"))
    story.append(P(
        "<font face='JetBrainsMono'>backtests/fase11_progress.py</font> calcula os criterios abaixo "
        "automaticamente a partir do events.jsonl real, a partir do momento em que os limiares de "
        "edge foram corrigidos pra breakeven (Secao 7.1) — sem isso, dado de antes e de depois da "
        "correcao ficariam misturados de forma enganosa. Rode a qualquer momento pra ver o progresso "
        "atual, sem precisar recalcular na mao.", "Body"))
    story.append(P(
        "Nao sao mais placeholder generico — abaixo estao numeros propostos nesta revisao, prontos "
        "pra virar gate real no codigo assim que confirmados (ou ajustados) por voce:", "Body"))
    story.append(bullets([
        "Minimo 4 semanas consecutivas de paper trading com dado 100% real, sem interrupcao nao planejada acima de 24h.",
        "Minimo 150 operacoes executadas nesse periodo, por estrategia (amostra pequena demais nao vale — ver achado da Secao 7 sobre Pump Exhaustion com 0 sinais).",
        "Resultado liquido acumulado positivo apos custos simulados, com profit factor &#8805; 1,3.",
        "Drawdown maximo nunca ultrapassa o limite configurado (Secao 9) sem que a reducao automatica de perna tenha acionado corretamente.",
        "Zero violacoes de limite de risco nos logs (toda oportunidade fora do limite foi corretamente rejeitada).",
        "Revisao manual sua dos logs de decisao — nao so do resultado final acumulado.",
    ]))

    story.append(P("8.3 Decisoes de provedor — todas resolvidas nesta revisao", "H2"))
    story.append(P(
        "As tres decisoes de provedor que a v1 listava como pendentes ja foram resolvidas: Multi-Asset "
        "(Alpaca, ativo), Whale Watch em escala (Alchemy, ativo) e classificacao de noticias (Ollama "
        "local, ativo). Detalhes de cada uma na Secao 12.", "Body"))

    story.append(P("9. Parametros de risco atuais", "H1"))
    story.append(P("Espelha exatamente <font face='JetBrainsMono'>orchestrator/config/risk.toml</font> em producao:", "Body"))
    story.append(section_table([
        ["Parametro", "Valor", "Significado"],
        ["total_equity_start", "US$ 200", "Capital inicial de referencia para paper trading"],
        ["protected_reserve_pct", "20%", "Fracao do lucro de estrategias continuas que vai para reserva"],
        ["rare_event_reserve_pct / infra_pct", "20% / 10%", "Divisao 70/20/10 para lucro de estrategias de evento raro"],
        ["initial_leg_size", "US$ 25", "Notional inicial de uma ordem por estrategia (nao e o risco — Secao 1.2)"],
        ["total_risk_pct", "0,50%", "Risco maximo simultaneo somando todas as estrategias"],
        ["arbitrage / order_flow", "0,35% / 0,25%", "Risco maximo individual por estrategia"],
        ["news / launch", "0,15% / 0,10%", "Risco maximo individual (launch e o mais baixo do sistema)"],
        ["pump_exhaustion / whale_watch / macro / liquidation_hunter", "0,25% / 0,20% / 0,20% / 0,20%", "Risco maximo individual por estrategia"],
        ["min_cycles_between_scale", "50 ciclos", "Ciclos minimos entre aumentos de tamanho de perna"],
        ["scale_growth_factor", "1,15 (+15%)", "Fator de crescimento da perna quando as condicoes de escalonamento sao atendidas — nunca dobra (correcao da v1)"],
        ["drawdown_halve_threshold_pct", "2%", "Drawdown a partir do qual a perna e reduzida pela metade"],
        ["max_leg_fraction_of_equity", "30%", "Fracao maxima do equity total que uma unica perna pode representar"],
    ], [190, 110, 190]))
    story.append(callout_box(
        "<b>Nota de correcao (v1 -> v2):</b> a v1 deste documento descrevia o escalonamento como "
        "'dobra perna em nova maxima + 50 ciclos + drawdown baixo' na secao da Fase 0. Isso nunca foi "
        "o que o codigo faz — era uma descricao desatualizada de uma ideia descartada ainda na "
        "conversa de planejamento. O motor de risco (<font face='JetBrainsMono'>risk.rs::maybe_scale</font>) "
        "sempre usou <font face='JetBrainsMono'>scale_growth_factor</font> configuravel, hoje 1,15 — "
        "exatamente o degrau de 10-15% que deveria estar documentado. A tabela acima e a fonte da "
        "verdade a partir de agora.",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
    ))

    story.append(PageBreak())

    # --------------------------------------------------------------- Sec9 (regra live)
    story.append(P("10. Regra de execucao com dinheiro real — redacao esclarecida", "H1"))
    story.append(P(
        "A v1 dizia apenas 'nenhuma execucao automatica com dinheiro real', o que e ambiguo: o "
        "objetivo final e justamente um sistema que opera automaticamente. A redacao correta separa "
        "quem ativa de quem opera depois de ativado:", "Body"))
    story.append(callout_box(
        "O assistente nunca ativa nem conecta capital real sozinho, sob nenhuma circunstancia — "
        "inclusive com autonomia ampla concedida pelo usuario para 'seguir pelo melhor caminho'. "
        "Conectar chaves de API reais e dar o primeiro comando de modo live e sempre um ato manual, "
        "explicito e deliberado do usuario. <b>Depois</b> de habilitado pelo usuario, o AurumOS pode "
        "operar automaticamente dentro dos limites de risco ja aprovados (Secao 9 de parametros), ate "
        "que um kill-switch — manual ou automatico por drawdown — seja acionado.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))

    story.append(PageBreak())
    story.append(P("11. Identidade visual", "H1"))
    story.append(P(
        "Este documento e o dashboard de observabilidade (Fase 9) agora seguem o AurumOS Brand Kit: "
        "paleta Gold 500 (#F5C453) / Ink 950 (#06080B) / Signal Cyan (#18D7FF), tipografia Space "
        "Grotesk (titulos) + Inter (corpo) + JetBrains Mono (dados/numeros), logo oficial, sumario "
        "com numeros de pagina, e este diagrama de arquitetura vetorial substituindo o espaco em "
        "branco da v1.", "Body"))

    story.append(P("12. Provedores e integracoes — pesquisa e estado atual", "H1"))
    story.append(P(
        "Pesquisa feita nesta revisao pelos 'melhores provedores gratuitos possiveis' pedidos, com uma "
        "restricao explicita: o assistente nunca cria contas em nome do usuario (regra de seguranca "
        "permanente, nao uma limitacao tecnica). Onde a melhor opcao exige conta, o codigo ja esta "
        "pronto — falta so a chave.", "Body"))

    story.append(P("12.1 Classificacao de noticias — resolvido, sem custo", "H2"))
    story.append(callout_box(
        "<b>Ollama + llama3.2:3b, rodando localmente.</b> Instalado e integrado nesta revisao — "
        "gratuito para sempre, sem chave, sem conta, sem limite de chamadas, e o titulo do filing "
        "nunca sai da maquina do usuario. News Reactor agora chama "
        "<font face='JetBrainsMono'>http://localhost:11434</font> pra classificar direcao (up/down/"
        "neutral) e confianca de cada filing 8-K novo. net_edge continua 0.0 (Secao 0 e Secao 5 "
        "exigem confirmacao de preco e validacao retrospectiva antes de virar vantagem numerica) — o "
        "que mudou e que a classificacao agora e real, nao ausente.",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
    ))

    story.append(P("12.2 Whale Watch em escala — ativo", "H2"))
    story.append(P(
        "Conta gratuita <b>Alchemy</b> criada pelo usuário e configurada. Whale Watch e Launch Radar/"
        "DEX agora leem <font face='JetBrainsMono'>AURUMOS_ETH_WS_URL</font> e "
        "<font face='JetBrainsMono'>AURUMOS_ETH_HTTP_URL</font> de um arquivo "
        "<font face='JetBrainsMono'>orchestrator/.env</font> local, com o RPC público como fallback "
        "caso o arquivo não exista.", "Body"))
    story.append(callout_box(
        "<b>Bug real corrigido (13/08/2026):</b> a promessa de \"carrega automaticamente, sem "
        "configurar nada\" não era verdade na prática — <font face='JetBrainsMono'>dotenvy::dotenv()"
        "</font> procura o arquivo <font face='JetBrainsMono'>.env</font> a partir do diretório de "
        "trabalho do processo, não do diretório do código-fonte. Se o binário for iniciado a partir da "
        "raiz do workspace (o padrão, já que os caminhos de dado usam "
        "<font face='JetBrainsMono'>CARGO_MANIFEST_DIR</font>), o arquivo nunca era encontrado — "
        "Whale Watch e Multi-Asset caíam silenciosamente no fallback (nó público / desativado) mesmo "
        "com chaves válidas configuradas há sessões. Corrigido para caminho absoluto, mesmo padrão dos "
        "arquivos de dado. Achado ao reiniciar o processo pra validar as correções desta revisão — não "
        "fazia parte da auditoria original, mas afetava diretamente dois módulos que ela cobre.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(P(
        "Rotulagem de carteiras ampliada de 6 para 11 endereços (Binance, Coinbase, Kraken, Bybit, "
        "OKX, KuCoin), cada um verificado individualmente contra o rótulo público do Etherscan antes "
        "de entrar na lista — nenhum inventado. Achado honesto ao tentar validar a hipótese de direção "
        "(depósito em exchange conhecida = pressão vendedora, saque = acumulação, ver Fase 4): com só "
        "6 endereços, <b>zero</b> dos 2.698 movimentos de baleia observados em ~18,5h bateram em algum "
        "deles — exchanges de verdade usam centenas de carteiras quentes, não uma só. A hipótese não "
        "estava \"não validada\", estava impossível de testar por falta de amostra. A ampliação é um "
        "primeiro passo; validação contra preço real segue em andamento.", "Body"))

    story.append(P("12.3 Multi-Asset (ações/ETFs dos EUA) — ativo, escopo corrigido", "H2"))
    story.append(P(
        "Conta paper trading gratuita <b>Alpaca Markets</b> criada pelo usuário e configurada. "
        "<font face='JetBrainsMono'>MultiAssetSource</font> conectado ao WebSocket de trades da "
        "Alpaca, observando 6 ações líquidas (AAPL, MSFT, SPY, QQQ, NVDA, TSLA) e emitindo "
        "oportunidades informativas (net_edge=0.0) em movimentos &gt;0,5% numa janela de 60s — "
        "Fase 8 do roadmap deixa de estar bloqueada.", "Body"))
    story.append(callout_box(
        "<b>Correção de escopo (revisão técnica externa, 13/08/2026):</b> \"Multi-Asset ativo\" "
        "descrevia bem mais do que o código cobre hoje. O que existe: 6 ações/ETFs líquidos dos EUA "
        "via Alpaca IEX. O que NÃO existe ainda: forex (pares de moeda), dólar (DXY ou similar) e "
        "índices futuros — nenhuma fonte de dado nem implementação para esses três está escrita. "
        "Título e texto desta seção corrigidos para refletir exatamente o que roda, sem sugerir "
        "cobertura mais ampla do que a real.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(callout_box(
        "Chaves da Alpaca e da Alchemy ficam em <font face='JetBrainsMono'>orchestrator/.env</font> "
        "(arquivo local, nunca versionado nem enviado a lugar nenhum além dos próprios provedores "
        "configurados). Se precisar trocar de chave, gere uma nova no dashboard do provedor e cole "
        "no mesmo arquivo — nenhuma mudança de código necessária.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(P("12.4 Kill-switch — em camadas (revisado 2x)", "H2"))
    story.append(P(
        "Entregável explícito da Fase 12. Revisado após 3ª auditoria técnica externa (13/08/2026): 5% "
        "diário único era alto demais para microcapital — passou a 4 gatilhos independentes. Endurecido "
        "de novo após 4ª auditoria (mesmo dia): o preventivo passou a reduzir a perna, não só avisar; "
        "o rígido diário caiu de 2% para 1,5%; e o nível mais grave (drawdown total) deixou de se "
        "liberar sozinho quando o equity recupera:", "Body"))
    story.append(section_table([
        ["Camada", "Limite", "Efeito"],
        ["Manual", "arquivo data/KILL existe", "§MONO§Bloqueia (crie/apague o arquivo)"],
        ["Preventivo diário", "1% de drawdown no dia (UTC)", "§MONO§Aviso + reduz a perna pela metade (uma vez, na borda)"],
        ["Rígido diário", "1,5% de drawdown no dia (UTC)", "§MONO§Bloqueia até o dia seguinte"],
        ["Semanal", "4% de drawdown na semana", "§MONO§Bloqueia até a semana seguinte"],
        ["Drawdown total", "7% desde o pico histórico", "§MONO§Bloqueia — NÃO libera sozinho, exige reset manual (data/RESET_DRAWDOWN_HALT)"],
    ], [130, 190, 170]))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "O nível de drawdown total é o único que trava de propósito: diferente dos outros 3, ele NÃO "
        "reavalia e libera sozinho quando o equity sobe de volta acima do limite — fica bloqueado até "
        "alguém criar o arquivo <font face='JetBrainsMono'>data/RESET_DRAWDOWN_HALT</font> (consumido/"
        "apagado automaticamente ao ser lido — é uma confirmação de uma vez, não um interruptor "
        "permanente). Decisão da 4ª revisão técnica: o nível mais grave de proteção não deveria se "
        "auto-corrigir silenciosamente.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "<b>Sobre \"cancelar ordens abertas\" (ponto levantado na revisão técnica):</b> o kill-switch "
        "não neutraliza nem fecha posições abertas porque, no modelo de execução atual, NÃO EXISTE "
        "posição aberta entre um ciclo e outro — toda ordem simulada é round-trip instantâneo (decide "
        "e resolve no mesmo tick, ver Seção 3). Não é uma lacuna do kill-switch, é uma característica "
        "honesta do estágio atual do sistema. Se/quando o modelo de execução passar a manter posições "
        "reais abertas (o próximo passo natural do realismo de execução, Seção 12.5), o kill-switch "
        "PRECISA ganhar lógica de neutralizar/fechar posição — registrado aqui como pré-requisito "
        "explícito antes dessa mudança maior, não algo a esquecer.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))

    story.append(P("12.5 Realismo de execução — parcialmente implementado", "H2"))
    story.append(P(
        "Dos itens listados na Seção 3, dois agora estão simulados: preenchimento parcial de ordem "
        "(fração aleatória do notional pedido em ~35% das execuções) e falha da segunda perna em "
        "arbitragem (perna 1 executa, perna 2 falha — ~3% das execuções, tratado como perda no dobro "
        "do <font face='JetBrainsMono'>max_loss_pct</font> sobre o notional pedido inteiro, não só o "
        "preenchido). <b>Ainda faltam:</b> fila de ordens maker, rate limits, clock drift, "
        "reconciliação pós-desconexão, funding cobrado durante posição aberta — a lista completa "
        "continua na Seção 3.", "Body"))
    story.append(callout_box(
        "<b>Honestidade sobre esses números (revisão técnica externa):</b> 35% e 3% são estimativas "
        "arbitrárias, não calibradas contra latência, rejeição ou preenchimento reais — o roadmap já "
        "dizia isso desde a primeira versão, mas vale repetir aqui com todas as letras: nenhum desses "
        "dois números veio de dado medido. Diferente do net_edge do Pump Exhaustion (Seção 2.1), que "
        "agora É calibrado por confirmação real, estes continuam placeholder. Calibrá-los exigiria "
        "execução real (não paper trading) ou uma fonte externa de latência/book de nível institucional "
        "— nenhuma das duas disponível nesta fase.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(Spacer(1, 4))
    story.append(P("12.6 Escrita de eventos — writer único (revisado)", "H2"))
    story.append(P(
        "Revisão técnica externa: confiar apenas na atomicidade de <font face='JetBrainsMono'>write()"
        "</font> em modo append, reabrindo o arquivo a cada evento, é mais frágil que um writer único "
        "explícito. Corrigido: <font face='JetBrainsMono'>events.rs</font> agora abre o arquivo UMA vez "
        "e protege escritas concorrentes com um mutex — nenhuma dependência de garantia de baixo nível "
        "do sistema de arquivos, impossível duas emissões concorrentes colidirem, ponto.", "Body"))
    story.append(P("12.7 Escalonamento — profit factor + robustez a outlier (revisado 2x)", "H2"))
    story.append(P(
        "Revisão técnica externa: ciclos + drawdown baixo sozinhos não provam lucratividade recente. "
        "Corrigido: aumentar a perna agora também exige profit factor mínimo numa janela móvel dos "
        "últimos trades (mínimo 30 amostras) — amostra pequena demais conta como \"não comprovado\", "
        "não escala. Endurecido na 4ª revisão: o piso subiu de 1,1 para 1,2 (1,1 sumia fácil com ruído "
        "ou uma única operação ruim) e ganhou uma segunda checagem independente — remove o melhor "
        "trade da janela e exige que o saldo continue positivo mesmo assim, pra não escalar em cima de "
        "um resultado carregado por uma única operação excepcional. Ver "
        "<font face='JetBrainsMono'>risk.rs::recent_profit_factor</font> e "
        "<font face='JetBrainsMono'>risk.rs::positive_excluding_best_trade</font>.", "Body"))

    story.append(P("12.8 Dashboard — cockpit de risco e pulso das estratégias (13/08/2026)", "H2"))
    story.append(P(
        "Reestruturação visual do painel (Fase 9), motivada por uma lacuna real encontrada ao "
        "construí-la: os 4 limiares do kill-switch em camadas (Seção 12.4) e o drawdown diário/semanal "
        "nunca chegavam ao frontend — só o drawdown total desde o pico. O painel não tinha como mostrar "
        "o estado real do kill-switch, só o resultado final (bloqueado ou não). Corrigido expondo "
        "<font face='JetBrainsMono'>daily_drawdown_pct</font>, <font face='JetBrainsMono'>"
        "weekly_drawdown_pct</font> e os 4 limiares configurados nos eventos "
        "<font face='JetBrainsMono'>portfolio_snapshot</font>/<font face='JetBrainsMono'>risk_config"
        "</font> — o novo \"cockpit de risco\" na Visão Geral desenha as 4 camadas com barra de "
        "consumo real, não um número solto.", "Body"))
    story.append(P(
        "Também adicionado: cards de \"pulso\" por estratégia (PnL recente, taxa de acerto, sparkline) "
        "visíveis na Visão Geral sem precisar abrir cada aba — decisão direta da análise desta revisão "
        "de que profit factor por estratégia importa mais que olhar só o win rate agregado do sistema.", "Body"))

    story.append(P("12.9 Amostragem independente e universo de símbolos (4ª revisão, 13/08/2026)", "H2"))
    story.append(P(
        "<b>Pump Exhaustion contava o mesmo evento várias vezes:</b> achado correto da revisão — um "
        "pump sustentado por minutos gerava \"sinal true\" em várias janelas de scan seguidas, e cada "
        "uma virava uma amostra nova na confirmation_history, mesmo sendo o mesmo evento observado "
        "repetidamente, não eventos independentes. Corrigido em duas frentes: (1) um símbolo não "
        "registra nova confirmação enquanto já tiver uma pendente (ainda dentro da janela de 20min); "
        "(2) o cooldown por símbolo subiu de 45s para 30min. Combinado, um símbolo só contribui uma "
        "amostra nova a cada ~50min no mínimo — 20min de janela + 30min de descanso.", "Body"))
    story.append(P(
        "<b>Fase 11 ganhou exigência de distribuição temporal:</b> 150 operações (ou 20, pra Pump "
        "Exhaustion) todas na mesma semana não provam nada sobre regimes de mercado diferentes. "
        "<font face='JetBrainsMono'>fase11_progress.py</font> agora também exige um mínimo de semanas "
        "distintas com pelo menos uma amostra cada (4 para arbitragem/order flow, 6 para Pump "
        "Exhaustion, dado o cooldown maior) — não só contagem bruta.", "Body"))
    story.append(P(
        "<b>Universo de símbolos ampliado de 30 para 70 pares</b> (Arbitragem, Order Flow, Pump "
        "Exhaustion, Liquidation Hunter). Achado que motivou isso: dos 30 símbolos originais, "
        "GRTUSDT era o ÚNICO com edge líquido real depois de custo — todos os outros 29 (incluindo "
        "BTC/ETH) tinham edge médio negativo o tempo todo, mercado grande demais já disputado por "
        "firmas de alta frequência. A aposta dos 40 pares novos (liquidez intermediária, nem os "
        "maiores nem os ilíquidos demais pra existir nas duas exchanges) é achar outro GRTUSDT, não "
        "operar \"mais rápido\" — o motor de risco continua recusando qualquer par sem edge real "
        "medido, isso não muda com mais símbolos.",
        "Body"))
    story.append(callout_box(
        "<b>Sobre \"lucro rápido\":</b> a expectativa de lucro extremamente rápido colide, de "
        "propósito, com todo o resto desta revisão. O sistema já foi mais rápido — operava em mais "
        "situações, com menos filtro — e isso é exatamente o que causava prejuízo líquido mesmo com "
        "56% de acerto (Seção 7, achado documentado desde a 1ª revisão). Não existe lucro rápido "
        "honesto num mercado líquido sem capital competindo em infraestrutura de alta frequência "
        "(fora de cogitação por regra permanente) ou aceitar EV negativo de novo. As alavancas reais "
        "que restam — mais símbolos, camada de confirmação nas estratégias informacionais — são mais "
        "lentas por natureza, não mais rápidas.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(PageBreak())
    story.append(P("Apendice A — mapa resumido das fases historicas (v1)", "H1"))
    story.append(P(
        "A v1 deste documento organizava tudo em 13 fases sequenciais (Fase 0 a Fase 12). A Secao 2 "
        "substitui isso por uma escada de maturidade independente por estrategia, mas os numeros de "
        "fase continuam aparecendo em varios lugares do codigo e da conversa do projeto — este mapa "
        "e so a referencia de continuidade, nao a estrutura principal do documento.", "Body"))
    story.append(section_table([
        ["Fase", "Objetivo original", "Status atual"],
        ["0 — Nucleo do Orquestrador", "Tipos, motor de risco, loop de decisao", "§MONO§Concluida"],
        ["1 — Arbitragem Cross-Exchange", "Spread real Bybit/Bitget em paper trading", "§MONO§Paper — resultado negativo (Secao 7)"],
        ["2 — Order Flow / Market Making leve", "Captura de spread via cotacao maker", "§MONO§Paper — resultado negativo (Secao 7)"],
        ["3 — News Reactor", "Coleta + classificacao de filings", "§MONO§Coleta real; classificador em Secao 12"],
        ["4 — Whale Watch (on-chain)", "Movimentacao on-chain + rotulagem", "§MONO§Observacao; 11 enderecos rotulados (Secao 12.2)"],
        ["5 — Launch Radar", "Checklist pre-lancamento CEX+DEX", "§MONO§Observacao — parcial (Secao 4)"],
        ["6 — Pump Exhaustion Detector", "Scoring de exaustao de alta", "§MONO§Scoring 3D + confirmacao de preco real implementada (Secao 2.1)"],
        ["7 — Macro Engine", "Calendario + modelo de reacao", "§MONO§Observacao; so calendario"],
        ["8 — Multi-Asset Volatility", "Acoes/ETFs dos EUA (forex/indices ainda nao)", "§MONO§Ativa — conectada a Alpaca (dado real)"],
        ["9 — Dashboard de Observabilidade", "Painel em tempo real", "§MONO§Concluida, com marca oficial"],
        ["10 — Framework de Backtesting Formal", "Backtest historico por modulo", "§MONO§Iniciada (backtests/)"],
        ["11 — Validacao em Paper Trading Prolongado", "Periodo minimo validado", "§MONO§Nao iniciada — criterios na Secao 8.2"],
        ["12 — Conexao com Capital Real", "Chaves reais, kill-switch", "§MONO§Bloqueada ate validacao"],
    ], [150, 190, 150]))

    doc.multiBuild(story)


if __name__ == "__main__":
    build()
    print(f"Gerado: {OUT_PATH}")
