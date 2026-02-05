#!/usr/bin/env python3
"""Gera o fixture PDF de 10 páginas para testes."""

from pathlib import Path

try:
    from reportlab.lib.pagesizes import A4
    from reportlab.pdfgen import canvas
except ImportError:
    print("Instale reportlab: pip install reportlab")
    raise

FIXTURES_DIR = Path(__file__).resolve().parent
OUTPUT_PDF = FIXTURES_DIR / "sample_10pages.pdf"


def main():
    c = canvas.Canvas(str(OUTPUT_PDF), pagesize=A4)
    width, height = A4
    for page in range(1, 11):
        c.drawString(72, height - 72, f"Página {page} de 10")
        c.drawString(72, height - 100, "Este é um documento PDF de teste para o pipeline de ingestão.")
        c.drawString(72, height - 124, "Cada página contém texto simples para validar o DocumentLoader.")
        c.drawString(72, height - 148, f"Linha extra na página {page} para preencher conteúdo.")
        if page < 10:
            c.showPage()
    c.save()
    print(f"Gerado: {OUTPUT_PDF}")


if __name__ == "__main__":
    main()
