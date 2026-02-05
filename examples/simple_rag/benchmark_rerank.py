#!/usr/bin/env python3
"""
Benchmark de qualidade do rerank no pipeline RAG.

Mede recall@5 antes e depois do rerank e latência (retrieval, retrieval+rerank).
Dataset: JSONL com {"question": "...", "relevant_ids": ["id1", "id2", ...]}.

Uso:
  python benchmark_rerank.py --collection docs --dataset benchmark_questions.jsonl
  python benchmark_rerank.py --collection docs --dataset benchmark_questions.jsonl --reranker cohere --server http://localhost:3000
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

# same path setup as app.py
APP_ROOT = Path(__file__).resolve().parent
if str(APP_ROOT) not in sys.path:
    sys.path.insert(0, str(APP_ROOT))

from dotenv import load_dotenv
load_dotenv(APP_ROOT / ".env")
load_dotenv()

EXAMPLES_ROOT = APP_ROOT.parent
INGESTION_ROOT = EXAMPLES_ROOT / "ingestion"
if str(INGESTION_ROOT) not in sys.path:
    sys.path.insert(0, str(INGESTION_ROOT))

from app import get_embedding_provider, get_reranker, search_hybrid_collection


def load_dataset(path: Path) -> list[dict]:
    """Carrega JSONL: uma linha por objeto com question e relevant_ids."""
    rows = []
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rows.append(json.loads(line))
    return rows


def recall_at_k(retrieved_ids: list[str], relevant_ids: list[str], k: int) -> float:
    """Recall@k: proporção de relevantes encontrados no top-k. Denominador: min(k, |relevant_ids|)."""
    if not relevant_ids:
        return 0.0
    top_k_ids = set(retrieved_ids[:k])
    relevant_set = set(relevant_ids)
    hits = len(top_k_ids & relevant_set)
    denom = min(k, len(relevant_set))
    return hits / denom if denom else 0.0


def run_benchmark(
    dataset_path: Path,
    collection: str,
    server_base: str,
    embedding: str,
    reranker_name: str,
    retrieve_top_k: int = 20,
    rerank_top_k: int = 5,
    hybrid_alpha: float = 0.5,
) -> dict:
    """Executa benchmark e retorna métricas agregadas."""
    provider = get_embedding_provider(embedding)
    reranker = get_reranker(reranker_name)

    rows = load_dataset(dataset_path)
    if not rows:
        return {"error": "dataset vazio", "n": 0}

    latencies_retrieval = []
    latencies_rerank = []
    recalls_before = []
    recalls_after = []

    for i, row in enumerate(rows):
        question = row.get("question", "").strip()
        relevant_ids = list(row.get("relevant_ids") or [])
        if not question:
            continue

        # Embed
        q_vector = provider.embed_batch([question])[0]

        # Retrieval only
        t0 = time.perf_counter()
        results = search_hybrid_collection(
            server_base, collection, question, q_vector,
            limit=retrieve_top_k, alpha=hybrid_alpha,
        )
        t_retrieval = time.perf_counter() - t0
        latencies_retrieval.append(t_retrieval)

        # Recall before rerank (top-5 dos resultados da busca)
        top5_before = [r["id"] for r in results[:rerank_top_k]]
        rec_before = recall_at_k(top5_before, relevant_ids, rerank_top_k)
        recalls_before.append(rec_before)

        # Rerank
        t1 = time.perf_counter()
        results_reranked = reranker.rerank(question, results, top_k=rerank_top_k)
        t_rerank = time.perf_counter() - t1
        latencies_rerank.append(t_rerank)

        # Recall after rerank
        top5_after = [r["id"] for r in results_reranked]
        rec_after = recall_at_k(top5_after, relevant_ids, rerank_top_k)
        recalls_after.append(rec_after)

    n = len(recalls_before)
    sorted_ret = sorted(latencies_retrieval)
    p95_idx = min(int(0.95 * n), n - 1) if n else 0
    latency_retrieval_p95_ms = sorted_ret[p95_idx] * 1000 if n else 0
    return {
        "n": n,
        "recall_at_5_before_mean": sum(recalls_before) / n if n else 0,
        "recall_at_5_after_mean": sum(recalls_after) / n if n else 0,
        "latency_retrieval_mean_ms": (sum(latencies_retrieval) / n * 1000) if n else 0,
        "latency_retrieval_p95_ms": latency_retrieval_p95_ms,
        "latency_rerank_mean_ms": (sum(latencies_rerank) / n * 1000) if n else 0,
        "latency_retrieval_plus_rerank_mean_ms": (sum(latencies_retrieval) / n + sum(latencies_rerank) / n) * 1000 if n else 0,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Benchmark de recall@5 e latência com/sem rerank")
    parser.add_argument("--collection", "-c", required=True, help="Nome da coleção")
    parser.add_argument("--dataset", "-d", required=True, type=Path, help="Arquivo JSONL (question, relevant_ids)")
    parser.add_argument("--server", default="http://localhost:3000", help="URL base do FerresDB")
    parser.add_argument("--embedding", "-e", choices=["openai", "cohere", "local"], default="openai")
    parser.add_argument("--reranker", choices=["cross_encoder", "cohere", "llm"], default="cross_encoder")
    parser.add_argument("--retrieve-top-k", type=int, default=20)
    parser.add_argument("--rerank-top-k", type=int, default=5)
    parser.add_argument("--hybrid-alpha", type=float, default=0.5)
    args = parser.parse_args()

    if not args.dataset.exists():
        print(f"Erro: dataset não encontrado: {args.dataset}", file=sys.stderr)
        sys.exit(1)

    metrics = run_benchmark(
        args.dataset,
        args.collection,
        args.server,
        args.embedding,
        args.reranker,
        retrieve_top_k=args.retrieve_top_k,
        rerank_top_k=args.rerank_top_k,
        hybrid_alpha=args.hybrid_alpha,
    )

    if metrics.get("error"):
        print(metrics["error"], file=sys.stderr)
        sys.exit(1)

    n = metrics["n"]
    print(f"Benchmark: {n} perguntas | coleção={args.collection} | reranker={args.reranker}")
    print("-" * 50)
    print(f"Recall@5 (antes do rerank):  {metrics['recall_at_5_before_mean']:.4f}")
    print(f"Recall@5 (depois do rerank): {metrics['recall_at_5_after_mean']:.4f}")
    print(f"Latência retrieval (média):  {metrics['latency_retrieval_mean_ms']:.0f} ms")
    print(f"Latência retrieval (p95):    {metrics['latency_retrieval_p95_ms']:.0f} ms")
    print(f"Latência rerank (média):     {metrics['latency_rerank_mean_ms']:.0f} ms")
    print(f"Latência retrieval+rerank:   {metrics['latency_retrieval_plus_rerank_mean_ms']:.0f} ms (média)")


if __name__ == "__main__":
    main()
