"""
Re-rankers para o pipeline RAG.

Interface Reranker e implementações: CrossEncoder (sentence-transformers),
Cohere (API) e LLM (OpenAI/Anthropic para julgar relevância).
"""

from __future__ import annotations

import os
import re
from abc import ABC, abstractmethod
from typing import Any, Dict, List, Optional, Union

# Type alias compatible with API response and build_rag_prompt
SearchResult = Dict[str, Any]  # id, score, metadata


def _text_from_result(r: SearchResult) -> str:
    """Extrai texto do resultado para o reranker."""
    meta = r.get("metadata") or {}
    return (meta.get("text") or meta.get("content") or "").strip() or "(sem texto)"


class Reranker(ABC):
    """Interface para reordenar resultados de busca por relevância à query."""

    @abstractmethod
    def rerank(
        self,
        query: str,
        results: List[SearchResult],
        top_k: int = 5,
    ) -> List[SearchResult]:
        """Reordena e trunca resultados. Retorna top_k mais relevantes."""
        ...


# ---------------------------------------------------------------------------
# CrossEncoderReranker (sentence-transformers)
# ---------------------------------------------------------------------------

try:
    from sentence_transformers import CrossEncoder as _CrossEncoder
except ImportError:
    _CrossEncoder = None  # type: ignore


class CrossEncoderReranker(Reranker):
    """Re-ranking com CrossEncoder (sentence-transformers)."""

    def __init__(
        self,
        model_name: str = "cross-encoder/ms-marco-MiniLM-L6-v2",
    ):
        if _CrossEncoder is None:
            raise ImportError(
                "sentence-transformers é necessário. Instale: pip install sentence-transformers"
            )
        self._model = _CrossEncoder(model_name)

    def rerank(
        self,
        query: str,
        results: List[SearchResult],
        top_k: int = 5,
    ) -> List[SearchResult]:
        if not results:
            return []
        pairs = [(query, _text_from_result(r)) for r in results]
        scores = self._model.predict(pairs)
        if hasattr(scores, "tolist"):
            scores = scores.tolist()
        indexed = list(zip(scores, results))
        indexed.sort(key=lambda x: x[0], reverse=True)
        top = indexed[:top_k]
        out = []
        for score, r in top:
            out.append({
                "id": r["id"],
                "score": float(score),
                "metadata": r.get("metadata") or {},
            })
        return out


# ---------------------------------------------------------------------------
# CohereReranker (API)
# ---------------------------------------------------------------------------

try:
    import cohere
except ImportError:
    cohere = None  # type: ignore


class CohereReranker(Reranker):
    """Re-ranking via API Cohere."""

    def __init__(
        self,
        api_key: Optional[str] = None,
        model: str = "rerank-english-v3.0",
    ):
        if cohere is None:
            raise ImportError("cohere é necessário. Instale: pip install cohere")
        self._client = cohere.Client(api_key=api_key or os.environ.get("COHERE_API_KEY"))
        self._model = model

    def rerank(
        self,
        query: str,
        results: List[SearchResult],
        top_k: int = 5,
    ) -> List[SearchResult]:
        if not results:
            return []
        documents = [_text_from_result(r) for r in results]
        response = self._client.rerank(
            query=query,
            documents=documents,
            top_n=min(top_k, len(results)),
            model=self._model,
        )
        out = []
        for item in response.results:
            idx = item.index
            r = results[idx]
            out.append({
                "id": r["id"],
                "score": float(item.relevance_score),
                "metadata": r.get("metadata") or {},
            })
        return out


# ---------------------------------------------------------------------------
# LLMReranker (OpenAI / Anthropic)
# ---------------------------------------------------------------------------

class LLMReranker(Reranker):
    """Re-ranking usando LLM para pontuar relevância query-documento."""

    def __init__(
        self,
        provider: str = "openai",
        model: Optional[str] = None,
        api_key: Optional[str] = None,
        batch_size: int = 5,
    ):
        self._provider = provider
        self._model = model or ("gpt-4o-mini" if provider == "openai" else "claude-3-5-haiku-20241022")
        self._api_key = api_key
        self._batch_size = batch_size

    def _score_single(self, query: str, doc_text: str) -> float:
        prompt = f"""Avalie a relevância do documento abaixo em relação à pergunta. Responda APENAS com um número de 0 a 10 (0 = irrelevante, 10 = muito relevante).

Pergunta: {query}

Documento:
{doc_text[:4000]}

Relevância (0-10):"""
        if self._provider == "openai":
            from openai import OpenAI
            client = OpenAI(api_key=self._api_key or os.environ.get("OPENAI_API_KEY"))
            r = client.chat.completions.create(
                model=self._model,
                messages=[{"role": "user", "content": prompt}],
                max_tokens=10,
            )
            text = (r.choices[0].message.content or "").strip()
        else:
            from anthropic import Anthropic
            client = Anthropic(api_key=self._api_key or os.environ.get("ANTHROPIC_API_KEY"))
            msg = client.messages.create(
                model=self._model,
                max_tokens=10,
                messages=[{"role": "user", "content": prompt}],
            )
            text = (msg.content[0].text if msg.content and msg.content[0].type == "text" else "").strip()
        return self._parse_score(text)

    @staticmethod
    def _parse_score(text: str) -> float:
        """Extrai número 0-10 da resposta. Fallback 0 em falha."""
        if not text:
            return 0.0
        match = re.search(r"\b([0-9]|10)\b", text)
        if match:
            return float(match.group(1))
        return 0.0

    def rerank(
        self,
        query: str,
        results: List[SearchResult],
        top_k: int = 5,
    ) -> List[SearchResult]:
        if not results:
            return []
        scored = []
        for r in results:
            doc_text = _text_from_result(r)
            score = self._score_single(query, doc_text)
            scored.append((score, r))
        scored.sort(key=lambda x: x[0], reverse=True)
        out = []
        for score, r in scored[:top_k]:
            out.append({
                "id": r["id"],
                "score": score,
                "metadata": r.get("metadata") or {},
            })
        return out


# ---------------------------------------------------------------------------
# Factory
# ---------------------------------------------------------------------------

def get_reranker(
    name: str,
    **kwargs: Any,
) -> Reranker:
    """Retorna instância do reranker pelo nome."""
    if name == "cross_encoder":
        return CrossEncoderReranker(**{k: v for k, v in kwargs.items() if k in ("model_name",)})
    if name == "cohere":
        return CohereReranker(**{k: v for k, v in kwargs.items() if k in ("api_key", "model")})
    if name == "llm":
        return LLMReranker(**{k: v for k, v in kwargs.items() if k in ("provider", "model", "api_key", "batch_size")})
    raise ValueError(f"Reranker desconhecido: {name}. Use: cross_encoder, cohere ou llm")
