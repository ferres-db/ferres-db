"""
E2E: full RAG pipeline — start server, ingest 10 docs, run app with 5 questions,
validate answers contain expected keywords and latency < 3s per question.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

# Questions and at least one keyword that must appear in the answer (case-insensitive)
RAG_QUESTIONS = [
    {"question": "What is FerresDB?", "expected_keywords": ["ferres", "vector", "database", "embedding"]},
    {"question": "How does semantic search work?", "expected_keywords": ["semantic", "embed", "vector", "similarity", "nearest"]},
    {"question": "What is RAG?", "expected_keywords": ["rag", "retrieval", "generation", "context", "llm"]},
    {"question": "What document formats can be ingested?", "expected_keywords": ["markdown", "pdf", "html"]},
    {"question": "How is data persisted?", "expected_keywords": ["storage", "disk", "persist", "restart", "durability"]},
]

MAX_LATENCY_MS = 3000


@pytest.fixture(scope="module")
def _local_embedding_available():
    """Skip if sentence-transformers (local embeddings) is not installed."""
    pytest.importorskip("sentence_transformers")


@pytest.fixture(scope="module")
def _openai_api_key():
    """Skip if OPENAI_API_KEY is not set (required for LLM in RAG)."""
    import os
    if not os.environ.get("OPENAI_API_KEY"):
        pytest.skip("OPENAI_API_KEY not set (required for RAG LLM)")


def test_full_rag_pipeline(
    ferres_server,
    corpus_10_path,
    ingestion_root,
    simple_rag_root,
    _local_embedding_available,
    _openai_api_key,
    tmp_path,
):
    """Ingest 10 docs, run 5 questions through RAG, assert keywords and latency."""
    base_url = ferres_server.base_url
    collection = "docs"

    # 1. Ingest corpus
    ingest_py = ingestion_root / "ingest.py"
    assert ingest_py.exists(), f"ingest.py not found at {ingest_py}"
    ingest_cmd = [
        sys.executable,
        str(ingest_py),
        "--source",
        str(corpus_10_path),
        "--collection",
        collection,
        "--server",
        base_url,
        "--embedding",
        "local",
        "--chunker",
        "fixed",
        "--chunk-size",
        "256",
    ]
    result = subprocess.run(ingest_cmd, cwd=str(ingestion_root), capture_output=True, text=True, timeout=120)
    assert result.returncode == 0, f"ingest failed: {result.stderr}"

    # 2. Write questions file
    questions_file = tmp_path / "questions.txt"
    questions_file.write_text(
        "\n".join(q["question"] for q in RAG_QUESTIONS),
        encoding="utf-8",
    )

    # 3. Run app in batch mode
    app_py = simple_rag_root / "app.py"
    assert app_py.exists(), f"app.py not found at {app_py}"
    app_cmd = [
        sys.executable,
        str(app_py),
        "--collection",
        collection,
        "--server",
        base_url,
        "--embedding",
        "local",
        "--llm",
        "openai",
        "--no-stream",
        "--questions-file",
        str(questions_file),
    ]
    # Allow OPENAI_API_KEY to be set in env for LLM calls
    result = subprocess.run(
        app_cmd,
        cwd=str(simple_rag_root),
        capture_output=True,
        text=True,
        timeout=120,
        env={**__import__("os").environ},
    )
    if result.returncode != 0:
        pytest.fail(f"app.py batch failed: {result.stderr or result.stdout}")

    # 4. Parse JSONL lines and validate
    lines = [line.strip() for line in result.stdout.strip().splitlines() if line.strip()]
    assert len(lines) == len(RAG_QUESTIONS), f"Expected {len(RAG_QUESTIONS)} JSONL lines, got {len(lines)}"

    for i, line in enumerate(lines):
        data = json.loads(line)
        assert "question" in data and "answer" in data and "latency_ms" in data
        answer = (data["answer"] or "").lower()
        latency_ms = data["latency_ms"]
        expected_keywords = RAG_QUESTIONS[i]["expected_keywords"]
        found = any(kw.lower() in answer for kw in expected_keywords)
        assert found, (
            f"Question {i + 1}: expected at least one of {expected_keywords} in answer. "
            f"Got: {data['answer'][:200]}..."
        )
        assert latency_ms < MAX_LATENCY_MS, (
            f"Question {i + 1}: latency {latency_ms} ms exceeds max {MAX_LATENCY_MS} ms"
        )
