"""
Testes do pipeline de ingestão: DocumentLoader e ChunkStrategies.
"""

import pytest
from pathlib import Path

# conftest.py adiciona o parent ao path
from document_processor import (
    Document,
    Chunk,
    DocumentLoader,
    ChunkStrategy,
    FixedSizeChunker,
    MarkdownHeaderChunker,
)

# Fixtures path
FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def loader():
    return DocumentLoader()


@pytest.fixture
def md_path():
    path = FIXTURES_DIR / "sample_with_headers.md"
    if not path.exists():
        pytest.skip("Fixture sample_with_headers.md não encontrado")
    return path


@pytest.fixture
def pdf_path():
    path = FIXTURES_DIR / "sample_10pages.pdf"
    if not path.exists():
        pytest.skip(
            "Fixture sample_10pages.pdf não encontrado. "
            "Execute: python tests/fixtures/generate_pdf_fixture.py"
        )
    return path


@pytest.fixture
def html_path():
    path = FIXTURES_DIR / "sample_with_tables.html"
    if not path.exists():
        pytest.skip("Fixture sample_with_tables.html não encontrado")
    return path


# ---------------------------------------------------------------------------
# DocumentLoader
# ---------------------------------------------------------------------------


class TestDocumentLoader:
    def test_load_markdown_returns_list_of_documents(self, loader, md_path):
        docs = loader.load_markdown(md_path)
        assert isinstance(docs, list)
        assert len(docs) == 1
        doc = docs[0]
        assert isinstance(doc, Document)
        assert doc.id
        assert "Seção Um" in doc.content
        assert doc.metadata.get("format") == "markdown"
        assert "source" in doc.metadata

    def test_load_markdown_file_not_found(self, loader):
        with pytest.raises(FileNotFoundError, match="não encontrado"):
            loader.load_markdown(FIXTURES_DIR / "nonexistent.md")

    def test_load_pdf_returns_one_doc_per_page(self, loader, pdf_path):
        pytest.importorskip("pypdf")
        docs = loader.load_pdf(pdf_path)
        assert isinstance(docs, list)
        assert len(docs) == 10
        for i, doc in enumerate(docs):
            assert isinstance(doc, Document)
            assert doc.id
            assert doc.metadata.get("format") == "pdf"
            assert doc.metadata.get("page") == i + 1
            assert doc.metadata.get("total_pages") == 10
            assert "Página" in doc.content or str(i + 1) in doc.content

    def test_load_pdf_file_not_found(self, loader):
        pytest.importorskip("pypdf")
        with pytest.raises(FileNotFoundError, match="não encontrado"):
            loader.load_pdf(FIXTURES_DIR / "nonexistent.pdf")

    def test_load_html_returns_list_and_strips_tags(self, loader, html_path):
        pytest.importorskip("bs4")
        docs = loader.load_html(html_path)
        assert isinstance(docs, list)
        assert len(docs) == 1
        doc = docs[0]
        assert isinstance(doc, Document)
        assert doc.id
        assert "Relatório de Dados" in doc.content
        assert "Tabela de Produtos" in doc.content
        assert "Produto A" in doc.content
        assert "<table>" not in doc.content
        assert "console.log" not in doc.content
        assert doc.metadata.get("format") == "html"

    def test_load_html_file_not_found(self, loader):
        pytest.importorskip("bs4")
        with pytest.raises(FileNotFoundError, match="não encontrado"):
            loader.load_html(FIXTURES_DIR / "nonexistent.html")


# ---------------------------------------------------------------------------
# Document e Chunk dataclasses
# ---------------------------------------------------------------------------


class TestDocumentAndChunk:
    def test_document_has_id_content_metadata(self):
        doc = Document(id="id1", content="hello", metadata={"k": "v"})
        assert doc.id == "id1"
        assert doc.content == "hello"
        assert doc.metadata == {"k": "v"}

    def test_chunk_has_id_text_metadata(self):
        ch = Chunk(id="c1", text="chunk text", metadata={"source_doc_id": "d1", "chunk_index": 0})
        assert ch.id == "c1"
        assert ch.text == "chunk text"
        assert ch.metadata["source_doc_id"] == "d1"
        assert ch.metadata["chunk_index"] == 0


# ---------------------------------------------------------------------------
# FixedSizeChunker
# ---------------------------------------------------------------------------


class TestFixedSizeChunker:
    def test_chunk_returns_list_of_chunks(self, loader, md_path):
        docs = loader.load_markdown(md_path)
        chunker = FixedSizeChunker(chunk_size=512, overlap=50)
        chunks = chunker.chunk(docs[0])
        assert isinstance(chunks, list)
        assert all(isinstance(c, Chunk) for c in chunks)
        for c in chunks:
            assert c.id
            assert c.text
            assert c.metadata.get("source_doc_id") == docs[0].id
            assert "chunk_index" in c.metadata

    def test_chunk_size_and_overlap_respected(self):
        doc = Document(id="d1", content="a" * 1000, metadata={})
        chunker = FixedSizeChunker(chunk_size=100, overlap=20)
        chunks = chunker.chunk(doc)
        assert len(chunks) >= 1
        assert all(len(c.text) <= 100 for c in chunks)
        # Overlap: next start = prev start + (100 - 20) = 80
        full_text = "".join(c.text for c in chunks)
        assert full_text.replace(" ", "").replace("\n", "") == "a" * 1000 or len(full_text) >= 1000

    def test_overlap_less_than_chunk_size(self):
        with pytest.raises(ValueError, match="overlap deve ser menor"):
            FixedSizeChunker(chunk_size=50, overlap=50)
        with pytest.raises(ValueError, match="overlap deve ser menor"):
            FixedSizeChunker(chunk_size=50, overlap=60)


# ---------------------------------------------------------------------------
# MarkdownHeaderChunker
# ---------------------------------------------------------------------------


class TestMarkdownHeaderChunker:
    def test_chunk_by_headers_returns_multiple_chunks(self, loader, md_path):
        docs = loader.load_markdown(md_path)
        chunker = MarkdownHeaderChunker()
        chunks = chunker.chunk(docs[0])
        assert isinstance(chunks, list)
        assert len(chunks) >= 3  # intro + Seção Um + Seção Dois + Seção Três (+ subseção)
        for c in chunks:
            assert isinstance(c, Chunk)
            assert c.metadata.get("source_doc_id") == docs[0].id
            assert "chunk_index" in c.metadata

    def test_chunks_contain_expected_sections(self, loader, md_path):
        docs = loader.load_markdown(md_path)
        chunker = MarkdownHeaderChunker()
        chunks = chunker.chunk(docs[0])
        texts = [c.text for c in chunks]
        assert any("Seção Um" in t for t in texts)
        assert any("Seção Dois" in t for t in texts)
        assert any("Seção Três" in t for t in texts)


# ---------------------------------------------------------------------------
# SemanticChunker (optional)
# ---------------------------------------------------------------------------


class TestSemanticChunker:
    def test_semantic_chunker_requires_sentence_transformers(self):
        from document_processor import SemanticChunker
        try:
            chunker = SemanticChunker()
            doc = Document(id="d1", content="First sentence. Second sentence. Third.", metadata={})
            chunks = chunker.chunk(doc)
            assert isinstance(chunks, list)
            assert all(isinstance(c, Chunk) for c in chunks)
        except ImportError as e:
            assert "sentence-transformers" in str(e)

    def test_semantic_chunker_integration(self):
        pytest.importorskip("sentence_transformers")
        from document_processor import SemanticChunker
        doc = Document(
            id="d1",
            content="Machine learning is useful. The weather is nice today. Neural networks learn from data.",
            metadata={},
        )
        chunker = SemanticChunker(similarity_threshold=0.3, min_chunk_chars=10, max_chunk_chars=500)
        chunks = chunker.chunk(doc)
        assert isinstance(chunks, list)
        assert len(chunks) >= 1
        for c in chunks:
            assert c.text
            assert c.metadata.get("source_doc_id") == "d1"
