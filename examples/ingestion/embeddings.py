"""
Módulo de embeddings para o pipeline de ingestão.

Fornece interface EmbeddingProvider e implementações (OpenAI, local, Cohere)
com cache SQLite, rate limiting, retry e barra de progresso.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import time
from abc import ABC, abstractmethod
from pathlib import Path
from typing import List, Optional

# Optional deps
try:
    import tqdm
except ImportError:
    tqdm = None  # type: ignore

try:
    import tiktoken
except ImportError:
    tiktoken = None  # type: ignore


# ---------------------------------------------------------------------------
# EmbeddingProvider (interface)
# ---------------------------------------------------------------------------


class EmbeddingProvider(ABC):
    """Interface para provedores de embeddings."""

    @property
    @abstractmethod
    def dimension(self) -> int:
        """Dimensão dos vetores de embedding."""
        ...

    @abstractmethod
    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Gera embeddings para uma lista de textos. Ordem preservada."""
        ...

    @property
    def batch_size(self) -> int:
        """Tamanho máximo recomendado por lote (para rate limit e cache)."""
        return 32

    @property
    def model_id(self) -> str:
        """Identificador do modelo (para cache)."""
        return self.__class__.__name__


# ---------------------------------------------------------------------------
# LocalEmbeddings (sentence-transformers)
# ---------------------------------------------------------------------------

try:
    from sentence_transformers import SentenceTransformer
except ImportError:
    SentenceTransformer = None  # type: ignore


class LocalEmbeddings(EmbeddingProvider):
    """Embeddings locais via sentence-transformers. Batch 32."""

    def __init__(self, model_name: str = "all-MiniLM-L6-v2"):
        if SentenceTransformer is None:
            raise ImportError(
                "sentence-transformers é necessário. Instale: pip install sentence-transformers"
            )
        self._model = SentenceTransformer(model_name)
        self._model_name = model_name
        self._dim = len(self._model.encode("test"))

    @property
    def dimension(self) -> int:
        return self._dim

    @property
    def batch_size(self) -> int:
        return 32

    @property
    def model_id(self) -> str:
        return f"local:{self._model_name}"

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        if not texts:
            return []
        vectors = self._model.encode(
            texts,
            convert_to_numpy=True,
            normalize_embeddings=True,
            show_progress_bar=False,
        )
        return [v.tolist() for v in vectors]


# ---------------------------------------------------------------------------
# OpenAIEmbeddings (tiktoken para contagem, batch 100)
# ---------------------------------------------------------------------------

try:
    from openai import OpenAI
except ImportError:
    OpenAI = None  # type: ignore


class OpenAIEmbeddings(EmbeddingProvider):
    """Embeddings via API OpenAI. Usa tiktoken para contagem de tokens, batch 100."""

    def __init__(
        self,
        model: str = "text-embedding-3-small",
        api_key: Optional[str] = None,
        max_tokens_per_text: int = 8191,
    ):
        if OpenAI is None:
            raise ImportError("openai é necessário. Instale: pip install openai")
        self._client = OpenAI(api_key=api_key)
        self._model = model
        self._max_tokens = max_tokens_per_text
        self._dim = self._get_dimension()
        self._encoding = None
        if tiktoken:
            try:
                self._encoding = tiktoken.encoding_for_model("gpt-4")  # fallback for embed models
            except Exception:
                self._encoding = tiktoken.get_encoding("cl100k_base")

    def _get_dimension(self) -> int:
        r = self._client.embeddings.create(input=["dimension test"], model=self._model)
        return len(r.data[0].embedding)

    def _count_tokens(self, text: str) -> int:
        if self._encoding is None:
            return len(text) // 4  # aproximação
        return len(self._encoding.encode(text))

    def _truncate_to_tokens(self, text: str) -> str:
        if not self._encoding or self._count_tokens(text) <= self._max_tokens:
            return text
        tokens = self._encoding.encode(text)
        return self._encoding.decode(tokens[: self._max_tokens])

    @property
    def dimension(self) -> int:
        return self._dim

    @property
    def batch_size(self) -> int:
        return 100

    @property
    def model_id(self) -> str:
        return f"openai:{self._model}"

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        if not texts:
            return []
        truncated = [self._truncate_to_tokens(t) for t in texts]
        r = self._client.embeddings.create(input=truncated, model=self._model)
        by_idx = {d.index: d.embedding for d in r.data}
        return [by_idx[i] for i in range(len(texts))]

    def count_tokens(self, texts: List[str]) -> int:
        """Contagem total de tokens (para estimativa de custo)."""
        return sum(self._count_tokens(self._truncate_to_tokens(t)) for t in texts)


# ---------------------------------------------------------------------------
# CohereEmbeddings
# ---------------------------------------------------------------------------

try:
    import cohere
except ImportError:
    cohere = None  # type: ignore


class CohereEmbeddings(EmbeddingProvider):
    """Embeddings via API Cohere."""

    def __init__(
        self,
        model: str = "embed-english-v3.0",
        api_key: Optional[str] = None,
        input_type: str = "search_document",
    ):
        if cohere is None:
            raise ImportError("cohere é necessário. Instale: pip install cohere")
        self._client = cohere.Client(api_key=api_key)
        self._model = model
        self._input_type = input_type
        self._dim = self._get_dimension()

    def _get_dimension(self) -> int:
        r = self._client.embed(texts=["test"], model=self._model, input_type=self._input_type)
        return len(r.embeddings[0])

    @property
    def dimension(self) -> int:
        return self._dim

    @property
    def batch_size(self) -> int:
        return 96  # limite típico Cohere

    @property
    def model_id(self) -> str:
        return f"cohere:{self._model}"

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        if not texts:
            return []
        r = self._client.embed(
            texts=texts,
            model=self._model,
            input_type=self._input_type,
        )
        return [list(e) for e in r.embeddings]


# ---------------------------------------------------------------------------
# Cache SQLite
# ---------------------------------------------------------------------------


def _text_hash(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


class CachedEmbeddingProvider(EmbeddingProvider):
    """Wrapper que adiciona cache SQLite a qualquer EmbeddingProvider."""

    def __init__(
        self,
        provider: EmbeddingProvider,
        cache_path: str | Path = ".embeddings_cache.sqlite",
        requests_per_minute: Optional[int] = 60,
        retries: int = 3,
        show_progress: bool = True,
    ):
        self._provider = provider
        self._path = Path(cache_path)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._conn: Optional[sqlite3.Connection] = None
        self._rpm = requests_per_minute
        self._retries = retries
        self._show_progress = show_progress
        self._init_db()

    def _init_db(self) -> None:
        self._conn = sqlite3.connect(str(self._path))
        self._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS embeddings (
                model_id TEXT NOT NULL,
                text_hash TEXT NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY (model_id, text_hash)
            )
            """
        )
        self._conn.commit()

    def _get_conn(self) -> sqlite3.Connection:
        if self._conn is None:
            self._init_db()
        return self._conn  # type: ignore

    @property
    def dimension(self) -> int:
        return self._provider.dimension

    @property
    def batch_size(self) -> int:
        return self._provider.batch_size

    @property
    def model_id(self) -> str:
        return self._provider.model_id

    def embed_batch(self, texts: List[str]) -> List[List[float]]:
        conn = self._get_conn()
        model_id = self.model_id
        result: List[Optional[List[float]]] = [None] * len(texts)
        to_embed: List[int] = []
        to_embed_texts: List[str] = []

        for i, text in enumerate(texts):
            h = _text_hash(text)
            row = conn.execute(
                "SELECT embedding FROM embeddings WHERE model_id = ? AND text_hash = ?",
                (model_id, h),
            ).fetchone()
            if row:
                result[i] = json.loads(row[0].decode("utf-8"))
            else:
                to_embed.append(i)
                to_embed_texts.append(text)

        if not to_embed_texts:
            return result  # type: ignore

        batch_size = self._provider.batch_size
        delay = (60.0 / self._rpm) if self._rpm else 0.0
        batches_idx = [to_embed[i : i + batch_size] for i in range(0, len(to_embed), batch_size)]
        batches_txt = [to_embed_texts[i : i + batch_size] for i in range(0, len(to_embed_texts), batch_size)]
        pairs = list(zip(batches_idx, batches_txt))
        it = tqdm.tqdm(pairs, desc="Embeddings (cache miss)", unit="batch") if (self._show_progress and tqdm) else pairs

        for batch_idx, batch_txt in it:
            last_err = None
            for attempt in range(self._retries):
                try:
                    new_emb = self._provider.embed_batch(batch_txt)
                    for i, emb in zip(batch_idx, new_emb):
                        result[i] = emb
                        h = _text_hash(texts[i])
                        conn.execute(
                            "INSERT OR REPLACE INTO embeddings (model_id, text_hash, embedding) VALUES (?, ?, ?)",
                            (model_id, h, json.dumps(emb).encode("utf-8")),
                        )
                    break
                except Exception as e:
                    last_err = e
                    if attempt < self._retries - 1:
                        time.sleep(2 ** attempt)
            else:
                if last_err:
                    raise last_err
            if delay > 0:
                time.sleep(delay)

        conn.commit()
        return result  # type: ignore


# ---------------------------------------------------------------------------
# Rate limit + Retry + Progress (embed_texts)
# ---------------------------------------------------------------------------


def embed_texts(
    provider: EmbeddingProvider,
    texts: List[str],
    *,
    use_cache: bool = False,
    cache_path: str | Path = ".embeddings_cache.sqlite",
    requests_per_minute: Optional[int] = 60,
    retries: int = 3,
    show_progress: bool = True,
) -> List[List[float]]:
    """
    Gera embeddings para todos os textos com rate limit, retry e opcionalmente cache e tqdm.

    - use_cache: usa CachedEmbeddingProvider com SQLite em cache_path.
    - requests_per_minute: delay entre batches (None = sem limite).
    - retries: tentativas por batch em caso de erro.
    - show_progress: usa tqdm se disponível.
    """
    if use_cache:
        provider = CachedEmbeddingProvider(
            provider,
            cache_path=cache_path,
            requests_per_minute=requests_per_minute,
            retries=retries,
            show_progress=show_progress,
        )
    if not texts:
        return []

    batch_size = provider.batch_size
    batches = [texts[i : i + batch_size] for i in range(0, len(texts), batch_size)]
    all_embeddings: List[List[float]] = []
    delay = (60.0 / requests_per_minute) if requests_per_minute else 0.0
    iterator = batches
    if show_progress and tqdm:
        iterator = tqdm.tqdm(batches, desc="Embeddings", unit="batch")

    for b in iterator:
        last_error = None
        for attempt in range(retries):
            try:
                emb = provider.embed_batch(b)
                all_embeddings.extend(emb)
                break
            except Exception as e:
                last_error = e
                if attempt < retries - 1:
                    time.sleep(2 ** attempt)
        else:
            raise last_error  # type: ignore
        if delay > 0:
            time.sleep(delay)

    return all_embeddings
