# -*- coding: utf-8 -*-
"""
Gera AurumOS_Relatorio_Completo.pdf — relatorio ponta a ponta (arquitetura +
resultados da sessao de 13/08/2026), com a identidade visual oficial da marca
(mesma paleta/tipografia/marca do Roadmap: ouro/carvao/ciano, Space Grotesk/
Inter/JetBrains Mono).

Uso: python generate_relatorio_completo.py
"""
import os

from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT, TA_CENTER
from reportlab.lib.pagesizes import LETTER
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    BaseDocTemplate, Frame, PageTemplate, Paragraph, Spacer, Table,
    TableStyle, PageBreak, ListFlowable, ListItem, KeepTogether,
)
from reportlab.platypus.tableofcontents import TableOfContents
from reportlab.graphics.shapes import Drawing, Rect, String, Line, Group, Path, Polygon
from reportlab.graphics import renderPDF
from svglib.svglib import svg2rlg

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT_PATH = os.path.join(ROOT, "AurumOS_Relatorio_Completo.pdf")

# ---------------------------------------------------------------- paleta ---
GOLD = colors.HexColor("#B87916")
GOLD_BG = colors.HexColor("#F5C453")
GOLD_SOFT = colors.HexColor("#FBEBC7")
INK = colors.HexColor("#0C1117")
INK_HEADER = colors.HexColor("#151C24")
CYAN = colors.HexColor("#0E7C90")
CYAN_BG = colors.HexColor("#18D7FF")
CYAN_SOFT = colors.HexColor("#DFF7FC")
SLATE = colors.HexColor("#5B6673")
WHITE = colors.HexColor("#FFFFFF")
RISK_RED = colors.HexColor("#C23A50")
RISK_SOFT = colors.HexColor("#F8DEE3")
POSITIVE = colors.HexColor("#1C9E76")
POSITIVE_SOFT = colors.HexColor("#DCF3EB")

# ------------------------------------------------------------- tipografia --
pdfmetrics.registerFont(TTFont("SpaceGrotesk", os.path.join(HERE, "fonts", "SpaceGrotesk.ttf")))
pdfmetrics.registerFont(TTFont("Inter", os.path.join(HERE, "fonts", "Inter.ttf")))
pdfmetrics.registerFont(TTFont("JetBrainsMono", os.path.join(HERE, "fonts", "JetBrainsMono.ttf")))

styles = {
    "Title": ParagraphStyle("Title", fontName="SpaceGrotesk", fontSize=29, leading=33, textColor=INK, spaceAfter=6),
    "Subtitle": ParagraphStyle("Subtitle", fontName="Inter", fontSize=12.5, leading=17, textColor=SLATE, spaceAfter=4),
    "H1": ParagraphStyle("H1", fontName="SpaceGrotesk", fontSize=17, leading=21, textColor=INK, spaceBefore=18, spaceAfter=8),
    "H2": ParagraphStyle("H2", fontName="SpaceGrotesk", fontSize=12.5, leading=16, textColor=GOLD, spaceBefore=12, spaceAfter=6),
    "H3": ParagraphStyle("H3", fontName="SpaceGrotesk", fontSize=10.5, leading=14, textColor=CYAN, spaceBefore=8, spaceAfter=4),
    "Body": ParagraphStyle("Body", fontName="Inter", fontSize=9.6, leading=14, textColor=INK, spaceAfter=6, alignment=TA_LEFT),
    "BodySmall": ParagraphStyle("BodySmall", fontName="Inter", fontSize=8.6, leading=12.5, textColor=SLATE, spaceAfter=4),
    "Callout": ParagraphStyle("Callout", fontName="Inter", fontSize=9.4, leading=13.5, textColor=INK, spaceAfter=4),
    "TableHeader": ParagraphStyle("TableHeader", fontName="SpaceGrotesk", fontSize=8.6, leading=11, textColor=WHITE),
    "TableCell": ParagraphStyle("TableCell", fontName="Inter", fontSize=8.6, leading=11.5, textColor=INK),
    "TableCellMono": ParagraphStyle("TableCellMono", fontName="JetBrainsMono", fontSize=8.2, leading=11, textColor=INK),
    "TOCHeading": ParagraphStyle("TOCHeading", fontName="SpaceGrotesk", fontSize=10.5, leading=15, textColor=INK),
    "TOCSub": ParagraphStyle("TOCSub", fontName="Inter", fontSize=9.5, leading=13.5, textColor=SLATE, leftIndent=12),
    "Mono": ParagraphStyle("Mono", fontName="JetBrainsMono", fontSize=9, leading=13, textColor=INK),
    "MetricNum": ParagraphStyle("MetricNum", fontName="SpaceGrotesk", fontSize=20, leading=22, textColor=WHITE, alignment=TA_CENTER),
    "MetricLabel": ParagraphStyle("MetricLabel", fontName="Inter", fontSize=7.6, leading=10, textColor=colors.HexColor("#C6CDD6"), alignment=TA_CENTER),
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


def callout_box(text, border_color=GOLD, bg=GOLD_SOFT, label=None):
    content = []
    if label:
        content.append(P(f'<b>{label}</b>', "H3"))
    content.append(P(text, "Callout"))
    t = Table([[content]], colWidths=[490])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), bg),
        ("BOX", (0, 0), (-1, -1), 1, border_color),
        ("LEFTPADDING", (0, 0), (-1, -1), 10),
        ("RIGHTPADDING", (0, 0), (-1, -1), 10),
        ("TOPPADDING", (0, 0), (-1, -1), 8),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 8),
    ]))
    return t


def section_divider(number, title, subtitle):
    inner = Table(
        [[P(f'<font color="#F5C453" face="JetBrainsMono" size="9">SEÇÃO {number}</font><br/>'
            f'<font color="#FFFFFF" face="SpaceGrotesk" size="15">{title}</font>', "Body"),
          P(subtitle, "BodySmall")]],
        colWidths=[220, 270],
    )
    inner.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), INK_HEADER),
        ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("LEFTPADDING", (0, 0), (0, 0), 12),
        ("RIGHTPADDING", (1, 0), (1, 0), 12),
        ("TOPPADDING", (0, 0), (-1, -1), 10),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 10),
    ]))
    return inner


def metric_tile(number, label, accent=GOLD_BG):
    inner = Table([[P(number, "MetricNum")], [P(label, "MetricLabel")]], colWidths=[112])
    inner.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), INK_HEADER),
        ("LINEABOVE", (0, 0), (-1, 0), 3, accent),
        ("TOPPADDING", (0, 0), (-1, 0), 12),
        ("BOTTOMPADDING", (0, 0), (-1, 0), 2),
        ("TOPPADDING", (0, 1), (-1, 1), 2),
        ("BOTTOMPADDING", (0, 1), (-1, 1), 12),
        ("ALIGN", (0, 0), (-1, -1), "CENTER"),
    ]))
    return inner


def metrics_row(tiles):
    t = Table([tiles], colWidths=[122] * len(tiles))
    t.setStyle(TableStyle([
        ("LEFTPADDING", (0, 0), (-1, -1), 3), ("RIGHTPADDING", (0, 0), (-1, -1), 3),
        ("TOPPADDING", (0, 0), (-1, -1), 0), ("BOTTOMPADDING", (0, 0), (-1, -1), 0),
    ]))
    return t


def load_brand_mark():
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


# ------------------------------------------------------------- diagramas ---

def pipeline_diagram():
    """9 fontes -> Orquestrador -> Motor de risco -> Execucao -> Persistencia -> Dashboard"""
    d = Drawing(490, 175)
    stages = [
        ("9 fontes de\ndado real", "Bybit · Bitget · ETH ·\nSEC · Alpaca · calendário", GOLD),
        ("Orquestrador", "Opportunity\n(canal assíncrono)", CYAN),
        ("Motor de\nrisco", "8 gates em\nsequência", GOLD),
        ("Execução\nsimulada", "confirmação real\nde preço", CYAN),
        ("Persistência\n+ Dashboard", "disco + WS\nao vivo", GOLD),
    ]
    n = len(stages)
    box_w, box_h, gap = 84, 74, 10.5
    total_w = n * box_w + (n - 1) * gap
    x0 = (490 - total_w) / 2
    y = 70
    for i, (title, sub, accent) in enumerate(stages):
        x = x0 + i * (box_w + gap)
        d.add(Rect(x, y, box_w, box_h, rx=8, ry=8, fillColor=colors.HexColor("#F7F8FA"), strokeColor=accent, strokeWidth=1.6))
        lines_t = title.split("\n")
        ty = y + box_h - 20
        for ln in lines_t:
            d.add(String(x + box_w / 2, ty, ln, fontName="SpaceGrotesk", fontSize=8.4, fillColor=INK, textAnchor="middle"))
            ty -= 11
        lines_s = sub.split("\n")
        sy = y + 22
        for ln in reversed(lines_s):
            d.add(String(x + box_w / 2, sy, ln, fontName="JetBrainsMono", fontSize=6.4, fillColor=accent, textAnchor="middle"))
            sy -= 9
        if i < n - 1:
            ax0 = x + box_w
            ax1 = ax0 + gap
            amid = y + box_h / 2
            d.add(Line(ax0, amid, ax1 - 3, amid, strokeColor=SLATE, strokeWidth=1.3))
            d.add(Polygon(points=[ax1 - 3, amid + 3, ax1 - 3, amid - 3, ax1, amid], fillColor=SLATE, strokeColor=SLATE))
    d.add(String(245, 30, "cada módulo só SUGERE — nunca decide sozinho se executa", fontName="Inter", fontSize=7.8, fillColor=SLATE, textAnchor="middle"))
    d.add(String(245, 14, "score = (vantagem_líquida × confiança) ÷ risco_de_cauda ÷ capital ÷ tempo", fontName="JetBrainsMono", fontSize=7.4, fillColor=GOLD, textAnchor="middle"))
    return d


def risk_funnel_diagram():
    """Funil de gates do risk::evaluate, estreitando ate 'Aprovado'."""
    gates = [
        "Não expirou",
        "Alavancagem dentro do limite",
        "Teto de drawdown (efetivo, não destrutivo)",
        "Kelly hierárquico (símbolo dentro da estratégia)",
        "Guard anti-martingale (vs. perna base)",
        "Limite de risco da estratégia",
        "Limite de risco total do portfólio",
        "Grupo de correlação livre",
    ]
    n = len(gates)
    row_h = 20
    top_w, bot_w = 460, 200
    d = Drawing(490, n * row_h + 34)
    y = n * row_h + 8
    for i, g in enumerate(gates):
        w = top_w - (top_w - bot_w) * (i / (n - 1))
        x = (490 - w) / 2
        fill = colors.HexColor("#F7F8FA") if i < n - 1 else POSITIVE_SOFT
        stroke = SLATE if i < n - 1 else POSITIVE
        d.add(Rect(x, y - row_h + 3, w, row_h - 5, rx=4, ry=4, fillColor=fill, strokeColor=stroke, strokeWidth=1.1))
        d.add(String(245, y - row_h + 9, g, fontName="Inter", fontSize=7.6, fillColor=INK, textAnchor="middle"))
        y -= row_h
    d.add(Rect((490 - 150) / 2, y - 22, 150, 20, rx=5, ry=5, fillColor=POSITIVE, strokeColor=POSITIVE))
    d.add(String(245, y - 16, "APROVADO → executa", fontName="SpaceGrotesk", fontSize=8.6, fillColor=WHITE, textAnchor="middle"))
    return d


def confirmation_diagram():
    """Fluxo da camada de confirmacao real de preco."""
    d = Drawing(490, 90)
    steps = [
        ("Sinal dispara", "edge instantâneo\n> limiar de fee"),
        ("Registra preço\nde entrada", "aguarda janela\n(30s / 2s)"),
        ("Confere preço\nreal depois", "compara contra\no que aconteceu"),
        ("Acumula\nhistórico real", "50 / 30 amostras\nmínimas"),
        ("Só então\nconfia", "edge + confiança\nmedidos, não chutados"),
    ]
    n = len(steps)
    box_w, box_h, gap = 84, 60, 10.5
    total_w = n * box_w + (n - 1) * gap
    x0 = (490 - total_w) / 2
    y = 18
    for i, (title, sub) in enumerate(steps):
        x = x0 + i * (box_w + gap)
        accent = CYAN if i < n - 1 else GOLD
        fillc = CYAN_SOFT if i < n - 1 else GOLD_SOFT
        d.add(Rect(x, y, box_w, box_h, rx=8, ry=8, fillColor=fillc, strokeColor=accent, strokeWidth=1.4))
        ty = y + box_h - 16
        for ln in title.split("\n"):
            d.add(String(x + box_w / 2, ty, ln, fontName="SpaceGrotesk", fontSize=7.6, fillColor=INK, textAnchor="middle"))
            ty -= 10
        sy = y + 10
        sub_lines = sub.split("\n")
        for ln in reversed(sub_lines):
            d.add(String(x + box_w / 2, sy, ln, fontName="JetBrainsMono", fontSize=6, fillColor=SLATE, textAnchor="middle"))
            sy -= 8
        if i < n - 1:
            amid = y + box_h / 2
            ax0, ax1 = x + box_w, x + box_w + gap
            d.add(Line(ax0, amid, ax1 - 3, amid, strokeColor=SLATE, strokeWidth=1.2))
            d.add(Polygon(points=[ax1 - 3, amid + 3, ax1 - 3, amid - 3, ax1, amid], fillColor=SLATE, strokeColor=SLATE))
    return d


# ------------------------------------------------------- doc + TOC + pagina
class ReportDoc(BaseDocTemplate):
    def afterFlowable(self, flowable):
        if isinstance(flowable, Paragraph):
            style_name = flowable.style.name
            text = flowable.getPlainText()
            if text == "Sumário":
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
    canv.setFillColor(GOLD)
    canv.rect(0, page_h - 4, page_w, 4, fill=1, stroke=0)
    canv.setFont("SpaceGrotesk", 8.5)
    canv.setFillColor(SLATE)
    canv.drawString(20 * mm, page_h - 14 * mm, "AurumOS")
    canv.setFont("Inter", 7.5)
    canv.drawRightString(page_w - 20 * mm, page_h - 14 * mm, "Relatório completo — arquitetura e resultados")
    canv.setStrokeColor(colors.HexColor("#E3E7EC"))
    canv.line(20 * mm, page_h - 16 * mm, page_w - 20 * mm, page_h - 16 * mm)
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

    canv.setFont("SpaceGrotesk", 40)
    canv.setFillColor(GOLD_BG)
    canv.drawString(24 * mm, page_h - 80 * mm, "Aurum")
    w = canv.stringWidth("Aurum", "SpaceGrotesk", 40)
    canv.setFillColor(CYAN_BG)
    canv.drawString(24 * mm + w, page_h - 80 * mm, "OS")

    canv.setFont("Inter", 13)
    canv.setFillColor(colors.HexColor("#C6CDD6"))
    canv.drawString(24 * mm, page_h - 92 * mm, "Relatório completo — arquitetura ponta a ponta")
    canv.drawString(24 * mm, page_h - 99 * mm, "e resultados da sessão de investigação e correção")

    canv.setFont("JetBrainsMono", 9.5)
    canv.setFillColor(GOLD_BG)
    canv.drawString(24 * mm, page_h - 115 * mm, "v1 — 13 de agosto de 2026")

    # tira de metricas na capa
    metrics = [("US$ 974", "equity ao vivo"), ("90,0%", "acerto real medido"), ("6", "bugs reais corrigidos"), ("9", "fontes de dado real")]
    mx = 24 * mm
    mw = (page_w - 48 * mm) / 4
    for i, (num, label) in enumerate(metrics):
        x = mx + i * mw
        canv.setStrokeColor(colors.HexColor("#2A3441"))
        canv.setLineWidth(0.6)
        if i > 0:
            canv.line(x, page_h - 145 * mm, x, page_h - 165 * mm)
        canv.setFont("SpaceGrotesk", 17)
        canv.setFillColor(GOLD_BG if i % 2 == 0 else CYAN_BG)
        canv.drawString(x + (0 if i == 0 else 8 * mm), page_h - 152 * mm, num)
        canv.setFont("Inter", 7.6)
        canv.setFillColor(colors.HexColor("#8B96A3"))
        canv.drawString(x + (0 if i == 0 else 8 * mm), page_h - 158 * mm, label)

    canv.setFont("Inter", 8.5)
    canv.setFillColor(colors.HexColor("#8B96A3"))
    canv.drawString(24 * mm, 20 * mm, "Documento de referência interno — paper trading, sem dinheiro real conectado")
    canv.restoreState()


# ------------------------------------------------------------------- build
def build():
    doc = ReportDoc(
        OUT_PATH,
        pagesize=LETTER,
        leftMargin=20 * mm, rightMargin=20 * mm, topMargin=20 * mm, bottomMargin=20 * mm,
        title="AurumOS - Relatorio Completo",
    )
    frame_cover = Frame(0, 0, LETTER[0], LETTER[1], id="cover")
    frame_content = Frame(20 * mm, 18 * mm, LETTER[0] - 40 * mm, LETTER[1] - 38 * mm, id="content")
    doc.addPageTemplates([
        PageTemplate(id="Cover", frames=[frame_cover], onPage=draw_cover),
        PageTemplate(id="Content", frames=[frame_content], onPage=draw_page_frame),
    ])

    from reportlab.platypus.doctemplate import NextPageTemplate
    story = [NextPageTemplate("Content"), PageBreak()]

    story.append(P("Sumário", "H1"))
    toc = TableOfContents()
    toc.levelStyles = [styles["TOCHeading"], styles["TOCSub"]]
    story.append(toc)
    story.append(PageBreak())

    # --------------------------------------------------------- metadados --
    story.append(P("AurumOS — Relatório Completo", "Title"))
    story.append(P("Como o sistema funciona, do dado bruto até uma operação simulada — e tudo que foi "
                    "investigado e corrigido na sessão de 13 de agosto de 2026.", "Subtitle"))
    story.append(Spacer(1, 10))
    story.append(metrics_row([
        metric_tile("US$ 974", "equity ao vivo (partiu de US$200)", GOLD_BG),
        metric_tile("90,0%", "acerto real medido (não mais sorteio)", CYAN_BG),
        metric_tile("6", "bugs reais encontrados e corrigidos", GOLD_BG),
        metric_tile("0", "erros / paradas no processo", POSITIVE),
    ]))
    story.append(Spacer(1, 12))
    story.append(section_table([
        ["Campo", "Valor"],
        ["Data do documento", "13 de agosto de 2026"],
        ["Modo de operação", "Paper trading — sem dinheiro real conectado, em nenhum momento"],
        ["Estado do processo no momento deste relatório", "Rodando desde 10:53:04 UTC, 7.331 ciclos, zero erros"],
        ["Regra inegociável", "O assistente nunca ativa nem conecta capital real sozinho"],
    ], [170, 320]))

    story.append(PageBreak())

    # =====================================================================
    # PARTE 1 — ARQUITETURA
    # =====================================================================
    story.append(section_divider("1", "O caminho completo", "Do dado bruto até uma operação simulada, explicado do zero"))
    story.append(Spacer(1, 10))

    story.append(P("1.1 — Visão geral em uma frase", "H1"))
    story.append(P(
        "O AurumOS não tem <b>uma</b> estratégia — são <b>9 módulos independentes</b>, cada um lendo uma "
        "fonte de dado real diferente, competindo por um orçamento de risco comum, decidido por um único "
        "motor central. Nenhum módulo decide executar sozinho; todos só <b>sugerem</b> — o orquestrador é "
        "quem aprova, rejeita ou escala o tamanho.", "Body"))
    story.append(Spacer(1, 6))
    story.append(pipeline_diagram())
    story.append(Spacer(1, 8))

    story.append(P("1.2 — As 9 fontes de dado (o que cada uma vê, de verdade)", "H1"))
    story.append(section_table([
        ["Módulo", "Fonte real", "O que mede"],
        ["Arbitragem", "WS público Bybit spot + Bitget spot", "Diferença de preço entre as duas exchanges"],
        ["Order Flow", "WS público Bybit spot", "Spread interno do book de UMA exchange"],
        ["Pump Exhaustion", "Bybit linear (funding, 24h, OI) + fusão", "Sinais de mercado esticado/exausto"],
        ["Liquidation Hunter", "§MONO§allLiquidation (Bybit)", "Cascatas de liquidação reais"],
        ["Whale Watch", "Nó Ethereum público (Transfer USDT/USDC)", "Movimentação de grandes carteiras"],
        ["News Reactor", "SEC EDGAR + LLM local (Ollama)", "Eventos corporativos classificados por IA local"],
        ["Launch Radar", "Bybit instruments-info + Uniswap V2", "Novos listings CEX/DEX"],
        ["Macro Engine", "Calendário oficial FOMC/CPI/NFP", "Janelas de risco em eventos macro"],
        ["Multi-Asset", "Alpaca (feed IEX)", "Ações US, só leitura, idle sem chave"],
    ], [90, 190, 190]))
    story.append(Spacer(1, 4))
    story.append(P("Cada módulo implementa o mesmo contrato (<font face=\"JetBrainsMono\" size=\"8\">trait "
                    "SignalSource</font>): conecta na fonte real, calcula uma vantagem esperada, e manda pro "
                    "orquestrador via um canal. Nenhum módulo tem acesso ao capital ou pode gastar dinheiro "
                    "sozinho.", "BodySmall"))

    story.append(PageBreak())
    story.append(P("1.3 — Como nasce uma oportunidade: o exemplo do Order Flow", "H1"))
    story.append(bullets([
        "O book da Bybit chega via WebSocket, atualizando várias vezes por segundo.",
        "<b>net_edge = spread do book − 2× taxa maker real</b> (0,08% por perna) — a vantagem já vem líquida de taxa real.",
        "Se o edge passa de um limiar mínimo (calibrado por análise de breakeven), o módulo registra uma hipótese pendente: preço no instante do sinal, aguarda 30 segundos.",
        "Passados os 30s, confere o <b>preço real de novo</b> — só então sabe se foi acerto ou erro de verdade.",
        "Só depois de 50 amostras reais confirmadas o sistema confia nesse edge pra decidir dinheiro de verdade.",
    ]))
    story.append(Spacer(1, 4))
    story.append(confirmation_diagram())
    story.append(Spacer(1, 6))
    story.append(callout_box(
        "A Arbitragem segue a mesma lógica, mas a janela é de 2 segundos (latência real de execução "
        "cross-exchange) e o teste é direto: \"o mesmo spread ainda existia no book de verdade quando uma "
        "ordem teria chegado?\" — testa literalmente o risco de latência, não uma suposição.",
        border_color=CYAN, bg=CYAN_SOFT, label="Arbitragem: mesmo princípio, teste diferente"))

    story.append(PageBreak())
    story.append(P("1.4 — O motor de risco: da sugestão à execução", "H1"))
    story.append(P(
        "Toda oportunidade aprovada passa por 8 filtros em sequência dentro de <font face=\"JetBrainsMono\" "
        "size=\"8\">risk::evaluate</font> — se qualquer um reprovar, a oportunidade é descartada sem executar "
        "nada. É um funil, não uma lista de sugestões independentes.", "Body"))
    story.append(Spacer(1, 4))
    story.append(risk_funnel_diagram())
    story.append(Spacer(1, 8))
    story.append(P("Destaques de dois gates específicos:", "H2"))
    story.append(bullets([
        "<b>Kelly hierárquico</b> — dentro do orçamento que a estratégia já ganhou, o símbolo específico recebe mais "
        "ou menos conforme seu próprio histórico (símbolo novo herda 100% do orçamento; um comprovadamente bom "
        "recebe até 2×; um ruim, até 0,10×).",
        "<b>Guard anti-martingale</b> — nunca aumentar o tamanho da ordem logo após uma perda daquela estratégia. "
        "Comparado contra a perna BASE (estável), não contra o último trade individual — essa mudança corrigiu um "
        "deadlock real (Seção 3.2).",
    ]))

    story.append(P("1.5 — Kill-switch em 4 camadas", "H1"))
    story.append(section_table([
        ["Camada", "Limiar", "Efeito"],
        ["Preventivo", "1% drawdown diário", "Avisa + reduz todas as pernas pela metade (uma vez, na transição)"],
        ["Bloqueio diário", "1,5% drawdown diário", "Para novas execuções até o dia seguinte UTC ou destrave manual"],
        ["Bloqueio semanal", "4% drawdown em 7 dias", "Mesmo mecanismo, janela mais longa"],
        ["Bloqueio total", "7% drawdown desde o pico histórico", "NÃO libera sozinho — exige confirmação manual explícita"],
    ], [100, 160, 210]))
    story.append(Spacer(1, 4))
    story.append(P("Kill-switch manual também existe: criar o arquivo "
                    "<font face=\"JetBrainsMono\" size=\"8\">orchestrator/data/KILL</font> pausa tudo; apagar retoma.", "BodySmall"))

    story.append(P("1.6 — Escalonamento: como a perna de cada estratégia cresce", "H1"))
    story.append(P("A perna só cresce se <b>todas</b> as condições abaixo forem verdadeiras ao mesmo tempo:", "Body"))
    story.append(bullets([
        "Recuperou pelo menos 80% do maior drawdown local que já sofreu.",
        "Passaram pelo menos 50 ciclos <b>e</b> pelo menos 5 minutos reais desde o último escalonamento.",
        "Drawdown do portfólio abaixo de 2%.",
        "Profit factor recente (janela móvel) acima do mínimo configurado.",
        "O resultado continua positivo mesmo excluindo o melhor trade da janela.",
    ]))
    story.append(P("Se tudo passa, o novo tamanho vem do Kelly fracionário medido (30% do Kelly cheio), "
                    "com teto de 30% do equity.", "BodySmall"))

    story.append(PageBreak())
    story.append(P("1.7 — Persistência: o que sobrevive a um restart", "H1"))
    story.append(section_table([
        ["Arquivo", "Conteúdo", "Sobrevive a restart?"],
        ["§MONO§portfolio_state.json", "Equity, perna de cada estratégia, vitórias/derrotas", "§MONO§Sim"],
        ["§MONO§events.jsonl", "Histórico completo de eventos (até 50 mil)", "§MONO§Sim"],
        ["§MONO§edge_scores_*.json", "Edge medido por símbolo (salvo a cada 30s)", "§MONO§Sim"],
        ["§MONO§active_symbols*.json", "Universo de símbolos ativo", "§MONO§Sim"],
        ["confirmation_history (memória)", "Histórico de confirmação real de preço", "§MONO§Não — refila rápido"],
    ], [150, 220, 100]))

    story.append(P("1.8 — Dashboard", "H1"))
    story.append(P(
        "Servidor axum embutido no próprio binário (<font face=\"JetBrainsMono\" size=\"8\">http://127.0.0.1:7878"
        "</font>), WebSocket com replay completo do histórico ao conectar. Mostra: pulso por estratégia, cockpit "
        "de risco conectado ao drawdown real, exposição por estratégia, universo de símbolos com edge ao vivo, "
        "feed combinado de todos os eventos.", "Body"))

    story.append(PageBreak())

    # =====================================================================
    # PARTE 2 — SESSAO
    # =====================================================================
    story.append(section_divider("2", "A sessão de 13/08", "Tudo que foi investigado, corrigido e testado"))
    story.append(Spacer(1, 10))

    story.append(P("2.1 — Linha do tempo", "H1"))
    story.append(bullets([
        "<b>Setup do repositório</b> — GitHub CLI instalado, repositório privado criado, README publicado.",
        "<b>\"Tá tudo rodando normal?\"</b> — auditoria de saúde ao vivo. Achado: Multi-Asset (Alpaca) reconectava "
        "a cada ~35s a noite inteira por não diferenciar mercado fechado de conexão morta.",
        "<b>\"Por que tem perp nesse edge e não está operando?\"</b> — achado: Order Flow gravava spread real do "
        "spot rotulado como se fosse de perpétuo, num mapa que sobrou de uma correção anterior incompleta.",
        "<b>\"Parou de ter trades desde 23h, só arbitragem funcionando\"</b> — a investigação mais longa: um "
        "deadlock permanente no guard anti-martingale, sem erro, sem log visível.",
        "<b>\"Isso é tudo simulado ou real?\"</b> — explicação do que é dado real vs. o que era assumido "
        "(a probabilidade de acerto de cada trade, decidida por sorteio).",
        "<b>\"Se fosse dinheiro real, o que aconteceria? Faça cálculos\"</b> — breakeven real, seleção adversa, "
        "rate limit de API, capital fantasma.",
        "<b>\"Quero real, dentro da realidade, sem nada falso além do dinheiro\"</b> — implementação da camada "
        "de confirmação de preço real, substituindo o sorteio.",
    ]))

    story.append(P("2.2 — Os 6 bugs reais encontrados e corrigidos", "H1"))
    story.append(section_table([
        ["#", "Bug", "Sintoma", "Correção"],
        ["1", "Watchdog do Alpaca não diferenciava mercado fechado de conexão morta", "Reconectava a cada ~35s a noite inteira", "Timeout adaptativo por horário real da NYSE"],
        ["2", "Order Flow gravava edge de spot no mapa de \"perpétuos\"", "Painel mostrava número real com rótulo errado", "Aponta pro mapa correto (spot)"],
        ["3", "Guard anti-martingale comparava contra o último trade individual", "Deadlock permanente — zero trades por horas", "Compara contra a perna base (estável)"],
        ["4", "Escalonamento só contava ciclos, não tempo", "Equity de US$195 a US$2.727 em <2h", "Piso de 5min reais além da contagem de ciclos"],
        ["5", "Resultado de cada trade decidido por sorteio", "Nenhuma validação contra preço real", "Camada de confirmação real de preço"],
        ["6", "Direção da arbitragem (Bybit↔Bitget) calculada e descartada", "Impossível saber onde cada operação aconteceu", "Exposto no campo asset da oportunidade"],
    ], [16, 155, 140, 159]))

    story.append(PageBreak())
    story.append(P("2.3 — Resultado do teste ao vivo pós-mudanças", "H1"))
    story.append(P("Equity resetado a US$200 pra comparação limpa antes de ligar a camada de confirmação real.", "BodySmall"))
    story.append(Spacer(1, 4))
    story.append(metrics_row([
        metric_tile("US$ 974", "equity após ~20min", GOLD_BG),
        metric_tile("90,0%", "acerto real (era 45% chutado)", CYAN_BG),
        metric_tile("7.331", "ciclos executados", GOLD_BG),
        metric_tile("0", "paradas / erros", POSITIVE),
    ]))
    story.append(Spacer(1, 10))
    story.append(bullets([
        "<font face=\"JetBrainsMono\" size=\"8\">leg_size</font> do Order Flow escalou de forma controlada "
        "(US$25 → US$255) ao longo de vários eventos espaçados por pelo menos 5 minutos reais — não mais explosivo.",
        "Arbitragem encontrou seu primeiro cruzamento real do limiar de 0,65% depois de alguns minutos — "
        "comportamento honesto (nem sempre existe), confirmado inspecionando o book real.",
        "Zero erros, dashboard respondendo, processo estável por mais de 20 minutos sem nenhuma parada.",
    ]))

    story.append(P("2.4 — O que ainda não é 100% real", "H1"))
    story.append(callout_box(
        "O modelo agora testa <b>\"o edge sobreviveu ao preço real depois\"</b> — uma mudança real e válida. "
        "O que ele ainda NÃO testa: se uma ordem passiva (maker) realmente teria sido preenchida. Seleção "
        "adversa (quem bate numa ordem parada geralmente sabe algo que você não sabe) e fila de execução real "
        "não estão modeladas — isso só dá pra validar com uma conta de paper trading oficial na própria "
        "exchange, não só com dado de mercado público.",
        border_color=RISK_RED, bg=RISK_SOFT, label="Limitação conhecida, não escondida"))
    story.append(Spacer(1, 6))
    story.append(P(
        "<b>Conclusão honesta:</b> o sistema evoluiu de \"nunca travou de propósito, resultado decidido por "
        "sorteio\" para \"trava corrigida, resultado validado contra preço real subsequente\". O próximo passo "
        "natural, se quiser ir mais fundo em realismo, é conectar uma conta paper oficial de exchange — não pra "
        "arriscar capital, mas pra obter probabilidade de fill real em vez de inferida.", "Body"))

    story.append(Spacer(1, 14))
    story.append(P("Onde encontrar tudo", "H2"))
    story.append(bullets([
        "Código: github.com/Genezera/AurumOS",
        "Relatório de compounding/realismo: docs/relatorio_realismo_13-08-2026.md",
        "Relatório completo (fonte deste PDF): docs/relatorio_completo_13-08-2026.md",
        "Dashboard ao vivo: http://127.0.0.1:7878",
    ], style="BodySmall"))

    doc.multiBuild(story)


if __name__ == "__main__":
    build()
