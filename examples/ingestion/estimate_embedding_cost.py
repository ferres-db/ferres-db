#!/usr/bin/env python3
"""
Calcula custo estimado de embeddings e pede confirmação antes de gerar.

Uso:
  py estimate_embedding_cost.py --provider openai --texts-file chunks.txt
  py estimate_embedding_cost.py --provider openai --texts "foo" "bar"
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

# Adiciona o diretório do exemplo ao path
ROOT = Path(__file__).resolve().parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

# Preços por 1K tokens (ajuste conforme a documentação oficial)
OPENAI_PRICE_PER_1K = 0.00002   # text-embedding-3-small
COHERE_PRICE_PER_1K = 0.0001    # embed-english-v3.0 (valor referência; conferir em cohere.com/pricing)


def get_openai_token_count(texts: list[str]) -> int:
    try:
        import tiktoken
    except ImportError:
        return sum(len(t) // 4 for t in texts)
    try:
        enc = tiktoken.encoding_for_model("gpt-4")
    except Exception:
        enc = tiktoken.get_encoding("cl100k_base")
    total = 0
    for t in texts:
        total += len(enc.encode(t))
    return total


def estimate_openai_cost(texts: list[str], model: str = "text-embedding-3-small") -> tuple[int, float]:
    tokens = get_openai_token_count(texts)
    # text-embedding-3-small: $0.02/1M tokens
    cost = (tokens / 1_000) * OPENAI_PRICE_PER_1K
    return tokens, cost


def estimate_cohere_cost(texts: list[str]) -> tuple[int, float]:
    # Cohere cobra por caractere/token de entrada; usar aproximação por tamanho
    total_chars = sum(len(t) for t in texts)
    approx_tokens = total_chars // 4
    cost = (approx_tokens / 1_000) * COHERE_PRICE_PER_1K
    return approx_tokens, cost


def estimate_local_cost(_texts: list[str]) -> tuple[int, float]:
    return 0, 0.0


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Estima custo de embeddings e opcionalmente pede confirmação."
    )
    parser.add_argument(
        "--provider",
        choices=["openai", "cohere", "local"],
        required=True,
        help="Provedor de embeddings",
    )
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "--texts-file",
        type=Path,
        help="Arquivo com um texto por linha (ou JSONL com campo 'text')",
    )
    group.add_argument(
        "--texts",
        nargs="+",
        help="Lista de textos na linha de comando",
    )
    parser.add_argument(
        "--yes",
        "-y",
        action="store_true",
        help="Pular confirmação (apenas exibir custo)",
    )
    parser.add_argument(
        "--run",
        action="store_true",
        help="Após confirmação, rodar embeddings (usa embeddings.embed_texts)",
    )
    args = parser.parse_args()

    if args.texts_file:
        path = args.texts_file
        if not path.is_absolute():
            path = ROOT / path
        if not path.exists():
            print(f"Erro: arquivo não encontrado: {path}", file=sys.stderr)
            sys.exit(1)
        raw = path.read_text(encoding="utf-8", errors="replace")
        lines = [ln.strip() for ln in raw.splitlines() if ln.strip()]
        # Se for JSONL com campo "text"
        if lines and lines[0].startswith("{"):
            import json
            texts = []
            for ln in lines:
                try:
                    texts.append(json.loads(ln).get("text", ln))
                except Exception:
                    texts.append(ln)
        else:
            texts = lines
    else:
        texts = args.texts or []

    if not texts:
        print("Nenhum texto para estimar.", file=sys.stderr)
        sys.exit(1)

    if args.provider == "openai":
        tokens, cost = estimate_openai_cost(texts)
        print(f"Provedor: OpenAI (text-embedding-3-small)")
        print(f"Textos:   {len(texts)}")
        print(f"Tokens:   ~{tokens:,}")
        print(f"Custo:    ~${cost:.4f} USD")
    elif args.provider == "cohere":
        tokens, cost = estimate_cohere_cost(texts)
        print(f"Provedor: Cohere")
        print(f"Textos:   {len(texts)}")
        print(f"Tokens:   ~{tokens:,} (aprox.)")
        print(f"Custo:    ~${cost:.4f} USD (referência; conferir cohere.com/pricing)")
    else:
        tokens, cost = estimate_local_cost(texts)
        print(f"Provedor: Local (sentence-transformers)")
        print(f"Textos:   {len(texts)}")
        print(f"Custo:    $0 (local)")

    if args.provider != "local" and cost > 0 and not args.yes:
        try:
            resp = input("\nContinuar e gerar embeddings? [y/N] ").strip().lower()
            if resp not in ("y", "yes"):
                print("Cancelado.")
                sys.exit(0)
        except EOFError:
            print("Cancelado (sem input).")
            sys.exit(0)

    if args.run:
        from embeddings import (
            embed_texts,
            LocalEmbeddings,
            OpenAIEmbeddings,
            CohereEmbeddings,
        )
        api_key = os.environ.get("OPENAI_API_KEY") if args.provider == "openai" else os.environ.get("COHERE_API_KEY")
        if args.provider == "openai":
            provider = OpenAIEmbeddings(api_key=api_key)
        elif args.provider == "cohere":
            provider = CohereEmbeddings(api_key=api_key)
        else:
            provider = LocalEmbeddings()
        embs = embed_texts(
            provider,
            texts,
            use_cache=True,
            requests_per_minute=60,
            retries=3,
            show_progress=True,
        )
        print(f"Gerados {len(embs)} embeddings (dimensão {len(embs[0])}).")


if __name__ == "__main__":
    main()
