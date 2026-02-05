#!/usr/bin/env python3
"""
Avaliação do pipeline RAG: Recall@5, MRR e latência.

Uso:
  python evaluate.py --collection docs --questions eval_questions.jsonl --report eval_report.json
  python evaluate.py --collection docs --no-llm   # só retrieval (sem LLM), mais rápido
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Set, Tuple

APP_ROOT = Path(__file__).resolve().parent
if str(APP_ROOT) not in sys.path:
    sys.path.insert(0, str(APP_ROOT))

from dotenv import load_dotenv
load_dotenv(APP_ROOT / ".env")
load_dotenv()

import requests

from app import get_embedding_provider, rag_query, search_collection


def doc_id_from_source(source: str) -> str:
    """Extrai o id do documento a partir de metadata.source (path). Ex: .../058fe27392fd9b09.md -> 058fe27392fd9b09."""
    if not source:
        return ""
    p = Path(source)
    return p.stem


def retrieved_doc_ids(results: List[Dict[str, Any]], top_k: int = 5) -> List[str]:
    """Retorna os doc ids (stem do source) dos top_k resultados, na ordem."""
    doc_ids: List[str] = []
    seen: Set[str] = set()
    for r in results[:top_k]:
        meta = r.get("metadata") or {}
        src = meta.get("source") or meta.get("source_path") or ""
        doc_id = doc_id_from_source(src)
        if doc_id and doc_id not in seen:
            seen.add(doc_id)
            doc_ids.append(doc_id)
    return doc_ids


def recall_at_k(expected: List[str], retrieved: List[str], k: int = 5) -> float:
    """Recall@k: proporção de expected que aparece nos primeiros k retrieved."""
    if not expected:
        return 1.0
    retrieved_set = set(retrieved[:k])
    hit = sum(1 for e in expected if e in retrieved_set)
    return hit / len(expected)


def mrr_at_k(expected: List[str], retrieved: List[str], k: int = 5) -> float:
    """MRR@k: 1/rank do primeiro expected encontrado (rank 1-based). 0 se nenhum em top-k."""
    if not expected:
        return 1.0
    for rank, doc_id in enumerate(retrieved[:k], start=1):
        if doc_id in expected:
            return 1.0 / rank
    return 0.0


def keyword_hits(answer: str, keywords: List[str]) -> Tuple[int, int]:
    """Retorna (quantidade de keywords que aparecem na resposta, total de keywords)."""
    if not keywords:
        return (0, 0)
    answer_lower = answer.lower()
    hit = sum(1 for kw in keywords if kw.lower() in answer_lower)
    return (hit, len(keywords))


def load_questions(path: Path) -> List[Dict[str, Any]]:
    """Carrega eval_questions.jsonl."""
    if not path.exists():
        raise FileNotFoundError(f"Arquivo não encontrado: {path}")
    questions = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        questions.append(json.loads(line))
    return questions


def run_evaluation(
    questions: List[Dict[str, Any]],
    *,
    server_base: str,
    collection: str,
    embedding_provider: Any,
    llm: str,
    llm_model: str | None = None,
    top_k: int = 5,
    use_llm: bool = True,
    reranker: Any = None,
) -> Tuple[List[Dict[str, Any]], List[float], List[Dict[str, Any]]]:
    """
    Roda cada pergunta no RAG (ou só retrieval se use_llm=False).
    Retorna (resultados por pergunta, latências em segundos, detalhes por pergunta).
    """
    session = requests.Session()
    session.headers.setdefault("Content-Type", "application/json")

    per_question: List[Dict[str, Any]] = []
    latencies: List[float] = []
    details: List[Dict[str, Any]] = []

    for i, q in enumerate(questions):
        question = q.get("question", "")
        expected_docs = q.get("expected_docs") or []
        keywords = q.get("keywords") or []

        t0 = time.perf_counter()
        try:
            if use_llm:
                answer, results = rag_query(
                    question,
                    server_base=server_base,
                    collection=collection,
                    embedding_provider=embedding_provider,
                    llm=llm,
                    llm_model=llm_model,
                    top_k=top_k,
                    stream=False,
                    session=session,
                    reranker=reranker,
                    quiet=True,
                )
            else:
                # Apenas retrieval
                q_vector = embedding_provider.embed_batch([question])[0]
                results = search_collection(
                    server_base, collection, q_vector, limit=top_k, session=session
                )
                answer = ""
        except Exception as e:
            results = []
            answer = ""
            per_question.append({
                "question": question,
                "error": str(e),
                "retrieved_doc_ids": [],
                "expected_docs": expected_docs,
                "recall_at_5": 0.0,
                "mrr_at_5": 0.0,
                "keyword_hits": 0,
                "keyword_total": len(keywords),
            })
            latencies.append(time.perf_counter() - t0)
            details.append({"question": question, "error": str(e)})
            continue

        elapsed = time.perf_counter() - t0
        latencies.append(elapsed)

        retrieved = retrieved_doc_ids(results, top_k=top_k)
        rec = recall_at_k(expected_docs, retrieved, k=top_k)
        mrr = mrr_at_k(expected_docs, retrieved, k=top_k)
        kw_hit, kw_tot = keyword_hits(answer, keywords)

        per_question.append({
            "question": question,
            "retrieved_doc_ids": retrieved,
            "expected_docs": expected_docs,
            "recall_at_5": round(rec, 4),
            "mrr_at_5": round(mrr, 4),
            "keyword_hits": kw_hit,
            "keyword_total": kw_tot,
            "latency_sec": round(elapsed, 3),
        })
        details.append({
            "question": question,
            "recall_at_5": rec,
            "mrr_at_5": mrr,
            "latency_sec": elapsed,
            "retrieved": retrieved,
            "expected": expected_docs,
        })

    return per_question, latencies, details


def build_report(
    per_question: List[Dict[str, Any]],
    latencies: List[float],
    config: Dict[str, Any],
) -> Dict[str, Any]:
    """Monta o relatório agregado."""
    n = len(per_question)
    if n == 0:
        return {
            "timestamp": time.time(),
            "num_questions": 0,
            "config": config,
            "metrics": {},
            "per_question": [],
        }

    recalls = [p["recall_at_5"] for p in per_question if "recall_at_5" in p]
    mrrs = [p["mrr_at_5"] for p in per_question if "mrr_at_5" in p]
    lat_sec = [p.get("latency_sec", 0) for p in per_question if "latency_sec" in p]

    mean_recall = sum(recalls) / len(recalls) if recalls else 0.0
    mean_mrr = sum(mrrs) / len(mrrs) if mrrs else 0.0
    mean_latency_ms = (sum(lat_sec) / len(lat_sec)) * 1000 if lat_sec else 0.0

    kw_hits = sum(p.get("keyword_hits", 0) for p in per_question)
    kw_tot = sum(p.get("keyword_total", 0) for p in per_question)
    keyword_recall = kw_hits / kw_tot if kw_tot else 0.0

    errors = sum(1 for p in per_question if p.get("error"))

    return {
        "timestamp": time.time(),
        "num_questions": n,
        "config": config,
        "metrics": {
            "recall_at_5_mean": round(mean_recall, 4),
            "mrr_at_5_mean": round(mean_mrr, 4),
            "latency_mean_ms": round(mean_latency_ms, 2),
            "keyword_recall": round(keyword_recall, 4),
            "errors": errors,
        },
        "per_question": per_question,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Avaliação RAG: Recall@5, MRR, latência")
    parser.add_argument("--collection", "-c", required=True, help="Nome da coleção")
    parser.add_argument("--questions", "-q", type=Path, default=APP_ROOT / "eval_questions.jsonl", help="Arquivo eval_questions.jsonl")
    parser.add_argument("--report", "-r", type=Path, default=APP_ROOT / "eval_report.json", help="Arquivo de saída do relatório JSON")
    parser.add_argument("--server", default="http://localhost:3000", help="URL base do FerresDB")
    parser.add_argument("--embedding", "-e", choices=["openai", "cohere", "local"], default="openai")
    parser.add_argument("--llm", choices=["openai", "anthropic"], default="openai")
    parser.add_argument("--llm-model", help="Modelo LLM")
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument("--no-llm", action="store_true", help="Só retrieval, sem chamada ao LLM (mais rápido)")
    args = parser.parse_args()

    questions = load_questions(args.questions)
    if not questions:
        print("Nenhuma pergunta em", args.questions, file=sys.stderr)
        sys.exit(1)

    try:
        provider = get_embedding_provider(args.embedding)
    except (ValueError, ImportError) as e:
        print(f"Erro: {e}", file=sys.stderr)
        sys.exit(1)

    config = {
        "collection": args.collection,
        "server": args.server,
        "embedding": args.embedding,
        "llm": args.llm if not args.no_llm else None,
        "top_k": args.top_k,
        "use_llm": not args.no_llm,
    }

    print(f"Avaliando {len(questions)} perguntas (LLM={'on' if not args.no_llm else 'off'})...")
    per_question, latencies, _ = run_evaluation(
        questions,
        server_base=args.server,
        collection=args.collection,
        embedding_provider=provider,
        llm=args.llm,
        llm_model=args.llm_model,
        top_k=args.top_k,
        use_llm=not args.no_llm,
        reranker=None,
    )

    report = build_report(per_question, latencies, config)
    report_path = Path(args.report).resolve()
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")

    m = report["metrics"]
    recall = m["recall_at_5_mean"]
    target_ok = recall >= 0.60
    print("\nMétricas:")
    print(f"  Recall@5 (média): {recall:.2%}  {'[OK]' if target_ok else '[abaixo do target 60%]'}")
    print(f"  MRR@5 (média):    {m['mrr_at_5_mean']:.4f}")
    print(f"  Latência (média): {m['latency_mean_ms']:.0f} ms")
    print(f"  Keyword recall:   {m['keyword_recall']:.2%}")
    print(f"  Erros:            {m['errors']}")
    print(f"\nRelatório salvo: {report_path}")


if __name__ == "__main__":
    main()
