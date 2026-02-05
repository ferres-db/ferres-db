#!/usr/bin/env python3
"""
Monitor ativo para PoC: tail de queries.log, detecção de anomalias e notificação por webhook.

Anomalias:
- Latência > 5s
- Zero resultados retornados
- Mesma query repetida 3x (mesmo vector_preview + collection em janela recente)

Uso:
  python monitor.py --log-file ../data/logs/queries.log
  WEBHOOK_URL=https://hooks.slack.com/... python monitor.py
  WEBHOOK_URL=https://discord.com/api/webhooks/... python monitor.py
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from collections import deque
from pathlib import Path

try:
    import requests
except ImportError:
    requests = None  # type: ignore


def load_dotenv() -> None:
    from pathlib import Path
    env = Path(__file__).resolve().parent / ".env"
    if env.exists():
        for line in env.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, _, v = line.partition("=")
                k, v = k.strip(), v.strip().strip('"').strip("'")
                if k and k not in os.environ:
                    os.environ[k] = v


# Carrega .env se existir
load_dotenv()

LATENCY_THRESHOLD_MS = 5000
REPEAT_WINDOW = 20  # últimas N entradas para considerar "mesma query 3x"
REPEAT_COUNT = 3


def tail_lines(path: Path, from_end: bool = True):
    """Abre o arquivo e produz linhas novas (tail -f). Se from_end=True, começa do fim."""
    path = path.resolve()
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        if from_end:
            f.seek(0, 2)
        while True:
            line = f.readline()
            if line:
                yield line.rstrip("\n")
            else:
                time.sleep(0.2)


def parse_log_line(line: str) -> dict | None:
    line = line.strip()
    if not line:
        return None
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return None


def key_for_repeat(entry: dict) -> tuple:
    """Chave para detectar repetição: (collection, vector_preview)."""
    return (
        entry.get("collection") or "",
        entry.get("vector_preview") or "",
    )


def send_webhook(webhook_url: str, text: str, is_slack: bool) -> bool:
    if not webhook_url.strip():
        return False
    if requests is None:
        print("[monitor] requests não instalado; webhook ignorado.", file=sys.stderr)
        return False
    try:
        if "slack.com" in webhook_url:
            payload = {"text": text}
        else:
            # Discord
            payload = {"content": text[:2000]}
        r = requests.post(webhook_url, json=payload, timeout=10)
        if r.status_code >= 400:
            print(f"[monitor] webhook status {r.status_code}: {r.text[:200]}", file=sys.stderr)
            return False
        return True
    except Exception as e:
        print(f"[monitor] webhook error: {e}", file=sys.stderr)
        return False


def run_monitor(
    log_file: Path,
    webhook_url: str,
    latency_threshold_ms: int = LATENCY_THRESHOLD_MS,
    repeat_window: int = REPEAT_WINDOW,
    repeat_count: int = REPEAT_COUNT,
) -> None:
    recent_keys: deque = deque(maxlen=repeat_window)
    key_counts: dict[tuple, int] = {}
    last_notified_repeat: set[tuple] = set()

    for line in tail_lines(log_file):
        entry = parse_log_line(line)
        if not entry:
            continue

        query_id = entry.get("query_id", "?")
        collection = entry.get("collection", "?")
        took_ms = entry.get("took_ms", 0)
        results_count = entry.get("results_count", 0)
        ts = entry.get("timestamp", "")

        # 1) Latência > threshold
        if took_ms >= latency_threshold_ms:
            msg = (
                f"⚠️ *Latência alta* ({took_ms} ms)\n"
                f"Collection: `{collection}` | query_id: `{query_id}` | {ts}"
            )
            print(f"[anomaly] {msg.replace(chr(10), ' ')}")
            if webhook_url:
                send_webhook(webhook_url, msg, "slack.com" in webhook_url)

        # 2) Zero resultados
        if results_count == 0:
            msg = (
                f"⚠️ *Zero resultados*\n"
                f"Collection: `{collection}` | query_id: `{query_id}` | {ts}"
            )
            print(f"[anomaly] {msg.replace(chr(10), ' ')}")
            if webhook_url:
                send_webhook(webhook_url, msg, "slack.com" in webhook_url)

        # 3) Mesma query repetida N vezes (janela deslizante)
        key = key_for_repeat(entry)
        if len(recent_keys) == repeat_window:
            old_key = recent_keys[0]
            key_counts[old_key] = key_counts.get(old_key, 1) - 1
            if key_counts[old_key] <= 0:
                key_counts.pop(old_key, None)
                last_notified_repeat.discard(old_key)
        recent_keys.append(key)
        key_counts[key] = key_counts.get(key, 0) + 1
        if key_counts[key] >= repeat_count and key not in last_notified_repeat:
            last_notified_repeat.add(key)
            preview = (key[1] or "")[:60]
            msg = (
                f"⚠️ *Mesma query repetida {repeat_count}x* (usuário não achou?)\n"
                f"Collection: `{collection}` | vector_preview: `{preview}...` | {ts}"
            )
            print(f"[anomaly] {msg.replace(chr(10), ' ')}")
            if webhook_url:
                send_webhook(webhook_url, msg, "slack.com" in webhook_url)


def main() -> None:
    parser = argparse.ArgumentParser(description="Monitor de queries.log com alertas por webhook")
    parser.add_argument(
        "--log-file",
        type=Path,
        default=Path(os.environ.get("QUERIES_LOG", "data/logs/queries.log")),
        help="Caminho para queries.log (default: data/logs/queries.log ou QUERIES_LOG)",
    )
    parser.add_argument(
        "--webhook",
        default=os.environ.get("WEBHOOK_URL", ""),
        help="URL do webhook Slack ou Discord (ou env WEBHOOK_URL)",
    )
    parser.add_argument(
        "--latency-threshold-ms",
        type=int,
        default=LATENCY_THRESHOLD_MS,
        help=f"Alertar se latência > N ms (default: {LATENCY_THRESHOLD_MS})",
    )
    parser.add_argument(
        "--repeat-count",
        type=int,
        default=REPEAT_COUNT,
        help=f"Alertar se mesma query repetida N vezes (default: {REPEAT_COUNT})",
    )
    args = parser.parse_args()

    webhook_url = (args.webhook or os.environ.get("WEBHOOK_URL") or "").strip()
    if not webhook_url:
        print("Aviso: WEBHOOK_URL não definido; alertas só no console.", file=sys.stderr)

    print(
        f"Monitorando {args.log_file} (latência > {args.latency_threshold_ms} ms, repetição {args.repeat_count}x)"
    )
    run_monitor(
        args.log_file,
        webhook_url,
        latency_threshold_ms=args.latency_threshold_ms,
        repeat_count=args.repeat_count,
    )


if __name__ == "__main__":
    main()
