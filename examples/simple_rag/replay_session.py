#!/usr/bin/env python3
"""
Replay de sessão gravada (PoC): lê um arquivo de sessão JSONL e exibe o fluxo para análise.

Uso:
  python replay_session.py data/sessions/<session_id>.jsonl
  python replay_session.py --compact data/sessions/abc123.jsonl
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def replay(path: Path, compact: bool = False) -> None:
    if not path.exists():
        print(f"Arquivo não encontrado: {path}", file=sys.stderr)
        sys.exit(1)
    lines = path.read_text(encoding="utf-8").strip().splitlines()
    for i, line in enumerate(lines, 1):
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            print(f"[linha {i}] (JSON inválido)", file=sys.stderr)
            continue
        ts = event.get("ts", "")
        ev_type = event.get("event", "?")
        if ev_type == "query":
            q = event.get("question", "")
            a = event.get("answer", "")[:200] + ("..." if len(event.get("answer", "")) > 200 else "")
            latency = event.get("latency_ms", "")
            if compact:
                print(f"[{ts}] Q: {q[:60]}... | {latency} ms")
            else:
                print("\n" + "=" * 60)
                print(f"Turn {event.get('turn_index', '?')} | {ts}")
                print("Pergunta:", q)
                print("Resposta (início):", a)
                print(f"Latência: {latency} ms | fontes: {len(event.get('sources', []))}")
        elif ev_type == "feedback":
            useful = event.get("useful", "?")
            comment = event.get("comment")
            if compact:
                print(f"  -> útil={useful}" + (f" | {comment}" if comment else ""))
            else:
                print(f"  Feedback: {'Útil' if useful else 'Não útil'}" + (f" | Comentário: {comment}" if comment else ""))


def main() -> None:
    parser = argparse.ArgumentParser(description="Replay de sessão JSONL para análise")
    parser.add_argument("session_file", type=Path, help="Arquivo da sessão (ex.: data/sessions/<id>.jsonl)")
    parser.add_argument("--compact", "-c", action="store_true", help="Saída resumida (uma linha por query)")
    args = parser.parse_args()
    replay(args.session_file, compact=args.compact)


if __name__ == "__main__":
    main()
