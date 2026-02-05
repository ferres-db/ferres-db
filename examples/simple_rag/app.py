#!/usr/bin/env python3
"""
Exemplo RAG (Retrieval-Augmented Generation) com FerresDB.

Componentes:
- Ingestão: reutiliza o pipeline de examples/ingestion
- Query: embedding da pergunta -> busca top-k -> prompt com contexto -> LLM -> resposta + fontes

Uso:
  python app.py ingest --source ./docs --collection docs
  python app.py --collection docs --llm openai
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# Paths: app está em examples/simple_rag, ingestão em examples/ingestion
APP_ROOT = Path(__file__).resolve().parent
EXAMPLES_ROOT = APP_ROOT.parent
INGESTION_ROOT = EXAMPLES_ROOT / "ingestion"

# Carrega variáveis de .env (APP_ROOT ou diretório atual)
from dotenv import load_dotenv
load_dotenv(APP_ROOT / ".env")
load_dotenv()  # fallback: .env no cwd

if str(INGESTION_ROOT) not in sys.path:
    sys.path.insert(0, str(INGESTION_ROOT))

import requests

from embeddings import (
    CohereEmbeddings,
    EmbeddingProvider,
    LocalEmbeddings,
    OpenAIEmbeddings,
)
from reranker import Reranker, get_reranker


def get_embedding_provider(name: str) -> EmbeddingProvider:
    if name == "openai":
        return OpenAIEmbeddings(api_key=os.environ.get("OPENAI_API_KEY"))
    if name == "cohere":
        return CohereEmbeddings(api_key=os.environ.get("COHERE_API_KEY"))
    if name == "local":
        return LocalEmbeddings()
    raise ValueError(f"Embedding desconhecido: {name}. Use: openai, cohere ou local (deve coincidir com a ingestão)")


def search_collection(
    server_base: str,
    collection: str,
    vector: List[float],
    limit: int = 5,
    session: Optional[requests.Session] = None,
) -> List[Dict[str, Any]]:
    """Busca os top-k pontos mais similares. Retorna lista de {id, score, metadata}."""
    url = f"{server_base.rstrip('/')}/api/v1/collections/{collection}/search"
    payload = {"vector": vector, "limit": limit}
    if session is None:
        session = requests.Session()
    resp = session.post(url, json=payload)
    if resp.status_code != 200:
        try:
            err = resp.json()
            raise RuntimeError(err.get("message", resp.text))
        except ValueError:
            raise RuntimeError(f"HTTP {resp.status_code}: {resp.text}")
    data = resp.json()
    return data.get("results", [])


def search_hybrid_collection(
    server_base: str,
    collection: str,
    query_text: str,
    query_vector: List[float],
    limit: int = 20,
    alpha: float = 0.5,
    session: Optional[requests.Session] = None,
) -> List[Dict[str, Any]]:
    """Busca híbrida (vetorial + BM25). Retorna lista de {id, score, metadata}. Requer coleção com BM25 habilitado."""
    url = f"{server_base.rstrip('/')}/api/v1/collections/{collection}/search/hybrid"
    payload = {
        "query_text": query_text,
        "query_vector": query_vector,
        "limit": limit,
        "alpha": alpha,
    }
    if session is None:
        session = requests.Session()
    resp = session.post(url, json=payload)
    if resp.status_code != 200:
        try:
            err = resp.json()
            raise RuntimeError(err.get("message", resp.text))
        except ValueError:
            raise RuntimeError(f"HTTP {resp.status_code}: {resp.text}")
    data = resp.json()
    return data.get("results", [])


def llm_openai(
    prompt: str,
    stream: bool = True,
    model: str = "gpt-4o-mini",
    api_key: Optional[str] = None,
) -> str:
    from openai import OpenAI
    client = OpenAI(api_key=api_key or os.environ.get("OPENAI_API_KEY"))
    messages = [{"role": "user", "content": prompt}]
    if stream:
        stream_obj = client.chat.completions.create(
            model=model,
            messages=messages,
            stream=True,
        )
        full = []
        for chunk in stream_obj:
            if chunk.choices and chunk.choices[0].delta.content:
                content = chunk.choices[0].delta.content
                full.append(content)
                print(content, end="", flush=True)
        print()
        return "".join(full)
    r = client.chat.completions.create(model=model, messages=messages)
    return (r.choices[0].message.content or "").strip()


def _llm_anthropic_stream(client: Any, prompt: str, model: str) -> str:
    stream = client.messages.create(
        model=model,
        max_tokens=1024,
        messages=[{"role": "user", "content": prompt}],
        stream=True,
    )
    full = []
    for event in stream:
        if getattr(event, "type", None) == "content_block_delta":
            delta = getattr(event, "delta", None)
            if delta and getattr(delta, "text", None):
                t = delta.text
                full.append(t)
                print(t, end="", flush=True)
    print()
    return "".join(full)


def build_rag_prompt(question: str, context_chunks: List[Dict[str, Any]]) -> str:
    context_parts = []
    for i, r in enumerate(context_chunks, 1):
        meta = r.get("metadata") or {}
        text = meta.get("text") or meta.get("content")
        if not text:
            text = "(conteúdo não disponível no metadata)"
        source = meta.get("source", "N/A")
        context_parts.append(f"[{i}] (fonte: {source})\n{text}")
    context = "\n\n---\n\n".join(context_parts) if context_parts else "(Nenhum contexto recuperado)"
    return f"""Contexto da documentação:

{context}

---

Pergunta: {question}

Responda com base apenas no contexto acima. Se não souber, diga que não encontrou informação."""


def rag_query(
    question: str,
    *,
    server_base: str,
    collection: str,
    embedding_provider: EmbeddingProvider,
    llm: str,
    llm_model: Optional[str] = None,
    top_k: int = 5,
    stream: bool = True,
    session: Optional[requests.Session] = None,
    reranker: Optional[Reranker] = None,
    retrieve_top_k: int = 20,
    rerank_top_k: int = 5,
    hybrid_alpha: float = 0.5,
    quiet: bool = False,
) -> Tuple[str, List[Dict[str, Any]]]:
    """Pipeline RAG: embed -> search (e opcionalmente rerank) -> prompt -> LLM. Retorna (resposta, resultados da busca)."""
    q_vector = embedding_provider.embed_batch([question])[0]
    if reranker is not None:
        results = search_hybrid_collection(
            server_base, collection, question, q_vector,
            limit=retrieve_top_k, alpha=hybrid_alpha, session=session,
        )
        results = reranker.rerank(question, results, top_k=rerank_top_k)
    else:
        results = search_collection(server_base, collection, q_vector, limit=top_k, session=session)
    prompt = build_rag_prompt(question, results)

    if not quiet:
        print("[gerando resposta...]")
        if stream:
            print("Resposta: ", end="", flush=True)
    if llm == "openai":
        model = llm_model or "gpt-3.5-turbo"
        answer = llm_openai(prompt, stream=stream, model=model)
    elif llm == "anthropic":
        from anthropic import Anthropic
        model = llm_model or "claude-3-5-haiku-20241022"
        client = Anthropic(api_key=os.environ.get("ANTHROPIC_API_KEY"))
        if stream:
            answer = _llm_anthropic_stream(client, prompt, model)
        else:
            msg = client.messages.create(
                model=model,
                max_tokens=1024,
                messages=[{"role": "user", "content": prompt}],
            )
            answer = (msg.content[0].text if msg.content and msg.content[0].type == "text" else "")
    else:
        raise ValueError(f"LLM desconhecido: {llm}")
    return answer, results


def run_ingest(args: argparse.Namespace) -> None:
    """Chama o script de ingestão (examples/ingestion/ingest.py)."""
    ingest_script = INGESTION_ROOT / "ingest.py"
    if not ingest_script.exists():
        print(f"Erro: não encontrado {ingest_script}", file=sys.stderr)
        sys.exit(1)
    cmd = [
        sys.executable,
        str(ingest_script),
        "--source", args.source,
        "--collection", args.collection,
        "--embedding", getattr(args, "embedding", "openai"),
        "--chunker", getattr(args, "chunker", "semantic"),
        "--chunk-size", str(getattr(args, "chunk_size", 512)),
        "--server", args.server,
    ]
    if getattr(args, "cache", False):
        cmd.append("--cache")
    if getattr(args, "workers", None):
        cmd.extend(["--workers", str(args.workers)])
    result = subprocess.run(cmd, cwd=str(INGESTION_ROOT))
    sys.exit(result.returncode)


def save_history_entry(
    history_path: Path,
    question: str,
    answer: str,
    sources: List[Dict[str, Any]],
) -> None:
    entry = {
        "ts": time.time(),
        "question": question,
        "answer": answer,
        "sources": [
            {"id": r.get("id"), "score": r.get("score"), "source": (r.get("metadata") or {}).get("source")}
            for r in sources
        ],
    }
    with open(history_path, "a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def save_feedback_entry(
    feedback_path: Path,
    session_id: str,
    turn_index: int,
    useful: bool,
    comment: Optional[str] = None,
) -> None:
    """Append one line to feedback.jsonl (PoC: feedback inline Útil/Não útil)."""
    feedback_path.parent.mkdir(parents=True, exist_ok=True)
    entry = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "session_id": session_id,
        "turn_index": turn_index,
        "useful": useful,
        "comment": comment,
    }
    with open(feedback_path, "a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def save_session_event(session_path: Path, event: Dict[str, Any]) -> None:
    """Append one session event (query ou feedback) para replay depois."""
    session_path.parent.mkdir(parents=True, exist_ok=True)
    event["ts"] = time.time()
    with open(session_path, "a", encoding="utf-8") as f:
        f.write(json.dumps(event, ensure_ascii=False) + "\n")


def interactive_loop(args: argparse.Namespace) -> None:
    session = requests.Session()
    session.headers.setdefault("Content-Type", "application/json")
    try:
        provider = get_embedding_provider(args.embedding)
    except (ValueError, ImportError) as e:
        print(f"Erro: {e}", file=sys.stderr)
        sys.exit(1)

    history_path = Path(args.history).resolve() if args.history else None
    if history_path:
        history_path.parent.mkdir(parents=True, exist_ok=True)

    session_id = os.environ.get("SESSION_ID") or str(uuid.uuid4())
    session_path: Optional[Path] = None
    if getattr(args, "session_file", None):
        session_path = Path(args.session_file).resolve()
    else:
        # Gravar sessão por padrão quando feedback está ativo (replay depois)
        if getattr(args, "feedback", False):
            session_path = APP_ROOT / "data" / "sessions" / f"{session_id}.jsonl"
    feedback_path: Optional[Path] = None
    if getattr(args, "feedback", False):
        default_feedback = APP_ROOT.parent.parent / "data" / "logs" / "feedback.jsonl"
        feedback_path = Path(args.feedback_file).resolve() if getattr(args, "feedback_file", None) else default_feedback
        feedback_path.parent.mkdir(parents=True, exist_ok=True)

    reranker_instance = None
    if getattr(args, "rerank", False):
        try:
            reranker_instance = get_reranker(args.reranker)
        except (ValueError, ImportError) as e:
            print(f"Erro ao carregar reranker: {e}", file=sys.stderr)
            sys.exit(1)

    print("RAG (FerresDB + LLM). Coleção:", args.collection, "| LLM:", args.llm, "| Embedding:", args.embedding, end="")
    if reranker_instance:
        print(" | Rerank:", args.reranker, f"({args.retrieve_top_k}->{args.rerank_top_k})", end="")
    if getattr(args, "feedback", False):
        print(" | Feedback: on", end="")
    if session_path:
        print(" | Sessão:", session_id, end="")
    print()
    print("Digite sua pergunta (enter). Sair: quit, exit ou Ctrl+D.\n")

    turn_index = 0
    while True:
        try:
            line = input("RAG> ").strip()
        except EOFError:
            break
        if not line or line.lower() in ("quit", "exit", "q"):
            break
        question = line

        print("[buscando contexto...]")
        t0 = time.perf_counter()
        try:
            answer, sources = rag_query(
                question,
                server_base=args.server,
                collection=args.collection,
                embedding_provider=provider,
                llm=args.llm,
                llm_model=args.llm_model,
                top_k=args.top_k,
                stream=args.stream,
                session=session,
                reranker=reranker_instance,
                retrieve_top_k=getattr(args, "retrieve_top_k", 20),
                rerank_top_k=getattr(args, "rerank_top_k", 5),
                hybrid_alpha=getattr(args, "hybrid_alpha", 0.5),
            )
        except Exception as e:
            print(f"Erro: {e}", file=sys.stderr)
            continue
        latency_ms = int((time.perf_counter() - t0) * 1000)

        if not args.stream:
            print("Resposta:", answer)

        if args.show_chunks and sources:
            print("\nChunks recuperados:")
            for i, r in enumerate(sources, 1):
                meta = r.get("metadata") or {}
                src = meta.get("source", "N/A")
                score = r.get("score", 0)
                text_preview = (meta.get("text") or "")[:120]
                if len(meta.get("text") or "") > 120:
                    text_preview += "..."
                print(f"  {i}. {src} (score: {score:.2f})")
                if text_preview:
                    print(f"     {text_preview}")

        print("\nFontes:")
        for r in sources:
            meta = r.get("metadata") or {}
            src = meta.get("source", "N/A")
            score = r.get("score", 0)
            print(f"  - {src} (score: {score:.2f})")

        if session_path:
            save_session_event(
                session_path,
                {
                    "event": "query",
                    "session_id": session_id,
                    "turn_index": turn_index,
                    "question": question,
                    "answer": answer,
                    "sources": [
                        {"id": r.get("id"), "score": r.get("score"), "source": (r.get("metadata") or {}).get("source")}
                        for r in sources
                    ],
                    "latency_ms": latency_ms,
                },
            )

        if feedback_path:
            while True:
                try:
                    useful_in = input("Resposta foi útil? (s/n): ").strip().lower()
                except EOFError:
                    useful_in = ""
                if useful_in in ("s", "sim", "y", "yes"):
                    useful = True
                    break
                if useful_in in ("n", "não", "nao", "no"):
                    useful = False
                    break
                print("Digite s ou n.")
            try:
                comment = input("Comentário (enter para pular): ").strip() or None
            except EOFError:
                comment = None
            save_feedback_entry(feedback_path, session_id, turn_index, useful, comment)
            if session_path:
                save_session_event(
                    session_path,
                    {"event": "feedback", "session_id": session_id, "turn_index": turn_index, "useful": useful, "comment": comment},
                )

        if history_path:
            save_history_entry(history_path, question, answer, sources)
        turn_index += 1
        print()

    if session_path:
        print("Sessão gravada em:", session_path)
    print("Até mais.")


def batch_loop(args: argparse.Namespace) -> None:
    """Run RAG for each question in --questions-file; print one JSONL line per result (question, answer, latency_ms)."""
    questions_path = Path(args.questions_file)
    if not questions_path.exists():
        print(f"Erro: arquivo não encontrado: {questions_path}", file=sys.stderr)
        sys.exit(1)
    questions = [
        line.strip() for line in questions_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not questions:
        print("Erro: nenhuma pergunta no arquivo", file=sys.stderr)
        sys.exit(1)

    session = requests.Session()
    session.headers.setdefault("Content-Type", "application/json")
    try:
        provider = get_embedding_provider(args.embedding)
    except (ValueError, ImportError) as e:
        print(f"Erro: {e}", file=sys.stderr)
        sys.exit(1)

    reranker_instance = None
    if getattr(args, "rerank", False):
        try:
            reranker_instance = get_reranker(args.reranker)
        except (ValueError, ImportError) as e:
            print(f"Erro ao carregar reranker: {e}", file=sys.stderr)
            sys.exit(1)

    for question in questions:
        t0 = time.perf_counter()
        try:
            answer, sources = rag_query(
                question,
                server_base=args.server,
                collection=args.collection,
                embedding_provider=provider,
                llm=args.llm,
                llm_model=args.llm_model,
                top_k=args.top_k,
                stream=False,
                session=session,
                reranker=reranker_instance,
                retrieve_top_k=getattr(args, "retrieve_top_k", 20),
                rerank_top_k=getattr(args, "rerank_top_k", 5),
                hybrid_alpha=getattr(args, "hybrid_alpha", 0.5),
                quiet=True,
            )
        except Exception as e:
            answer = ""
            sources = []
            print(f"Erro para pergunta: {e}", file=sys.stderr)
        latency_ms = int((time.perf_counter() - t0) * 1000)
        out = {
            "question": question,
            "answer": answer,
            "latency_ms": latency_ms,
        }
        if getattr(args, "show_sources_in_batch", False):
            out["sources"] = [
                {"id": r.get("id"), "score": r.get("score"), "source": (r.get("metadata") or {}).get("source")}
                for r in sources
            ]
        print(json.dumps(out, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser(description="RAG com FerresDB: ingestão + query interativa")
    subparsers = parser.add_subparsers(dest="command", help="comando")

    # ingest
    ingest_parser = subparsers.add_parser("ingest", help="Ingerir documentos (reutiliza pipeline de examples/ingestion)")
    ingest_parser.add_argument("--source", "-s", required=True, help="Diretório ou arquivo fonte")
    ingest_parser.add_argument("--collection", "-c", required=True, help="Nome da coleção")
    ingest_parser.add_argument("--embedding", "-e", choices=["openai", "cohere", "local"], default="openai")
    ingest_parser.add_argument("--chunker", default="semantic", choices=["semantic", "fixed", "markdown"])
    ingest_parser.add_argument("--chunk-size", type=int, default=512)
    ingest_parser.add_argument("--server", default="http://localhost:3000")
    ingest_parser.add_argument("--cache", action="store_true")
    ingest_parser.add_argument("--workers", "-w", type=int, default=4)

    # Opções do modo query (default, sem subcomando)
    parser.add_argument("--collection", "-c", help="Nome da coleção")
    parser.add_argument("--llm", choices=["openai", "anthropic"], default="openai")
    parser.add_argument("--llm-model", help="Modelo LLM")
    parser.add_argument("--embedding", "-e", choices=["openai", "local"], default="openai")
    parser.add_argument("--server", default="http://localhost:3000")
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument("--rerank", action="store_true", help="Usar busca híbrida (top-20) + rerank (top-5). Requer BM25 na coleção.")
    parser.add_argument("--reranker", choices=["cross_encoder", "cohere", "llm"], default="cross_encoder", help="Backend do reranker (quando --rerank)")
    parser.add_argument("--rerank-top-k", type=int, default=5, help="Documentos após rerank (default: 5)")
    parser.add_argument("--retrieve-top-k", type=int, default=20, help="Documentos na retrieval quando --rerank (default: 20)")
    parser.add_argument("--hybrid-alpha", type=float, default=0.5, help="Peso vetorial na busca híbrida 0..1 (default: 0.5)")
    parser.add_argument("--show-chunks", action="store_true")
    parser.add_argument("--no-stream", action="store_true", dest="no_stream")
    parser.add_argument("--history", help="Arquivo de histórico JSONL")
    parser.add_argument("--feedback", action="store_true", help="Após cada resposta, perguntar Útil/Não útil e salvar em feedback.jsonl")
    parser.add_argument("--feedback-file", help="Caminho do feedback.jsonl (default: data/logs/feedback.jsonl para o dashboard)")
    parser.add_argument("--session-file", help="Gravar sessão (fluxo completo) em JSONL para replay (default: data/sessions/<session_id>.jsonl)")
    parser.add_argument("--questions-file", help="Arquivo com uma pergunta por linha (modo batch: imprime JSONL com question, answer, latency_ms)")

    args = parser.parse_args()

    if args.command == "ingest":
        run_ingest(args)
        return

    # Default: query interativa ou batch
    if not getattr(args, "collection", None):
        parser.error("--collection é obrigatório para o modo query. Use: python app.py --collection docs --llm openai")
    args.stream = not getattr(args, "no_stream", True)

    if getattr(args, "questions_file", None):
        batch_loop(args)
        return
    interactive_loop(args)


if __name__ == "__main__":
    main()
