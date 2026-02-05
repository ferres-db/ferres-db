"""Configuração do pytest: path e geração do fixture PDF se ausente."""

import sys
from pathlib import Path

# Garante que o módulo document_processor está no path
ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))


def _generate_pdf_fixture_if_missing():
    """Gera sample_10pages.pdf com reportlab se o arquivo não existir."""
    fixtures_dir = Path(__file__).resolve().parent / "fixtures"
    pdf_path = fixtures_dir / "sample_10pages.pdf"
    if pdf_path.exists():
        return
    try:
        from reportlab.lib.pagesizes import A4
        from reportlab.pdfgen import canvas
    except ImportError:
        return
    pdf_path.parent.mkdir(parents=True, exist_ok=True)
    c = canvas.Canvas(str(pdf_path), pagesize=A4)
    _, height = A4
    for page in range(1, 11):
        c.drawString(72, height - 72, f"Página {page} de 10")
        c.drawString(72, height - 100, "Documento PDF de teste para o pipeline de ingestão.")
        c.drawString(72, height - 124, f"Conteúdo da página {page}.")
        if page < 10:
            c.showPage()
    c.save()


def pytest_configure(config):
    _generate_pdf_fixture_if_missing()
