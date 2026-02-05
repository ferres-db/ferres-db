#!/usr/bin/env python3
"""
CLI de ingestão de documentos para FerresDB.

Descobre recursivamente .md, .pdf, .html, processa em paralelo,
gera embeddings e insere pontos na coleção. Suporta modo incremental
via manifesto e hash MD5. Inclui "text" no metadata dos pontos para uso em RAG.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from threading import Lock
from typing import Any, Dict, List, Optional, Tuple

ROOT = Path(__file__).resolve().parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

# Carrega variáveis de .env (pasta do script ou cwd)
from dotenv import load_dotenv
load_dotenv(ROOT / ".env")
load_dotenv()

from document_processor import (
    Chunk,
    Document,
    DocumentLoader,
    FixedSizeChunker,
    MarkdownHeaderChunker,
    SemanticChunker,
)
from embeddings import (
    CohereEmbeddings,
    EmbeddingProvider,
    LocalEmbeddings,
    OpenAIEmbeddings,
    embed_texts,
)

SUPPORTED_EXTENSIONS = {".md", ".pdf", ".html"}
# Servidor Axum tem limite de body (ex.: 2MB default); batches menores evitam 413 Payload Too Large
UPSERT_BATCH_SIZE = 100


def discover_files(source: Path) -> List[Path]:
    if not source.exists():
        return []
    if source.is_file():
        return [source] if source.suffix.lower() in SUPPORTED_EXTENSIONS else []
    return [
        p
        for p in source.rglob("*")
        if p.is_file() and p.suffix.lower() in SUPPORTED_EXTENSIONS
    ]


def file_md5(path: Path) -> str:
    h = hashlib.md5()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def load_and_chunk_file(
    path: Path,
    loader: DocumentLoader,
    chunker: Any,
) -> Tuple[Path, Optional[str], List[Chunk]]:
    try:
        suffix = path.suffix.lower()
        if suffix == ".md":
            docs = loader.load_markdown(path)
        elif suffix == ".pdf":
            docs = loader.load_pdf(path)
        elif suffix == ".html":
            docs = loader.load_html(path)
        else:
            return (path, f"Formato não suportado: {suffix}", [])
        chunks: List[Chunk] = []
        for doc in docs:
            chunks.extend(chunker.chunk(doc))
        return (path, None, chunks)
    except Exception as e:
        return (path, str(e), [])


def ensure_collection(
    server_base: str,
    collection: str,
    dimension: int,
    session: Any,
) -> bool:
    url = f"{server_base.rstrip('/')}/api/v1/collections"
    list_resp = session.get(url)
    if list_resp.status_code != 200:
        return False
    data = list_resp.json()
    names = [c["name"] for c in data.get("collections", [])]
    if collection in names:
        return True
    create_resp = session.post(
        url,
        json={"name": collection, "dimension": dimension, "distance": "Cosine"},
    )
    if create_resp.status_code in (200, 201):
        return True
    try:
        err = create_resp.json()
        print(f"Erro ao criar coleção: {err.get('message', create_resp.text)}", file=sys.stderr)
    except Exception:
        print(f"Erro ao criar coleção: {create_resp.status_code} {create_resp.text}", file=sys.stderr)
    return False


def upsert_batch(
    server_base: str,
    collection: str,
    points: List[Dict[str, Any]],
    session: Any,
) -> Tuple[int, List[Dict[str, str]]]:
    url = f"{server_base.rstrip('/')}/api/v1/collections/{collection}/points"
    payload = {"points": points}
    resp = session.post(url, json=payload)
    if resp.status_code != 200:
        try:
            err = resp.json()
            msg = err.get("message", resp.text)
        except Exception:
            msg = resp.text
        return (0, [{"id": p["id"], "reason": msg} for p in points])
    data = resp.json()
    return (data.get("upserted", 0), data.get("failed", []))


@dataclass
class Progress:
    files_ok: int = 0
    files_failed: int = 0
    chunks: int = 0
    embeddings: int = 0
    points_inserted: int = 0
    _lock: Lock = field(default_factory=Lock)

    def inc_files_ok(self) -> None:
        with self._lock:
            self.files_ok += 1

    def inc_files_failed(self) -> None:
        with self._lock:
            self.files_failed += 1

    def add_chunks(self, n: int) -> None:
        with self._lock:
            self.chunks += n

    def set_embeddings(self, n: int) -> None:
        with self._lock:
            self.embeddings = n

    def add_points(self, n: int) -> None:
        with self._lock:
            self.points_inserted += n

    def snapshot(self) -> Dict[str, int]:
        with self._lock:
            return {
                "files_ok": self.files_ok,
                "files_failed": self.files_failed,
                "chunks": self.chunks,
                "embeddings": self.embeddings,
                "points_inserted": self.points_inserted,
            }


def load_manifest(manifest_path: Path) -> Optional[Dict[str, Any]]:
    if not manifest_path.exists():
        return None
    try:
        return json.loads(manifest_path.read_text(encoding="utf-8"))
    except Exception:
        return None


def files_from_manifest(manifest: Dict[str, Any]) -> Dict[str, str]:
    entries = manifest.get("files", [])
    if isinstance(entries, list):
        return {e["path"]: e["md5"] for e in entries if isinstance(e, dict) and "path" in e and "md5" in e}
    return {}


def filter_incremental(
    paths: List[Path],
    previous_manifest: Optional[Dict[str, Any]],
) -> List[Path]:
    if not previous_manifest:
        return list(paths)
    prev = files_from_manifest(previous_manifest)
    result: List[Path] = []
    for p in paths:
        path_str = str(p.resolve())
        current_md5 = file_md5(p)
        if path_str not in prev or prev[path_str] != current_md5:
            result.append(p)
    return result


def get_embedding_provider(name: str) -> EmbeddingProvider:
    if name == "openai":
        return OpenAIEmbeddings(api_key=os.environ.get("OPENAI_API_KEY"))
    if name == "cohere":
        return CohereEmbeddings(api_key=os.environ.get("COHERE_API_KEY"))
    if name == "local":
        return LocalEmbeddings()
    raise ValueError(f"Embedding desconhecido: {name}. Use: openai, cohere, local")


def get_chunker(name: str, chunk_size: int) -> Any:
    if name == "semantic":
        return SemanticChunker(max_chunk_chars=chunk_size)
    if name == "fixed":
        overlap = max(0, min(50, chunk_size // 10))
        return FixedSizeChunker(chunk_size=chunk_size, overlap=overlap)
    if name == "markdown":
        return MarkdownHeaderChunker()
    raise ValueError(f"Chunker desconhecido: {name}. Use: semantic, fixed, markdown")


def estimate_embedding_cost(provider: EmbeddingProvider, texts: List[str]) -> float:
    if isinstance(provider, OpenAIEmbeddings):
        tokens = provider.count_tokens(texts)
        return (tokens / 1_000) * 0.00002
    if "Cohere" in type(provider).__name__:
        total_chars = sum(len(t) for t in texts)
        approx_tokens = total_chars // 4
        return (approx_tokens / 1_000) * 0.0001
    return 0.0


def run(args: argparse.Namespace) -> None:
    source = Path(args.source).resolve()
    collection = args.collection
    server_base = args.server
    manifest_path = Path(args.manifest).resolve() if args.manifest else (ROOT / "ingest_manifest.json")
    errors_log_path = Path(args.errors_log).resolve() if args.errors_log else (ROOT / "errors.log")
    workers = args.workers

    all_files = discover_files(source)
    if not all_files:
        print("Nenhum arquivo .md, .pdf ou .html encontrado em", source, file=sys.stderr)
        sys.exit(1)

    previous_manifest = load_manifest(manifest_path) if args.incremental else None
    files_to_process = filter_incremental(all_files, previous_manifest)
    if not files_to_process:
        print("Modo incremental: todos os arquivos já estão atualizados. Nada a processar.")
        return
    if args.incremental and len(files_to_process) < len(all_files):
        print(f"Incremental: processando {len(files_to_process)} de {len(all_files)} arquivos (novos ou modificados).")

    try:
        provider = get_embedding_provider(args.embedding)
        chunker = get_chunker(args.chunker, args.chunk_size)
    except (ValueError, ImportError) as e:
        print(f"Erro: {e}", file=sys.stderr)
        sys.exit(1)

    loader = DocumentLoader()
    progress = Progress()
    errors_log: List[str] = []
    all_chunks: List[Chunk] = []
    processed_paths: List[Path] = []
    file_hashes: Dict[str, str] = {}

    def on_file_done(future: Any) -> None:
        path, err, chunks = future.result()
        if err:
            progress.inc_files_failed()
            errors_log.append(f"{path}\t{err}")
        else:
            progress.inc_files_ok()
            progress.add_chunks(len(chunks))
            all_chunks.extend(chunks)
            processed_paths.append(path)
            file_hashes[str(path.resolve())] = file_md5(path)

    print("Arquivos:", len(files_to_process), "| Chunker:", args.chunker, "| Chunk size:", args.chunk_size)
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = [
            executor.submit(load_and_chunk_file, p, loader, chunker)
            for p in files_to_process
        ]
        for f in as_completed(futures):
            on_file_done(f)

    snap = progress.snapshot()
    print(f"  Arquivos processados: {snap['files_ok']} ok, {snap['files_failed']} falhas | Chunks: {snap['chunks']}")

    if errors_log:
        errors_log_path.parent.mkdir(parents=True, exist_ok=True)
        with open(errors_log_path, "a", encoding="utf-8") as ef:
            ef.write("\n".join(errors_log) + "\n")
        print(f"  Erros anotados em: {errors_log_path}")

    if not all_chunks:
        print("Nenhum chunk gerado. Encerrando.")
        _save_manifest(manifest_path, args, processed_paths, file_hashes, progress, 0.0)
        return

    texts = [c.text for c in all_chunks]
    cost_estimate = estimate_embedding_cost(provider, texts)
    if cost_estimate > 0:
        print(f"  Custo estimado embeddings: ~${cost_estimate:.4f} USD")
    embs = embed_texts(
        provider,
        texts,
        use_cache=args.cache,
        cache_path=args.cache_path or str(ROOT / ".embeddings_cache.sqlite"),
        requests_per_minute=args.rpm,
        retries=3,
        show_progress=True,
    )
    progress.set_embeddings(len(embs))
    print(f"  Embeddings criados: {len(embs)}")

    try:
        import requests
        session = requests.Session()
        session.headers.setdefault("Content-Type", "application/json")
    except ImportError:
        print("Erro: 'requests' é necessário. pip install requests", file=sys.stderr)
        sys.exit(1)

    dim = provider.dimension
    if not ensure_collection(server_base, collection, dim, session):
        print("Falha ao garantir coleção. Verifique o servidor e a URL.", file=sys.stderr)
        sys.exit(1)

    # Incluir "text" no metadata para uso em RAG (exemplos/simple_rag)
    points = [
        {
            "id": c.id,
            "vector": embs[i],
            "metadata": {**c.metadata, "text": c.text},
        }
        for i, c in enumerate(all_chunks)
    ]
    total_upserted = 0
    for i in range(0, len(points), UPSERT_BATCH_SIZE):
        batch = points[i : i + UPSERT_BATCH_SIZE]
        upserted, failed = upsert_batch(server_base, collection, batch, session)
        total_upserted += upserted
        progress.add_points(upserted)
        if failed:
            for f in failed:
                errors_log.append(f"point {f.get('id')}\t{f.get('reason')}")
    print(f"  Pontos inseridos: {total_upserted}")

    _save_manifest(manifest_path, args, processed_paths, file_hashes, progress, cost_estimate)
    print(f"  Manifesto salvo: {manifest_path}")


def _save_manifest(
    manifest_path: Path,
    args: argparse.Namespace,
    processed_paths: List[Path],
    file_hashes: Dict[str, str],
    progress: Progress,
    embedding_cost: float,
) -> None:
    snap = progress.snapshot()
    previous = load_manifest(manifest_path)
    files_entries = list(files_from_manifest(previous or {}).items())
    path_to_md5 = dict(files_entries)
    for p in processed_paths:
        path_str = str(p.resolve())
        path_to_md5[path_str] = file_hashes.get(path_str, file_md5(p))
    files_list = [{"path": path, "md5": md5} for path, md5 in path_to_md5.items()]

    manifest = {
        "timestamp": time.time(),
        "config": {
            "source": str(Path(args.source).resolve()),
            "collection": args.collection,
            "embedding": args.embedding,
            "chunker": args.chunker,
            "chunk_size": args.chunk_size,
            "server": args.server,
        },
        "files": files_list,
        "stats": {
            "files_processed": snap["files_ok"],
            "files_failed": snap["files_failed"],
            "total_chunks": snap["chunks"],
            "embeddings_created": snap["embeddings"],
            "points_inserted": snap["points_inserted"],
            "embedding_cost_usd": round(embedding_cost, 6),
        },
    }
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Ingestão de documentos para FerresDB (md, pdf, html).",
    )
    parser.add_argument("--source", "-s", required=True, help="Diretório ou arquivo fonte")
    parser.add_argument("--collection", "-c", required=True, help="Nome da coleção")
    parser.add_argument("--embedding", "-e", choices=["openai", "cohere", "local"], default="openai")
    parser.add_argument("--chunker", choices=["semantic", "fixed", "markdown"], default="semantic")
    parser.add_argument("--chunk-size", type=int, default=512)
    parser.add_argument("--server", default="http://localhost:3000")
    parser.add_argument("--manifest", "-m", help="Caminho do manifesto JSON")
    parser.add_argument("--incremental", "-i", action="store_true")
    parser.add_argument("--errors-log", default="errors.log")
    parser.add_argument("--workers", "-w", type=int, default=4)
    parser.add_argument("--cache", action="store_true")
    parser.add_argument("--cache-path", help="Caminho do cache SQLite")
    parser.add_argument("--rpm", type=int, default=60)
    args = parser.parse_args()
    run(args)


if __name__ == "__main__":
    main()
