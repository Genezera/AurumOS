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
    TableStyle, PageBreak, ListFlowable, ListItem, KeepTogether,
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
    canv.drawString(24 * mm, page_h - 115 * mm, "v4.2 — Order Flow corrigido p/ spot real, ranking ao vivo, dashboard detalhado, 12/08/2026")

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
        ["Data do documento", "12 de agosto de 2026 (v4.2 — historico de revisoes na Secao 0 abaixo)"],
        ["Capital inicial de referencia", "US$ 200 (US$100 Bybit + US$100 Bitget) — ver decisao pendente na Secao 8"],
        ["Status atual", "Fase 0 concluida; Fase 1/2 em paper trading com dado real; universo de simbolos, escalonamento e Kelly hierarquico reescritos nesta revisao (Secoes 1.4, 1.5, 9)"],
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
        ["v3.0", "12/08/2026", "3a e 4a auditorias, combinadas: correcao do erro matematico "
         "do breakeven, kill-switch em 4 camadas (depois endurecido: 1,5% diario, trava manual no "
         "nivel total), separacao burn/locker, calibracao do Pump Exhaustion por percentil real "
         "(depois corrigida pra evitar contar o mesmo evento varias vezes), escalonamento mais "
         "rigoroso (PF>=1.2 + robustez a outlier), Fase 11 com exigencia de distribuicao temporal, "
         "universo de simbolos ampliado, cockpit de risco no dashboard, e esta consolidacao de "
         "contradicoes internas que se acumularam entre v2/v2.1/v3.0."],
        ["v4.0", "12/08/2026", "Sessao de pedido explicito do usuario ('quero tudo "
         "funcionando', 'faca a fusao', 'estende a busca ampla'): universo de simbolos deixa de ser "
         "uma lista fixa e passa a ser dois universos dinamicos independentes, medidos por edge real "
         "e rotativos a cada 15min (Secao 1.4) — um para os modulos de perpetuos, outro proprio para "
         "arbitragem (spot x spot, antes usava por engano a lista de perpetuos e so tinha book valido "
         "em 55 dos 220 simbolos). Whale Watch e Liquidation Hunter passam a reforcar a confianca do "
         "Pump Exhaustion via fusao (Secao 1.5), nunca o net_edge. Escalonamento deixa de ser um unico "
         "valor global e passa a ser por estrategia, com recuperacao parcial (80% do drawdown local, "
         "nao mais exigir novo pico do equity inteiro) e sizing continuo por Kelly fracionario (Secao "
         "9). Corrigido um bug estrutural no gatilho do Pump Exhaustion: o criterio '2 de 3 sinais' "
         "nunca disparava porque as metricas nunca cruzavam seus limiares ao mesmo tempo no mesmo "
         "simbolo, mesmo cada uma cruzando o dela centenas de vezes isoladamente (Secao 2.3) — zero "
         "sinais em 24h de operacao real viraram 10 candidatos nos primeiros 90s apos a correcao."],
        ["v4.1", "12/08/2026", "Auditoria externa adicional, mesma sessao: corrigido bug real "
         "onde a reducao de drawdown era aplicada A CADA TRADE (nao so na transicao), colapsando a "
         "perna ate o piso de US$1 sem nunca recuperar sozinha — substituida por um teto nao-destrutivo "
         "em escada (Secao 9.1). Kelly passa a ter uma segunda camada por SIMBOLO dentro de cada "
         "estrategia, nao so por estrategia inteira (Secao 9.1). Removida a tabela fixa de concorrencia "
         "(1/2/3/5 operacoes por faixa de capital) — o proprio orcamento de risco (limite por "
         "estrategia+correlacao+total) agora decide quantas operacoes cabem no mesmo ciclo. Corrigido "
         "o problema mais critico apontado pelo usuario: o edge medido por simbolo (EdgeScores) so "
         "existia em memoria e era zerado a cada restart do processo — com quantos restarts uma sessao "
         "de testes acumula, isso apagava o aprendizado inteiro toda hora (RVNUSDT chegou a mostrar "
         "spread crescendo de 0,49% para 0,64% em 21min antes de um restart apagar o progresso, sem "
         "ter tido tempo de provar se cruzaria o limiar de 0,65%). Agora persiste em disco a cada 15min "
         "e a cada 30s, recarregado no boot. Dashboard: paineis que pareciam quebrados ou desconectados "
         "(pulso de estrategias sem trade, cockpit de risco com caminho de arquivo cru, exposicao "
         "sempre vazia) corrigidos com motivo real explicado em vez de espaco vazio."],
        ["v4.2 (esta)", "12/08/2026", "Order Flow tinha o mesmo bug ja corrigido para Arbitragem na "
         "v4.0 (universo de simbolos da categoria errada), so que em DUAS camadas: conectava no book "
         "SPOT da Bybit mas recebia lista de simbolos do universo LINEAR, e mesmo apos corrigir a fonte "
         "a logica de bootstrap ainda vazava simbolos invalidos da semente original — sete simbolos "
         "(MKRUSDT, FTMUSDT, EOSUSDT, ONEUSDT, ZECUSDT, DASHUSDT, STORJUSDT) que existem como perpetuo "
         "mas nao como par spot ficavam presos em zero amostras para sempre (Secao 1.4.1). Corrigidas "
         "as duas camadas; cobertura de simbolos com amostra real no universo de perpetuos mais que "
         "triplicou (60 -> 208) na primeira rotacao apos a correcao. Ranking exibido no dashboard "
         "(media/amostras por simbolo) passa a reemitir a cada 5 segundos entre rotacoes de 15min, em "
         "vez de só na propria rotacao — esclarecimento importante registrado: a DECISAO de operar ja "
         "era 100% em tempo real desde sempre, só a EXIBICAO estava atrasada (Secao 1.4.1). Detalhamento "
         "completo das correcoes de dashboard da v4.1 (pulso de estrategias, cockpit de risco, "
         "exposicao por estrategia) na nova Secao 11.1."],
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
        "aprovado apenas se: exposicao_atual + capital_at_risk &lt;= strategy_limit",
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

    story.append(P("1.4 Universo de simbolos dinamico (12/08/2026)", "H2"))
    story.append(P(
        "Pedido explicito do usuario: \"nao quero que fique travado no mesmo, quero uma analise "
        "inteira do mercado inteiro buscando por oportunidades... quero que aprenda... pesquise o "
        "mercado inteiro globalmente, uma pool extremamente maior\". Ate esta revisao, os simbolos "
        "monitorados eram uma lista fixa escolhida a mao (<font face='JetBrainsMono'>WIDE_SYMBOLS</font>, "
        "70 pares). Agora existe um modulo (<font face='JetBrainsMono'>symbol_universe.rs</font>) que "
        "roda em segundo plano e se recalcula sozinho:", "Body"))
    story.append(bullets([
        "<b>Edge medido, nao escolhido a dedo</b> — cada modulo que toca book real "
        "(<font face='JetBrainsMono'>Order Flow</font> para o universo de perpetuos, "
        "<font face='JetBrainsMono'>Arbitragem</font> para o proprio) grava o net_edge observado a "
        "cada tick numa janela movel de 500 amostras por simbolo (<font face='JetBrainsMono'>EdgeStats</font>).",
        "<b>\"Comprovados\" vs. \"exploracao\"</b> — simbolos com pelo menos 200 amostras E media "
        "positiva entram na lista ativa com prioridade; o resto das vagas (minimo 50) gira "
        "continuamente por candidatos novos do pool completo, por ordem de volume 24h na exchange — "
        "nunca uma lista fixa, e um simbolo comprovado que piorar sai sozinho da lista no proximo "
        "ciclo quando a media da janela movel decair.",
        "<b>Rotacao a cada 15 minutos</b> (ate esta revisao, a cada 2h) — reavalia quem esta com edge "
        "real muito mais rapido; um par que esfriar libera vaga pra exploracao em minutos, nao horas.",
        "<b>Universo alvo de 220 simbolos ativos</b> (antes 80), escolhidos de um pool de candidatos "
        "de ate 450 (antes 250) — cobre a maior parte do mercado liquido da exchange em vez de uma "
        "fatia pequena escolhida a mao.",
    ]))
    story.append(callout_box(
        "<b>Universo separado por mercado, nao um so compartilhado:</b> a primeira versao desta ideia "
        "usava a MESMA lista dinamica (baseada em perpetuos, categoria linear da Bybit) para todos os "
        "modulos, inclusive Arbitragem. Isso era um erro silencioso: Arbitragem compara SPOT contra "
        "SPOT (Bybit x Bitget), e a lista de perpetuos inclui simbolos sinteticos (acoes/commodities "
        "tokenizados como TSLAUSDT, AAPLUSDT, XAUUSDT) sem par spot em nenhuma das duas exchanges — "
        "so 55 dos 220 simbolos ativos tinham book valido nas duas pontas ao mesmo tempo. Corrigido: "
        "Arbitragem agora tem seu proprio universo dinamico, construido a partir de pares SPOT reais "
        "da Bybit (categoria spot, ate 450 candidatos) e alimentado pelo proprio spread cross-exchange "
        "medido a cada tick, nao pelo edge de book unico do Order Flow. Efeito medido: o melhor edge "
        "de arbitragem observado foi de -0,055% (negativo, lista antiga) para +0,110% (MOVEUSDT, lista "
        "nova) em poucos minutos apos a correcao.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(P(
        "Persistido em disco para observabilidade (<font face='JetBrainsMono'>data/active_symbols.json</font> "
        "para perpetuos, <font face='JetBrainsMono'>data/active_symbols_spot.json</font> para spot) e "
        "visivel ao vivo no dashboard, painel \"Universo de simbolos\", dividido nos dois mercados. "
        "<b>Isto amplia a busca sem inflar edge artificialmente</b> — nenhum limiar de decisao (Secao "
        "7.1) foi tocado; o que mudou e quanto do mercado real o sistema efetivamente olha antes de "
        "decidir.", "Body"))

    story.append(P("1.4.1 Order Flow tambem preso em simbolos inexistentes — mesmo bug, duas camadas (12/08/2026)", "H2"))
    story.append(P(
        "Pergunta direta do usuario ao notar varios simbolos travados em \"0 amostras\" para sempre no "
        "ranking do universo de perpetuos (nao so os recem-explorados, que ainda nao tiveram tempo — "
        "esses especificos nunca mudavam): \"porque os outros simbolos nao estao mudando?\". Causa raiz, "
        "confirmada direto na API da propria Bybit: Order Flow conecta no book <b>SPOT</b> da Bybit para "
        "medir edge, mas recebia a lista de simbolos vinda do universo <b>LINEAR</b> (perpetuos) — sete "
        "simbolos da semente original (MKRUSDT, FTMUSDT, EOSUSDT, ONEUSDT, ZECUSDT, DASHUSDT, STORJUSDT) "
        "existem como perpetuo mas <b>nao existem como par spot</b>. Nao era \"ainda nao mediu\" — era "
        "\"nunca vai medir\", estruturalmente. Mesma classe de bug ja corrigida para Arbitragem na Secao "
        "1.4 (universo compartilhado por engano entre mercados diferentes).", "Body"))
    story.append(P(
        "Corrigido em duas camadas, a segunda achada só ao verificar a primeira ao vivo:", "Body"))
    story.append(bullets([
        "<b>Fonte do simbolo</b> — Order Flow passou a usar o mesmo universo dinamico spot que a "
        "Arbitragem ja usa, em vez do universo linear.",
        "<b>A semente do \"primeiro ciclo\" ainda vazava simbolos invalidos</b> — mesmo depois da "
        "correcao acima, os sete simbolos continuaram aparecendo na assinatura do Order Flow. Causa: a "
        "logica de bootstrap de <font face='JetBrainsMono'>rotate()</font> forcava a semente inteira "
        "(pensada para perpetuos) dentro de QUALQUER universo no primeiro ciclo, sem checar se aquele "
        "simbolo existe de verdade na categoria — inclusive no universo spot. Corrigido: só entra da "
        "semente o que esta presente no pool de candidatos REAL desta categoria (vindo da propria "
        "Bybit).",
    ]))
    story.append(callout_box(
        "<b>Efeito medido ao vivo:</b> cobertura de simbolos com amostra real no universo de perpetuos "
        "mais que triplicou (60 -&gt; 208 simbolos) na primeira rotacao apos a correcao — o Order Flow "
        "estava, sem saber, gritando no vazio para sete simbolos o tempo todo em vez de medir o resto do "
        "mercado.",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
    ))
    story.append(P(
        "Tambem nesta revisao: o RANKING exibido no dashboard (media/amostras por simbolo) passou a "
        "reemitir a cada 5 segundos em vez de só a cada rotacao de 15 minutos — pergunta do usuario ao "
        "ver o numero do GRTUSDT mudar: \"nao tem como ver o edge de todos os ativos em tempo real?\". "
        "Esclarecimento: a DECISAO de operar ja era 100% em tempo real desde sempre (net_edge calculado "
        "a cada tick de book recebido, comparado ao limiar na hora, independente desta tabela) — só a "
        "EXIBICAO estava atrasada. A lista de quem fica ativo continua trocando só a cada 15min, de "
        "proposito (evita promover/rebaixar um simbolo por causa de um tick ruidoso isolado); só o "
        "numero mostrado ficou mais vivo.", "Body"))

    story.append(P("1.5 Fusao entre modulos (12/08/2026)", "H2"))
    story.append(P(
        "Pedido explicito do usuario: \"faca intercomunicacao\", com o exemplo \"pump + depositos de "
        "baleias em exchange + liquidacoes compradoras crescendo = candidato forte de exaustao\". "
        "Whale Watch e Liquidation Hunter ja coletavam dado real do mesmo mercado mas ficavam "
        "isolados — cada um so alimentava seu proprio feed do dashboard, sem afetar a decisao de "
        "nenhum outro modulo. Implementado em <font face='JetBrainsMono'>fusion.rs</font>: dois "
        "quadros compartilhados em memoria (<font face='JetBrainsMono'>LiquidationBoard</font>, "
        "<font face='JetBrainsMono'>WhaleBoard</font>) que Liquidation Hunter e Whale Watch escrevem "
        "e o Pump Exhaustion le como reforco.", "Body"))
    story.append(callout_box(
        "<b>Separacao deliberada, para nao violar a Secao 0:</b> um reforco de fusao (cascata de "
        "liquidacao de longs no mesmo simbolo nos ultimos 15min, ou pressao agregada de deposito em "
        "exchange acima de US$2M/30min) so pode aumentar a <i>confianca</i> do candidato (ate +30%, "
        "nunca acima de 0,95) — nunca o <font face='JetBrainsMono'>net_edge</font>, que continua vindo "
        "exclusivamente da camada de confirmacao de preco medida (Secao 2.1). Um sinal de outro modulo "
        "nunca inventa vantagem numerica; so ajusta o quanto o sistema confia numa vantagem ja medida.",
        border_color=CYAN, bg=colors.HexColor("#E7F8FC"),
    ))

    # PageBreak forcado removido (polimento visual, 13/08/2026): deixava a
    # pagina anterior com mais de 600pt de espaco vazio (~80% da pagina)
    # quando o conteudo da Secao 1 terminava cedo — o titulo + paragrafo de
    # abertura da Secao 2 abaixo (protegidos por KeepTogether) agora fluem
    # naturalmente pro espaco que sobrar; o diagrama de maturidade, sendo um
    # elemento atomico, pula pra pagina seguinte sozinho se nao couber.
    # ------------------------------------------------------------- Sec2 (maturidade)
    story.append(KeepTogether([
        P("2. Modelo de maturidade por estrategia", "H1"),
        P(
            "A v1 deste roadmap organizava tudo em fases estritamente sequenciais (Fase 1 -> Fase 2 -> "
            "... -> Fase 12), o que sugeria — incorretamente — que nenhuma estrategia poderia chegar perto "
            "de capital real ate o sistema inteiro estar pronto. Isso nao combina com o objetivo de "
            "acelerar sem cortar caminho na seguranca. A partir desta revisao, cada estrategia avanca "
            "numa escada de maturidade <b>independente</b>:", "Body"),
    ]))
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
        ["Arbitragem", "Paper (dado real) — universo spot proprio (Secao 1.4), sem operar ainda", "melhor edge observado subiu de -0,055% para +0,110% (MOVEUSDT) apos a correcao de universo; ainda abaixo do limiar de 0,65% (Secao 7.1)"],
        ["Order Flow", "Paper (dado real) — motor de base, opera continuamente", "unica estrategia com volume de operacoes alto e continuo; GRTUSDT oscila perto do limiar de 0,45%, cruza intermitentemente"],
        ["Pump Exhaustion", "Paper — gatilho corrigido (Secao 2.3), emitindo candidatos reais; amostra de confirmacao ainda insuficiente", "0 sinais em 24h antes da correcao -> 10 candidatos nos primeiros 90s depois; net_edge continua 0 ate 20 confirmacoes de preco reais acumularem"],
        ["Whale Watch", "Observacao + reforco de fusao (Secao 1.5)", "net_edge=0 por design; alimenta o WhaleBoard que reforca a confianca do Pump Exhaustion; 11 enderecos de exchange rotulados (Secao 12.2)"],
        ["News Reactor", "Observacao — classificacao real ativa", "net_edge=0 por design (falta confirmacao de preco); classificador via Ollama local (Secao 12.1) gerando direction/confidence reais"],
        ["Macro Engine", "Pesquisa/Observacao — dormente por design", "calendario real carregado (FOMC/CPI/NFP); so emite dentro de uma janela de -5min/+15min ao redor do horario oficial do evento — quieto o resto do tempo, nao e falha"],
        ["Launch Radar (CEX+DEX)", "Observacao — checklist parcial", "cobre EVM/Uniswap V2, holders, LP queimado, candidato a sybil; so dispara quando um par NOVO e listado (raro por natureza) — gaps documentados na Secao 4"],
        ["Liquidation Hunter", "Observacao + reforco de fusao (Secao 1.5)", "net_edge=0 por design; alimenta o LiquidationBoard que reforca a confianca do Pump Exhaustion"],
        ["Multi-Asset Volatility", "Ativa — observacao, dormente fora do horario de pregao", "conectada a Alpaca (dado real IEX); feed so envia trade quando a bolsa de NY esta aberta (9:30-16h ET) — reconecta em loop fora desse horario, nao e falha; net_edge=0 por design (Secao 12.3)"],
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

    story.append(P("2.2 Limiares de gatilho recalibrados por percentil real (12/08/2026)", "H2"))
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
        "de OI em 1h &gt;1%. Combinados (&gt;=2 de 3), isso ocorre em 0,35% dos snapshots observados "
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

    story.append(P("2.3 Correção estrutural do gatilho — a recalibração da Seção 2.2 não bastava (12/08/2026)", "H2"))
    story.append(P(
        "A recalibração acima corrigiu os TRÊS limiares individuais pelo percentil real de cada "
        "métrica isolada, mas manteve a lógica de gatilho original: exigir que pelo menos 2 das 3 "
        "métricas (funding, pump 24h, crescimento de OI) cruzassem seu limiar <b>ao mesmo tempo, no "
        "mesmo símbolo</b>. Mesmo depois da recalibração, o sistema continuou emitindo <b>zero sinais "
        "durante 24h de operação real</b> — inclusive com o universo de símbolos já ampliado para 87 "
        "e depois 220 (Seção 1.4). O diagnóstico honesto disso, feito analisando 3.000 amostras reais "
        "recentes (<font face='JetBrainsMono'>raw_pump_exhaustion.jsonl</font>): funding cruzou seu "
        "limiar sozinho 249 vezes, pump 24h cruzou o dele sozinho 175 vezes — mas os dois <b>nunca "
        "coocorreram no mesmo símbolo, nem uma vez</b>. O gate não era raro; era estruturalmente "
        "inatingível — a suposição implícita de que os sinais coocorrem simplesmente não é verdade "
        "no dado real.", "Body"))
    story.append(P(
        "Correção: <font face='JetBrainsMono'>exhaustion_score()</font> já calculava a combinação "
        "ponderada contínua das 3 dimensões (usada só para o heartbeat do dashboard até então) — a "
        "distribuição real desse score, medida nas mesmas 3.000 amostras, tem espalhamento genuíno "
        "(p50=0,27, p75=0,43, p90=0,58, p95=0,72, p99=0,91). O gatilho passou a usar esse score "
        "diretamente, calibrado no seu próprio percentil observado (&gt;=0,60 dispara sozinho; "
        "&gt;=0,40 dispara combinado com reforço de fusão — Seção 1.5), em vez de exigir dois "
        "cruzamentos discretos que na prática nunca se encontravam.", "Body"))
    story.append(callout_box(
        "<b>Resultado verificado ao vivo, não estimado:</b> 10 candidatos detectados nos primeiros 90 "
        "segundos após o restart com a correção — contra zero em 24h de operação antes dela. Isso não "
        "significa lucro — o net_edge continua 0,0 até 20 confirmações de preço reais acumularem "
        "(Seção 2.1), e a amostra desta correção ainda é de minutos, não dias. É a diferença entre "
        "\"o detector não encontra o padrão\" e \"o gate nunca poderia disparar, independente do "
        "mercado\" — a segunda era o problema real.",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
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
    story.append(callout_box(
        "<b>Honestidade sobre quais destes já rodam de verdade (13/08/2026) — esta lista é um "
        "alvo, não uma descrição do código atual:</b> IMPLEMENTADOS e rodando em produção — profit "
        "factor mínimo (com hierarquia de 2 níveis, Seção 9.1), drawdown máximo (kill-switch + teto "
        "não-destrutivo, Seção 9.2), número mínimo de operações por classe de estratégia (Seção 8.2), "
        "e resultado não dependente do melhor trade (<font face='JetBrainsMono'>risk::"
        "positive_excluding_best_trade</font>). NÃO IMPLEMENTADOS ainda, continuam só nesta lista de "
        "alvo — CVaR/expected shortfall da cauda de perdas, simulação Monte Carlo de reordenação, "
        "desempenho fora da amostra (validação out-of-sample formal) e teste de estresse com latência/"
        "slippage maiores que o observado. O que a <font face='JetBrainsMono'>score()</font> real do "
        "sistema calcula hoje (Seção 1.2) é bem mais simples que essa lista inteira: "
        "<font face='JetBrainsMono'>net_edge&#215;confidence &#247; max_loss_pct &#247; capital_needed "
        "&#247; horas_de_holding</font> — um ranking de prioridade entre candidatos já aprovados pelo "
        "motor de risco, não um score de aprovação com CVaR ou probabilidade de execução embutidos. "
        "Confundir os dois (o score que RANQUEIA com os critérios que APROVAM promoção de estágio) foi "
        "um erro de revisões anteriores deste documento — corrigido aqui.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))

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
        "<b>Correcao de terminologia (revisao tecnica externa, 12/08/2026):</b> a v2.1 tratava "
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
        "events.jsonl</font> (134.493 eventos acumulados) via <font face='JetBrainsMono'>backtests/"
        "live_performance_report.py</font>, do backtest historico via "
        "<font face='JetBrainsMono'>backtests/pump_exhaustion_historical.py</font> contra dado real "
        "da Bybit, e da analise de breakeven via "
        "<font face='JetBrainsMono'>backtests/edge_threshold_analysis.py</font> (Secao 7.1):", "Body"))
    story.append(section_table([
        ["Estrategia", "Operacoes", "Taxa de acerto", "PnL liquido", "Leitura"],
        ["Arbitragem", "3.356", "87,9%", "+US$1.002,79", "§MONO§positivo, razao risco/retorno 1,70"],
        ["Order Flow", "19.059", "90,9%", "+US$2.268,81", "§MONO§positivo, razao risco/retorno 2,07"],
        ["Pump Exhaustion", "0", "—", "US$0,00", "§MONO§ainda sem trade (net_edge=0 ate acumular amostra de confirmacao — Secao 2.1)"],
    ], [90, 75, 90, 90, 145]))
    story.append(Spacer(1, 6))
    story.append(callout_box(
        "<b>Isto nao e a mesma medicao da v2 deste documento</b> (que mostrava 56,3%/-US$3,96 em "
        "arbitragem e 42,3%/-US$0,21 em order flow — o padrao que a Secao 3.1 existe pra prevenir: "
        "acerto alto e ainda assim prejuizo, porque as perdas eram maiores que os ganhos em media). "
        "Aquele numero era de ANTES da correcao de breakeven (Secao 7.1, MIN_NET_EDGE elevado acima "
        "do breakeven teorico) e de antes de toda a rodada de realismo de execucao desta revisao "
        "(Secao 12.5 — exposicao simultanea real, reamostragem por bootstrap em vez de sorteio "
        "formulaico, deduplicacao de sinais sobrepostos). Os numeros acima sao positivos, mas vem de "
        "um log ACUMULADO desde o inicio do desenvolvimento — misturam ciclos de codigo antigos "
        "(round-trip instantaneo, sem rate limit, sem notional minimo real) com os ciclos mais "
        "recentes e mais realistas. Nao e uma medicao limpa de 'quanto o sistema ganha desde que "
        "ficou 100% real' — e por isso que o criterio da Fase 11 (Secao 8.2) precisa contar a janela "
        "de validacao a PARTIR de agora, nao reaproveitar este historico misto.",
        border_color=GOLD, bg=GOLD_SOFT,
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
        "<b>Correcao de um erro de calculo (revisao tecnica externa, 12/08/2026):</b> a v2.1 "
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
        "<b>Pump Exhaustion — recalibrado por percentil real, gatilho corrigido (13/08/2026):</b> os "
        "limiares descritos nas revisoes anteriores deste documento (funding &gt;0,10%/8h E variacao "
        "de 24h &gt;+15%, simultaneos — o \"gatilho em producao\" citado ali) nunca chegaram a rodar de "
        "verdade: analisando 17h de dado bruto acumulado (30 simbolos), pump nunca passou de 7,5% nem "
        "OI de 5,2% no periodo — os limiares estavam calibrados pra um regime bem mais volatil do que "
        "o observado, e o criterio \"2 de 3 sinais simultaneos\" partia de uma premissa que os proprios "
        "dados contradiziam: em 3.000 amostras, funding cruzou o limiar 249 vezes, pump 175 vezes, OI 4 "
        "vezes, mas NUNCA dois ao mesmo tempo no mesmo simbolo. O gatilho nao era so raro, era "
        "estruturalmente inatingivel.", "Body"))
    story.append(Spacer(1, 4))
    story.append(P(
        "Substituido por um gatilho continuo baseado no proprio score ponderado das 3 dimensoes "
        "(<font face='JetBrainsMono'>exhaustion_score</font>), calibrado no percentil real observado "
        "da propria distribuicao (nao um numero escolhido a dedo): dispara sozinho acima de "
        "p90 (score &gt;= 0,60), ou combinado com reforco de fusao (Liquidation Hunter/Whale Watch) "
        "acima de p75 (score &gt;= 0,40). Limiares individuais tambem recalibrados pro percentil real: "
        "funding &gt;0,015%/8h (era 0,10%) e pump 24h &gt;3% (era 15%). Verificado ao vivo: 0 sinais em "
        "17h antes da correcao, dezenas de candidatos nos primeiros minutos depois. O modulo agora "
        "tambem tem a mesma camada de confirmacao de preco real das outras duas estrategias que "
        "executam trade (Order Flow/Arbitragem) — inclusive a mesma taxa de execucao descontada no "
        "edge medido e o mesmo sorteio por bootstrap de desfecho real (Secao 12.5) — mas ainda esta "
        "acumulando as 20 amostras confirmadas minimas antes de net_edge sair de 0.0; 0 trades "
        "executados ate a medicao mais recente (Secao 7).", "Body"))

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
        "<b>Recomendacao externa recebida (12/08/2026), coerente com a Opcao A:</b> US$100 na Bybit + "
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
        "Nao sao mais placeholder generico — mas tambem nao sao mais um numero UNICO pra todas as "
        "estrategias. Revisao tecnica externa (13/08/2026): \"um requisito unico de 150 operacoes e "
        "pouco pra estrategias rapidas e talvez inalcancavel pra eventos raros — cada estrategia "
        "precisa de criterio proprio\", implementado em "
        "<font face='JetBrainsMono'>backtests/fase11_progress.py::STRATEGY_CRITERIA</font>:", "Body"))
    story.append(section_table([
        ["Classe", "Estrategias", "Operacoes minimas", "Profit factor minimo", "Semanas distintas minimas"],
        ["Continua (alto volume)", "Arbitragem, Order Flow", "150", "1,3", "4"],
        ["Evento raro c/ confirmacao", "Pump Exhaustion", "20 (mesmo piso de MIN_CONFIRMATION_SAMPLES)", "1,1", "6"],
        ["Observacional (net_edge=0)", "Whale Watch, News, Macro, Launch Radar, Liquidation Hunter, Multi-Asset", "sem gate — nunca executam", "—", "—"],
    ], [110, 120, 105, 75, 80]))
    story.append(Spacer(1, 4))
    story.append(P(
        "<b>Por que \"semanas distintas\", nao so contagem bruta:</b> 2ª revisao tecnica externa "
        "(13/08/2026) — 20 amostras que acontecem todas na MESMA semana (ou pior, no mesmo dia) nao "
        "provam nada sobre regimes de mercado diferentes. Estrategias continuas ja cobrem varias "
        "semanas so pelo volume; estrategias raras precisam do minimo EXPLICITO de semanas com pelo "
        "menos 1 amostra cada.", "Body"))
    story.append(bullets([
        "Minimo 4 semanas consecutivas de paper trading com dado 100% real, sem interrupcao nao planejada acima de 24h (piso do relogio da Fase 11, independente do criterio por classe acima).",
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
    story.append(P("Espelha exatamente <font face='JetBrainsMono'>orchestrator/config/risk.toml</font> em producao (atualizado 13/08/2026, pos-auditoria externa de realismo de execucao — ver Secao 12.5):", "Body"))
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
        ["leverage.liquid_max / launch_max", "2,0x / 1,0x", "Alavancagem maxima de UMA oportunidade isolada, por classe (Secao 12.5)"],
        ["leverage.global_max", "1,0x (teto absoluto no codigo: 2,0x)", "NOVO (13/08/2026) — teto de alavancagem AGREGADA de todas as posicoes abertas ao mesmo tempo, somada sobre o equity; nunca configuravel acima do teto absoluto (Secao 12.5)"],
        ["min_cycles_between_scale", "50 ciclos", "Ciclos minimos entre ajustes de tamanho de perna (por estrategia — Secao 9.1)"],
        ["(constante no codigo) MIN_TIME_BETWEEN_SCALE", "300s", "Piso de TEMPO REAL entre ajustes, alem da contagem de ciclos — achado ao vivo: com centenas de ciclos/min, 50 ciclos passam em segundos e o Kelly comprimia sobre si mesmo repetidas vezes por minuto (Secao 9.1)"],
        ["scale_growth_factor", "1,15 (+15%)", "Degrau fixo antigo — agora so usado como reserva quando a amostra ainda nao permite calcular Kelly (Secao 9.1)"],
        ["scale_shrink_factor", "0,80 (&#8722;20%)", "NOVO (13/08/2026) — reducao ativa da perna quando o profit factor recente degrada abaixo do piso de \"continuar\" (Secao 9.1)"],
        ["min_recent_profit_factor_for_scale / to_hold", "1,3 / 1,2", "Hierarquia de 2 niveis (13/08/2026): PF&gt;=1,3 autoriza crescer; PF&lt;1,2 forca reducao ativa; entre os dois, mantem o tamanho (Secao 9.1)"],
        ["partial_recovery_fraction", "80%", "Fracao do maior drawdown LOCAL da propria estrategia que precisa estar recuperada pra liberar CRESCIMENTO (nao bloqueia reducao — Secao 9.1)"],
        ["kelly_safety_fraction", "30%", "Fracao do Kelly cheio realmente aplicada ao tamanho da perna — desde 13/08/2026, a propria taxa de acerto usada no calculo e um limite inferior de confianca (Wilson, 95%), nao a taxa pontual (Secao 9.1)"],
        ["drawdown_halve_threshold_pct", "2%", "Drawdown do PORTFOLIO INTEIRO a partir do qual a perna de TODAS as estrategias e reduzida pela metade — continua global de proposito (Secao 9.1)"],
        ["max_leg_fraction_of_equity", "30%", "Fracao maxima do equity total que uma unica perna pode representar"],
    ], [190, 110, 190]))
    story.append(Spacer(1, 4))
    story.append(callout_box(
        "Tres parametros NAO ficam em risk.toml, sao constantes fixas no codigo (documentadas aqui "
        "mesmo assim, porque tambem sao \"parametro de risco em producao\"): "
        "<font face='JetBrainsMono'>MIN_TIME_BETWEEN_SCALE</font> (300s, risk.rs), "
        "<font face='JetBrainsMono'>ABSOLUTE_LEVERAGE_CEILING</font> (2,0x, risk.rs) e "
        "<font face='JetBrainsMono'>LAUNCH_TRADE_ENABLED</font> (false — trava estrutural do Launch "
        "Radar, independente do gate de score; so muda pra true com decisao explicita, nunca como "
        "efeito colateral de outra mudanca).",
        border_color=GOLD, bg=GOLD_SOFT,
    ))
    story.append(callout_box(
        "<b>Nota de correcao (v1 -> v2):</b> a v1 deste documento descrevia o escalonamento como "
        "'dobra perna em nova maxima + 50 ciclos + drawdown baixo' na secao da Fase 0. Isso nunca foi "
        "o que o codigo faz — era uma descricao desatualizada de uma ideia descartada ainda na "
        "conversa de planejamento. O motor de risco (<font face='JetBrainsMono'>risk.rs::maybe_scale</font>) "
        "sempre usou <font face='JetBrainsMono'>scale_growth_factor</font> configuravel — a tabela acima "
        "e a fonte da verdade a partir de agora.",
        border_color=POSITIVE, bg=colors.HexColor("#E4F5EE"),
    ))

    story.append(P("9.1 Escalonamento por estrategia — Kelly fracionario e recuperacao parcial (12/08/2026)", "H2"))
    story.append(P(
        "Ate esta revisao, o motor de risco tinha um unico <font face='JetBrainsMono'>leg_size</font> "
        "global, um unico historico de PnL recente e um unico contador de ciclos — compartilhados por "
        "TODAS as estrategias. Isso significava que uma estrategia ruim podia travar o escalonamento "
        "de uma boa, e vice-versa, alem de exigir que o <b>equity do portfolio inteiro</b> batesse novo "
        "pico historico antes de qualquer aumento de perna — mesmo que uma estrategia especifica ja "
        "estivesse consistentemente lucrativa havia tempo. Reescrito em tres partes, pedidas "
        "explicitamente pelo usuario:", "Body"))
    story.append(bullets([
        "<b>Estado por estrategia</b> — cada uma agora rastreia seu proprio PnL acumulado, pico, vale "
        "e janela de trades recentes (<font face='JetBrainsMono'>risk::StrategyScaling</font>). Profit "
        "factor e robustez-sem-melhor-trade (Secao 12.7) passam a ser medidos por estrategia, nao mais "
        "misturados.",
        "<b>Recuperacao parcial, nao pico absoluto</b> — o gate antigo exigia "
        "<font face='JetBrainsMono'>equity &gt;= peak_equity</font> do portfolio inteiro. Agora basta "
        "recuperar 80% (<font face='JetBrainsMono'>partial_recovery_fraction</font>) do maior drawdown "
        "LOCAL da propria estrategia (pico-&gt;vale de PnL acumulado dela) — uma estrategia "
        "individualmente boa nao fica mais refem de outra que ainda nao recuperou.",
        "<b>Sizing continuo por Kelly fracionario</b> — o degrau fixo de +15% e substituido por um "
        "tamanho recalculado a cada escalonamento a partir da fracao de Kelly medida na janela recente "
        "da propria estrategia (<font face='JetBrainsMono'>kelly = taxa_de_acerto &#8722; "
        "(1&#8722;taxa_de_acerto)/payoff_ratio</font>), aplicando so 30% do Kelly cheio "
        "(<font face='JetBrainsMono'>kelly_safety_fraction</font> — Kelly cheio assume taxa de "
        "acerto/payoff exatos e conhecidos, o que nunca e o caso com amostra finita). O degrau fixo "
        "antigo vira so um reserva para quando a amostra ainda nao permite estimar Kelly.",
    ]))
    story.append(callout_box(
        "<b>A protecao de emergencia continua global, de proposito:</b> quando o drawdown do "
        "PORTFOLIO INTEIRO (nao de uma estrategia) ultrapassa certos limiares, o tamanho EFETIVO da "
        "ordem de TODAS as estrategias e reduzido de uma vez (ver Secao 9.2 — mecanismo corrigido "
        "depois desta revisao inicial). Um drawdown grande do portfolio e sinal de que todo o sistema "
        "deve operar menor agora, nao so a estrategia que \"causou\" — nenhuma delas tem como saber "
        "sozinha que o portfolio inteiro esta em apuros.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(P("9.2 Correcao: reducao de drawdown parou de colapsar a perna ate o piso (auditoria externa, 12/08/2026)", "H2"))
    story.append(P(
        "Achado por auditoria tecnica externa e confirmado lendo o codigo: a implementacao inicial da "
        "Secao 9.1 chamava a reducao de emergencia (halve pela metade) dentro de "
        "<font face='JetBrainsMono'>maybe_scale</font>, que roda A CADA TRADE — nao so na transicao de "
        "\"abaixo do limiar\" para \"acima do limiar\". Cada trade novo enquanto o drawdown ficasse "
        "&gt;=2% reduzia a perna PELA METADE DE NOVO, nao uma vez so — uma unica reducao real levaria "
        "US$25 para US$12,50, nao para o piso de US$1 observado. Era exatamente esse loop repetido que "
        "colapsava tudo, e sem nenhuma escada de recuperacao, ficava preso no piso indefinidamente "
        "mesmo com o drawdown oscilando perto do limiar por horas.", "Body"))
    story.append(P(
        "Corrigido com um teto NAO-DESTRUTIVO: em vez de mutar <font face='JetBrainsMono'>leg_size</font> "
        "permanentemente, o tamanho EFETIVO da ordem e multiplicado por um fator recalculado do zero a "
        "cada avaliacao, puramente em funcao do drawdown ATUAL — nunca acumula, nunca precisa perguntar "
        "\"ja reduzi essa vez?\":", "Body"))
    story.append(section_table([
        ["Drawdown do portfolio (desde o pico)", "Teto sobre o tamanho efetivo da ordem"],
        ["&gt;= 1,75%", "50%"],
        ["&gt;= 1,50%", "65%"],
        ["&gt;= 1,00%", "80%"],
        ["< 1,00%", "sem teto (100%)"],
    ], [280, 210]))
    story.append(Spacer(1, 4))
    story.append(P(
        "Por ser uma funcao pura do drawdown corrente, relaxa sozinha assim que o portfolio recupera — "
        "sem exigir estado extra nem logica de recuperacao separada. O <font face='JetBrainsMono'>"
        "leg_size</font> armazenado por estrategia (o que o Kelly fracionario constroi) nunca e mais "
        "destruido por drawdown; so o quanto dele pode ser usado AGORA fica temporariamente menor.", "Body"))
    story.append(callout_box(
        "Estes valores de escada sao fixos no codigo por enquanto (nao expostos em risk.toml ainda) — "
        "escolhidos pela auditoria externa, nao calibrados contra dado real de quanto tempo o "
        "portfolio efetivamente passa em cada faixa. Proximo passo natural se este mecanismo mostrar "
        "comportamento estranho: tornar configuravel e revisitar os limiares com mais historico.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(P("9.3 Kelly hierarquico — segunda camada por simbolo (auditoria externa, 12/08/2026)", "H2"))
    story.append(P(
        "Pedido da auditoria: \"direciona mais capital as melhores estrategias, simbolos e regimes\". "
        "Escopo desta revisao: as duas primeiras camadas (portfolio-&gt;estrategia-&gt;SIMBOLO) — a "
        "camada de \"regime\" (classificacao de regime de mercado — tendencia, volatilidade) fica de "
        "fora por enquanto, nao existe nenhuma infraestrutura de deteccao de regime ainda e construir "
        "isso agora seria fabricar sofisticacao sem base real medida.", "Body"))
    story.append(P(
        "Antes, dois simbolos dentro da mesma estrategia (ex.: QTUMUSDT e GRTUSDT dentro de Order Flow) "
        "recebiam exatamente o mesmo tamanho de ordem, mesmo com desempenho historico diferente. Agora "
        "<font face='JetBrainsMono'>risk::SymbolScaling</font> rastreia PnL recente por (estrategia, "
        "simbolo); o tamanho efetivo da ordem e multiplicado por uma fracao de alocacao — a razao entre "
        "o Kelly medido DESSE simbolo (ponderado pela amostra: 0 amostras = 100% de heranca do prior da "
        "estrategia, 50+ amostras = quase todo peso no Kelly proprio) e o Kelly da estrategia inteira, "
        "limitada a [0,10, 2,00] pra nunca zerar nem inflar um simbolo alem do razoavel a partir de "
        "amostra ainda ruidosa. Simbolo novo ou pouco visto nunca comeca travado — herda o orcamento da "
        "estrategia ate provar diferenca propria.", "Body"))

    story.append(P("9.4 Concorrencia por orcamento de risco, nao tabela fixa (auditoria externa, 12/08/2026)", "H2"))
    story.append(P(
        "A tabela fixa antiga (1 operacao ate US$499, 2 de US$500-999, 3 de US$1.000-2.499, 5 acima "
        "disso) limitava artificialmente quantas oportunidades DIFERENTES podiam virar ordem no mesmo "
        "ciclo, mesmo quando o orcamento de risco real (limite por estrategia + grupo de correlacao + "
        "total simultaneo de 0,50%) tinha espaco de sobra. Removida: o motor de risco ja reavalia a "
        "cada iteracao contra o estado ja atualizado pela oportunidade anterior no mesmo ciclo — esse "
        "e o orcamento real. O loop agora executa ate esse orcamento se esgotar sozinho ou ate um teto "
        "de sanidade de 50 operacoes por ciclo (nunca mais um limite de negocio, so uma protecao contra "
        "um ciclo unico rodar indefinidamente).", "Body"))

    story.append(P("9.5 Edge medido passa a sobreviver a restart do processo (12/08/2026)", "H2"))
    story.append(P(
        "Pedido direto do usuario apos observar perda de progresso: \"eu nao quero que perde nada "
        "quando reinicia o sistema\". Ate esta correcao, <font face='JetBrainsMono'>EdgeScores</font> "
        "(Secao 1.4 — o historico de edge real medido por simbolo, a unica coisa que faz a exploracao e "
        "o Kelly hierarquico significarem algo) so existia em memoria e era recriado vazio a cada boot "
        "do processo. Com quantos restarts uma sessao de testes acumula, isso apagava o aprendizado "
        "inteiro toda hora, mesmo simbolos com centenas de amostras.", "Body"))
    story.append(callout_box(
        "<b>Caso real observado antes da correcao:</b> RVNUSDT comecou a acumular spread real no "
        "universo spot de arbitragem, subindo de +0,49% para +0,64% ao longo de 21 minutos — perto do "
        "limiar de 0,65% (Secao 7.1). Um restart do processo apagou esse progresso antes que desse pra "
        "saber se cruzaria o limiar ou nao; o simbolo nao voltou a ser sorteado pela rotacao de "
        "exploracao desde entao. Nao e um caso hipotetico — foi o motivo concreto que motivou esta "
        "correcao.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(P(
        "Corrigido: o edge medido agora e salvo em disco a cada rotacao (15min) E a cada 30 segundos "
        "via uma tarefa periodica separada, e recarregado do disco antes da primeira rotacao no boot — "
        "nunca mais que ~30s de medicao perdida num restart abrupto. Verificado ao vivo reiniciando o "
        "processo de proposito: log confirma recuperacao do disco, contagem de amostras de simbolos ja "
        "medidos continuou de onde parou em vez de zerar.", "Body"))

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

    story.append(P("11.1 Paineis que pareciam quebrados corrigidos com motivo real (12/08/2026)", "H2"))
    story.append(P(
        "Feedback direto do usuario usando o dashboard ao vivo: \"não estou gostando desse pulso das "
        "estratégias, só tem 2 com gráficos... cockpit de risco eu não sei no que ele está conectado... "
        "não tem nenhuma exposição por estratégia\". Em todos os tres casos, o painel nao estava com "
        "dado errado — estava mostrando um espaco vazio sem nenhuma explicacao do porque, o que se le "
        "como \"quebrado\" mesmo sendo o comportamento esperado do sistema:", "Body"))
    story.append(bullets([
        "<b>Pulso das estrategias</b> — as 7 estrategias sem nenhum trade mostravam um grafico de PnL "
        "vazio (uma linha reta quase invisivel). Substituido por um card de estado \"dormente\" com o "
        "motivo real e verificado — Macro so dispara numa janela de -5/+15min ao redor de FOMC/CPI/NFP, "
        "Launch Radar so em listagem nova, Multi-Asset so no horario de pregao de NY, etc.",
        "<b>Cockpit de risco</b> — a linha \"Manual (data/KILL)\" expunha um caminho de arquivo cru sem "
        "contexto. Virou \"Parada manual de emergencia\" com texto explicando que e acionada por quem "
        "opera o servidor, nao por regra automatica; o resumo do painel agora diz explicitamente que "
        "esta conectado ao drawdown real do portfolio, recalculado a cada ciclo, direto de risk.toml.",
        "<b>Exposicao por estrategia</b> — a tabela ficava sempre vazia por natureza (o modelo de "
        "execucao round-trip fecha cada ordem no mesmo ciclo, entao \"risco aberto\" quase sempre e "
        "US$0 entre snapshots), sem nenhuma explicacao. Agora mostra sempre, para as 9 estrategias: "
        "limite de risco configurado + perna atual — o que realmente fica conectado a "
        "<font face='JetBrainsMono'>risk.toml</font> e ao escalonamento por Kelly, com a exposicao "
        "aberta instantanea como coluna extra quando existir.",
    ]))
    story.append(callout_box(
        "Um bug real foi encontrado e corrigido no proprio processo desta correcao (nao chegou a ser "
        "reportado como pronto sem checar): o percentual de limite de risco por estrategia estava sendo "
        "multiplicado por 100 duas vezes (uma no backend, outra no JavaScript), mostrando 35% em vez de "
        "0,35%. Pego antes de qualquer commit.",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

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
        "<b>Bug real corrigido (12/08/2026):</b> a promessa de \"carrega automaticamente, sem "
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
        "<b>Correção de escopo (revisão técnica externa, 12/08/2026):</b> \"Multi-Asset ativo\" "
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
    story.append(P("Checklist de segurança de chaves (verificado nesta revisão, 13/08/2026)", "H2"))
    story.append(bullets([
        "<font face='JetBrainsMono'>orchestrator/.env</font> confirmado no <font face='JetBrainsMono'>.gitignore</font> raiz (3 padrões independentes: caminho exato, *.env, **/*.env) — nunca foi commitado.",
        "Nenhuma chave (Alpaca, Alchemy) aparece em texto plano em nenhum log, evento do EventBus ou resposta do dashboard — só os módulos que as leem diretamente do ambiente têm acesso.",
        "Todas as chaves em uso são de contas de <b>paper trading</b> (Alpaca) ou de nível gratuito somente-leitura de dado público (Alchemy) — nenhuma tem permissão de mover fundos reais, mesmo que vazasse.",
        "Falha ao carregar uma chave é sempre fail-safe: o módulo correspondente cai pra observação/desativado (ver <font face='JetBrainsMono'>multi_asset.rs</font>), nunca opera com um valor inventado no lugar da chave ausente.",
    ]))
    story.append(callout_box(
        "O que este checklist NÃO cobre ainda, porque não existe capital real conectado a nada: "
        "rotação periódica de chave, alerta automático de uso anômalo, e o processo de revogação de "
        "chave em caso de vazamento suspeito. Vira obrigatório antes da Fase 12 (Seção 10) — registrado "
        "aqui como pré-requisito, não como lacuna do estágio atual (100% paper trading não tem chave "
        "com poder de mover nada).",
        border_color=GOLD, bg=GOLD_SOFT,
    ))

    story.append(P("12.4 Kill-switch — em camadas (revisado 2x)", "H2"))
    story.append(P(
        "Entregável explícito da Fase 12. Revisado após 3ª auditoria técnica externa (12/08/2026): 5% "
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
        "<b>Sobre \"cancelar ordens abertas\" — ATUALIZADO 13/08/2026, o pré-requisito registrado "
        "abaixo já aconteceu:</b> desde a correção de exposição simultânea real (Seção 12.5), posições "
        "GENUINAMENTE ficam abertas entre o momento em que a ordem é aprovada e o momento em que a "
        "janela real de <font face='JetBrainsMono'>expected_holding_secs</font> termina — deixou de "
        "ser round-trip instantâneo. O kill-switch continua bloqueando só NOVAS aberturas "
        "(<font face='JetBrainsMono'>orchestrator.rs</font>: a resolução de posições já abertas roda "
        "incondicionalmente a cada tick, antes até da checagem de halt — de propósito, fechar o que já "
        "está aberto não deveria depender do kill-switch estar ativo ou não). Isso significa que, se o "
        "kill-switch disparar logo depois de várias posições abrirem, elas continuam correndo até o "
        "prazo natural delas, não são neutralizadas na hora. Na prática a exposição é pequena e "
        "curta — no máximo 30s (Order Flow) ou 2s (Arbitragem), as duas estratégias com volume real; "
        "Pump Exhaustion tem janela de 20min mas ainda não executa nenhum trade (Seção 7). Gap real, "
        "porém pequeno e limitado no tempo — registrado honestamente, não mais tratado como inaplicável.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))

    story.append(P("12.5 Realismo de execução e regras reais — auditoria de 13/08/2026", "H2"))
    story.append(P(
        "Dos itens listados na Seção 3, dois já estavam simulados desde a revisão anterior: "
        "preenchimento parcial de ordem (fração aleatória do notional pedido em ~35% das execuções) e "
        "falha da segunda perna em arbitragem (perna 1 executa, perna 2 falha — ~3% das execuções, "
        "tratado como perda no dobro do <font face='JetBrainsMono'>max_loss_pct</font> sobre o "
        "notional pedido inteiro). Uma auditoria técnica externa mais recente (13/08/2026) perguntou "
        "diretamente \"o que falta ainda pra ser 100% real em regras, validações e etc?\" — a resposta "
        "revelou um problema estrutural mais sério do que qualquer item isolado da Seção 3: os mapas "
        "de exposição por estratégia/grupo NUNCA recebiam valor diferente de zero em lugar nenhum do "
        "código, porque toda posição resolvia no mesmo instante em que abria. Os gates de limite de "
        "risco liam exposição sempre zerada — nunca bloqueavam por excesso SIMULTÂNEO, só um trade "
        "grande demais sozinho.", "Body"))
    story.append(bullets([
        "<b>Exposição simultânea real</b> — posições agora abrem e ficam genuinamente reservadas até a janela real de <font face='JetBrainsMono'>expected_holding_secs</font> terminar (30s Order Flow, 2s Arbitragem, 20min Pump Exhaustion), não mais round-trip instantâneo. Ver Seção 12.4 sobre o que isso muda no kill-switch.",
        "<b>Rate limiter real por venue</b> — token bucket (8 requisições/s, mesmo padrão de uma conta de varejo comum), consumido só quando uma ordem é de fato aberta.",
        "<b>Notional mínimo real por símbolo</b> — buscado ao vivo da própria API pública da exchange no boot (Bybit <font face='JetBrainsMono'>instruments-info</font>, Bitget <font face='JetBrainsMono'>spot/public/symbols</font> — 555 e 1.262 símbolos respectivamente); ordem abaixo do mínimo real é rejeitada.",
        "<b>Taxas corrigidas</b> — Bitget taker 0,10%-&gt;0,20% (verificado ao vivo na API pública), Bybit maker 0,08%-&gt;0,10% (taxa publicada padrão não-VIP, já que a conta real nunca teve desconto confirmado), Pump Exhaustion ganhou taxa taker (2&#215;0,055%) que antes não descontava NENHUMA taxa do edge medido.",
        "<b>Reamostragem por bootstrap</b> — Order Flow, Arbitragem e Pump Exhaustion agora sorteiam o resultado de cada trade a partir de um desfecho REAL já confirmado contra preço (histórico de confirmação), não de uma fórmula confidence&#215;net_edge.",
        "<b>Deduplicação de sinais sobrepostos</b> — janelas de confirmação sobrepostas do mesmo símbolo (autocorrelação) não contam mais como amostras independentes: só uma confirmação em voo por símbolo por vez.",
        "<b>Kelly com limite inferior de confiança</b> — a taxa de acerto usada no cálculo de Kelly deixou de ser o valor pontual (que superestima em amostra pequena) e passou a ser o limite inferior de Wilson (95% de confiança).",
        "<b>Normalização entre símbolos</b> — o Kelly hierárquico por símbolo (Seção 9.3) agora normaliza contra a média dos pares da mesma estratégia, pra vários símbolos não reivindicarem 2,00x cada SIMULTANEAMENTE.",
        "<b>Gate de escalonamento por evidência</b> — crescer a perna agora também exige orçamento de risco livre (não estar usando &gt;50% do limite da estratégia) e diversidade de símbolos (lucro vindo de pelo menos 2 símbolos distintos), além dos critérios já existentes (Seção 9.1).",
        "<b>Teto global de alavancagem e trava do Launch Radar</b> — ver Seção 9 (tabela de parâmetros) e Seção 9.1.",
    ]))
    story.append(callout_box(
        "<b>Achado ao vivo durante a verificação (não fazia parte da auditoria original):</b> KUBUSDT "
        "mostrava \"100% de acerto\" com o MESMO edge repetido em toda confirmação — investigação do "
        "log bruto revelou que o book da Bitget tinha parado de atualizar por 8+ minutos enquanto o da "
        "Bybit continuava normal. Não era vantagem real, era um preço morto sendo tratado como cotação "
        "executável. <font face='JetBrainsMono'>TopOfBook</font> ganhou um timestamp de última "
        "atualização; um book que não atualiza há mais de 30s agora é ignorado. O histórico de "
        "confirmação contaminado por esse bug foi apagado e recalibrado do zero.",
        border_color=RISK_RED, bg=colors.HexColor("#FBE9EC"),
    ))
    story.append(Spacer(1, 4))
    story.append(P(
        "<b>Ainda faltam</b> (lista completa continua na Seção 3): fila de ordens maker, clock drift, "
        "reconciliação pós-desconexão, funding cobrado durante posição aberta. Rate limits e notional "
        "mínimo real, que apareciam nessa lista em revisões anteriores, foram implementados nesta "
        "auditoria e saíram dela.", "Body"))
    story.append(callout_box(
        "<b>Honestidade sobre os dois números de preenchimento parcial/falha de perna:</b> 35% e 3% "
        "continuam sendo estimativas arbitrárias, não calibradas contra latência, rejeição ou "
        "preenchimento reais — diferente de tudo que foi corrigido nesta revisão, que veio de dado "
        "medido ou de API pública verificada. Calibrar esses dois exigiria execução real (não paper "
        "trading) ou uma fonte externa de latência/book de nível institucional — nenhuma das duas "
        "disponível nesta fase.",
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

    story.append(P("12.8 Dashboard — cockpit de risco e pulso das estratégias (12/08/2026)", "H2"))
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

    # KeepTogether (polimento visual, 13/08/2026): esta secao ficou orfa em
    # revisoes anteriores — o titulo sozinho no fim de uma pagina, com o
    # paragrafo que o segue empurrado pra pagina seguinte. Titulo + primeiro
    # paragrafo agora sao um bloco atomico pro paginador, nunca mais quebram
    # separados.
    story.append(KeepTogether([
        P("12.9 Amostragem independente e universo de símbolos (4ª revisão, 12/08/2026)", "H2"),
        P(
            "<b>Pump Exhaustion contava o mesmo evento várias vezes:</b> achado correto da revisão — um "
            "pump sustentado por minutos gerava \"sinal true\" em várias janelas de scan seguidas, e cada "
            "uma virava uma amostra nova na confirmation_history, mesmo sendo o mesmo evento observado "
            "repetidamente, não eventos independentes. Corrigido em duas frentes: (1) um símbolo não "
            "registra nova confirmação enquanto já tiver uma pendente (ainda dentro da janela de 20min); "
            "(2) o cooldown por símbolo subiu de 45s para 30min. Combinado, um símbolo só contribui uma "
            "amostra nova a cada ~50min no mínimo — 20min de janela + 30min de descanso.", "Body"),
    ]))
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

    # PageBreak forcado removido (polimento visual, 13/08/2026) — mesmo
    # raciocinio da Secao 2: deixava ~180pt de espaco vazio na pagina
    # anterior. Titulo + primeiro paragrafo protegidos por KeepTogether.
    story.append(KeepTogether([
        P("Apendice A — mapa resumido das fases historicas (v1)", "H1"),
        P(
            "A v1 deste documento organizava tudo em 13 fases sequenciais (Fase 0 a Fase 12). A Secao 2 "
            "substitui isso por uma escada de maturidade independente por estrategia, mas os numeros de "
            "fase continuam aparecendo em varios lugares do codigo e da conversa do projeto — este mapa "
            "e so a referencia de continuidade, nao a estrutura principal do documento.", "Body"),
    ]))
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
