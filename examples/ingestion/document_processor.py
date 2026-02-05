"""
Pipeline de ingestão de documentos para FerresDB.

Carrega documentos (Markdown, PDF, HTML) e segmenta em chunks
com estratégias configuráveis.
"""

from __future__ import annotations

import re
import uuid
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Any, List, Optional

# Optional heavy deps: pypdf, bs4, sentence_transformers
try:
    from pypdf import PdfReader
except ImportError:
    PdfReader = None  # type: ignore

try:
    from bs4 import BeautifulSoup
except ImportError:
    BeautifulSoup = None  # type: ignore

try:
    from sentence_transformers import SentenceTransformer
except ImportError:
    SentenceTransformer = None  # type: ignore


@dataclass
class Document:
    """Documento carregado de uma fonte (arquivo)."""

    id: str
    content: str
    metadata: dict


@dataclass
class Chunk:
    """Fragmento de um documento, pronto para embedding/indexação."""

    id: str
    text: str
    metadata: dict  # inclui source_doc_id, chunk_index


# ---------------------------------------------------------------------------
# DocumentLoader
# ---------------------------------------------------------------------------


class DocumentLoader:
    """Carrega documentos a partir de arquivos Markdown, PDF e HTML."""

    def load_markdown(self, path: str | Path) -> List[Document]:
        """Carrega um arquivo Markdown e retorna uma lista com um Document."""
        path = Path(path)
        if not path.exists():
            raise FileNotFoundError(f"Arquivo não encontrado: {path}")
        content = path.read_text(encoding="utf-8", errors="replace")
        doc_id = str(uuid.uuid4())
        return [
            Document(
                id=doc_id,
                content=content,
                metadata={"source": str(path), "format": "markdown"},
            )
        ]

    def load_pdf(self, path: str | Path) -> List[Document]:
        """Carrega um PDF e retorna um Document por página (usa pypdf)."""
        if PdfReader is None:
            raise ImportError("pypdf é necessário para load_pdf. Instale com: pip install pypdf")
        path = Path(path)
        if not path.exists():
            raise FileNotFoundError(f"Arquivo não encontrado: {path}")
        reader = PdfReader(str(path))
        documents: List[Document] = []
        for i, page in enumerate(reader.pages):
            text = page.extract_text() or ""
            doc_id = str(uuid.uuid4())
            documents.append(
                Document(
                    id=doc_id,
                    content=text,
                    metadata={
                        "source": str(path),
                        "format": "pdf",
                        "page": i + 1,
                        "total_pages": len(reader.pages),
                    },
                )
            )
        return documents

    def load_html(self, path: str | Path) -> List[Document]:
        """Carrega um arquivo HTML e extrai texto (usa BeautifulSoup)."""
        if BeautifulSoup is None:
            raise ImportError(
                "beautifulsoup4 é necessário para load_html. Instale com: pip install beautifulsoup4"
            )
        path = Path(path)
        if not path.exists():
            raise FileNotFoundError(f"Arquivo não encontrado: {path}")
        content = path.read_text(encoding="utf-8", errors="replace")
        soup = BeautifulSoup(content, "html.parser")
        # Remove scripts e estilos
        for tag in soup(["script", "style"]):
            tag.decompose()
        text = soup.get_text(separator="\n", strip=True)
        # Normaliza quebras de linha
        text = re.sub(r"\n{3,}", "\n\n", text)
        doc_id = str(uuid.uuid4())
        return [
            Document(
                id=doc_id,
                content=text,
                metadata={"source": str(path), "format": "html"},
            )
        ]


# ---------------------------------------------------------------------------
# ChunkStrategy (interface)
# ---------------------------------------------------------------------------


class ChunkStrategy(ABC):
    """Interface para estratégias de segmentação de documentos em chunks."""

    @abstractmethod
    def chunk(self, document: Document) -> List[Chunk]:
        """Segmenta um documento em uma lista de Chunks."""
        ...


# ---------------------------------------------------------------------------
# FixedSizeChunker
# ---------------------------------------------------------------------------


class FixedSizeChunker(ChunkStrategy):
    """Segmenta por tamanho fixo em caracteres, com overlap opcional."""

    def __init__(self, chunk_size: int = 512, overlap: int = 50):
        if overlap >= chunk_size:
            raise ValueError("overlap deve ser menor que chunk_size")
        self.chunk_size = chunk_size
        self.overlap = overlap

    def chunk(self, document: Document) -> List[Chunk]:
        chunks: List[Chunk] = []
        text = document.content
        start = 0
        step = self.chunk_size - self.overlap
        idx = 0
        while start < len(text):
            end = start + self.chunk_size
            segment = text[start:end]
            if segment.strip():
                chunk_id = str(uuid.uuid4())
                chunks.append(
                    Chunk(
                        id=chunk_id,
                        text=segment,
                        metadata={
                            "source_doc_id": document.id,
                            "chunk_index": idx,
                            **document.metadata,
                        },
                    )
                )
                idx += 1
            start += step
        return chunks


# ---------------------------------------------------------------------------
# SemanticChunker
# ---------------------------------------------------------------------------


class SemanticChunker(ChunkStrategy):
    """Segmenta por similaridade semântica entre sentenças (sentence-transformers)."""

    def __init__(
        self,
        model_name: str = "all-MiniLM-L6-v2",
        similarity_threshold: float = 0.5,
        min_chunk_chars: int = 100,
        max_chunk_chars: int = 512,
    ):
        if SentenceTransformer is None:
            raise ImportError(
                "sentence-transformers é necessário para SemanticChunker. "
                "Instale com: pip install sentence-transformers"
            )
        self.model_name = model_name
        self.similarity_threshold = similarity_threshold
        self.min_chunk_chars = min_chunk_chars
        self.max_chunk_chars = max_chunk_chars
        self._model: Optional[Any] = None

    @property
    def model(self):
        if self._model is None:
            self._model = SentenceTransformer(self.model_name)
        return self._model

    def _split_sentences(self, text: str) -> List[str]:
        # Split por pontuação final e quebras de linha
        parts = re.split(r"(?<=[.!?])\s+|\n+", text)
        return [p.strip() for p in parts if p.strip()]

    def chunk(self, document: Document) -> List[Chunk]:
        sentences = self._split_sentences(document.content)
        if not sentences:
            return []
        embeddings = self.model.encode(sentences)
        # Calcular similaridade entre sentenças consecutivas; quebrar quando cair abaixo do threshold
        break_indices = [0]
        for i in range(1, len(embeddings)):
            sim = float(
                (embeddings[i - 1] @ embeddings[i])
                / (
                    (embeddings[i - 1] @ embeddings[i - 1]) ** 0.5
                    * (embeddings[i] @ embeddings[i]) ** 0.5
                    + 1e-9
                )
            )
            if sim < self.similarity_threshold:
                break_indices.append(i)
        break_indices.append(len(sentences))

        chunks: List[Chunk] = []
        for j in range(len(break_indices) - 1):
            start, end = break_indices[j], break_indices[j + 1]
            segment = " ".join(sentences[start:end]).strip()
            if not segment:
                continue
            if len(segment) > self.max_chunk_chars:
                # Quebrar em sub-chunks por tamanho
                for k in range(0, len(segment), self.max_chunk_chars):
                    sub = segment[k : k + self.max_chunk_chars].strip()
                    if sub:
                        chunk_id = str(uuid.uuid4())
                        chunks.append(
                            Chunk(
                                id=chunk_id,
                                text=sub,
                                metadata={
                                    "source_doc_id": document.id,
                                    "chunk_index": len(chunks),
                                    **document.metadata,
                                },
                            )
                        )
            elif len(segment) >= self.min_chunk_chars or not chunks:
                chunk_id = str(uuid.uuid4())
                chunks.append(
                    Chunk(
                        id=chunk_id,
                        text=segment,
                        metadata={
                            "source_doc_id": document.id,
                            "chunk_index": len(chunks),
                            **document.metadata,
                        },
                    )
                )
            else:
                # Anexar ao chunk anterior se muito pequeno
                prev = chunks[-1]
                chunks[-1] = Chunk(
                    id=prev.id,
                    text=prev.text + " " + segment,
                    metadata={**prev.metadata},
                )
        return chunks


# ---------------------------------------------------------------------------
# MarkdownHeaderChunker
# ---------------------------------------------------------------------------


class MarkdownHeaderChunker(ChunkStrategy):
    """Segmenta Markdown por headers de nível ## (e superiores)."""

    def __init__(self, header_pattern: str = r"^#{2,6}\s+.+$"):
        self.header_pattern = re.compile(header_pattern, re.MULTILINE)

    def chunk(self, document: Document) -> List[Chunk]:
        text = document.content
        parts: List[str] = []
        last = 0
        for m in self.header_pattern.finditer(text):
            if m.start() > last:
                parts.append(text[last : m.start()].strip())
            parts.append(text[m.start() : m.end()].strip())
            last = m.end()
        if last < len(text):
            parts.append(text[last:].strip())

        # Agrupar: cada header + conteúdo até o próximo header vira um chunk
        current: List[str] = []
        chunks: List[Chunk] = []
        for i, part in enumerate(parts):
            if self.header_pattern.match(part):
                if current:
                    chunk_text = "\n\n".join(current)
                    if chunk_text.strip():
                        chunk_id = str(uuid.uuid4())
                        chunks.append(
                            Chunk(
                                id=chunk_id,
                                text=chunk_text,
                                metadata={
                                    "source_doc_id": document.id,
                                    "chunk_index": len(chunks),
                                    **document.metadata,
                                },
                            )
                        )
                current = [part]
            else:
                if current:
                    current.append(part)
                else:
                    current = [part]
        if current:
            chunk_text = "\n\n".join(current)
            if chunk_text.strip():
                chunk_id = str(uuid.uuid4())
                chunks.append(
                    Chunk(
                        id=chunk_id,
                        text=chunk_text,
                        metadata={
                            "source_doc_id": document.id,
                            "chunk_index": len(chunks),
                            **document.metadata,
                        },
                    )
                )
        return chunks
