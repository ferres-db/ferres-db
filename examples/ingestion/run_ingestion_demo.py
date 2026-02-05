#!/usr/bin/env python3
"""Script de demonstração do pipeline de ingestão."""

from pathlib import Path

from document_processor import (
    DocumentLoader,
    FixedSizeChunker,
    MarkdownHeaderChunker,
)

def main():
    base = Path(__file__).resolve().parent
    fixtures = base / "tests" / "fixtures"

    loader = DocumentLoader()

    # 1) Markdown
    md_path = fixtures / "sample_with_headers.md"
    if md_path.exists():
        docs = loader.load_markdown(md_path)
        print(f"[Markdown] 1 documento, {len(docs[0].content)} chars")
        chunker = FixedSizeChunker(chunk_size=200, overlap=30)
        chunks = chunker.chunk(docs[0])
        print(f"  FixedSizeChunker: {len(chunks)} chunks")
        header_chunker = MarkdownHeaderChunker()
        chunks_h = header_chunker.chunk(docs[0])
        print(f"  MarkdownHeaderChunker: {len(chunks_h)} chunks")
    else:
        print("[Markdown] fixture não encontrado:", md_path)

    # 2) PDF
    pdf_path = fixtures / "sample_10pages.pdf"
    if pdf_path.exists():
        docs = loader.load_pdf(pdf_path)
        print(f"[PDF] {len(docs)} páginas")
        chunker = FixedSizeChunker(chunk_size=256, overlap=0)
        all_chunks = []
        for doc in docs:
            all_chunks.extend(chunker.chunk(doc))
        print(f"  Total chunks (FixedSize): {len(all_chunks)}")
    else:
        print("[PDF] fixture não encontrado. Rode: pytest tests -v (gera o PDF) ou python tests/fixtures/generate_pdf_fixture.py")

    # 3) HTML
    html_path = fixtures / "sample_with_tables.html"
    if html_path.exists():
        docs = loader.load_html(html_path)
        print(f"[HTML] 1 documento, {len(docs[0].content)} chars")
        chunker = FixedSizeChunker(chunk_size=300, overlap=50)
        chunks = chunker.chunk(docs[0])
        print(f"  FixedSizeChunker: {len(chunks)} chunks")
    else:
        print("[HTML] fixture não encontrado:", html_path)

    print("\nDone.")

if __name__ == "__main__":
    main()
