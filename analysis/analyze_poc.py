#!/usr/bin/env python3
"""
Análise do PoC: queries, qualidade e feedback.

Gera relatório Markdown com gráficos (matplotlib) e métricas agregadas.

Uso:
  python analyze_poc.py --query-log data/logs/queries.log --output-dir analysis/out
  python analyze_poc.py --query-log data/logs/queries.log --feedback data/logs/feedback.jsonl \\
    --sessions-dir examples/simple_rag/data/sessions --eval-report-before eval_report.json \\
    --eval-report-after eval_report_new.json --output-dir analysis/out
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# Optional deps (matplotlib, wordcloud) para gráficos
try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    HAS_MATPLOTLIB = True
except ImportError:
    HAS_MATPLOTLIB = False

try:
    from wordcloud import WordCloud
    HAS_WORDCLOUD = True
except ImportError:
    HAS_WORDCLOUD = False

REPO_ROOT = Path(__file__).resolve().parent.parent
SIMPLE_RAG = REPO_ROOT / "examples" / "simple_rag"


def load_query_log(path: Path) -> List[Dict[str, Any]]:
    """Carrega queries.log (JSONL)."""
    if not path.exists():
        return []
    entries = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            entries.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return entries


def load_feedback_jsonl(path: Path) -> List[Dict[str, Any]]:
    """Carrega feedback.jsonl (Útil/Não útil + comentário)."""
    if not path.exists():
        return []
    entries = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            entries.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return entries


def load_form_responses(path: Path) -> List[Dict[str, Any]]:
    """Carrega respostas do formulário (CSV ou JSON/JSONL)."""
    if not path.exists():
        return []
    text = path.read_text(encoding="utf-8")
    if path.suffix.lower() == ".json":
        try:
            data = json.loads(text)
            return data if isinstance(data, list) else [data]
        except json.JSONDecodeError:
            return []
    if path.suffix.lower() == ".jsonl":
        return [json.loads(l) for l in text.splitlines() if l.strip()]
    if path.suffix.lower() == ".csv":
        lines = [l.strip() for l in text.splitlines() if l.strip()]
        if not lines:
            return []
        import csv
        from io import StringIO
        reader = csv.DictReader(StringIO(text))
        return list(reader)
    return []


def extract_questions_from_sessions(sessions_dir: Path) -> List[str]:
    """Extrai todas as perguntas dos arquivos de sessão (event=query)."""
    questions = []
    if not sessions_dir.exists():
        return questions
    for f in sessions_dir.glob("*.jsonl"):
        for line in f.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
                if ev.get("event") == "query" and ev.get("question"):
                    questions.append(ev["question"].strip())
            except json.JSONDecodeError:
                continue
    return questions


# -----------------------------------------------------------------------------
# 1. Query analysis
# -----------------------------------------------------------------------------


def query_analysis(
    entries: List[Dict[str, Any]],
    query_texts: Optional[List[str]] = None,
    figs_dir: Optional[Path] = None,
) -> Dict[str, Any]:
    """Distribuições por latência, nº resultados, filtros; top-10 queries; taxa zero results."""
    if not entries:
        return {
            "total_queries": 0,
            "latency_ms": [],
            "results_count": [],
            "with_filter_pct": 0.0,
            "zero_results_pct": 0.0,
            "top_queries": [],
        }

    took_ms = [e.get("took_ms", 0) for e in entries if "took_ms" in e]
    results_count = [e.get("results_count", 0) for e in entries]
    with_filter = sum(1 for e in entries if e.get("filter") is not None and e.get("filter") != {})
    zero_results = sum(1 for c in results_count if c == 0)
    total = len(entries)

    # Top-10 queries: por texto (se fornecido) ou por "padrão" (results_count, limit, has_filter)
    top_queries: List[Tuple[str, int]] = []
    if query_texts:
        counter = Counter(q.strip().lower() for q in query_texts if q and q.strip())
        top_queries = counter.most_common(10)
    else:
        # Padrão: (results_count, limit, has_filter) como proxy
        pattern_counter: Counter = Counter()
        for e in entries:
            key = (e.get("results_count", 0), e.get("limit", 0), e.get("filter") is not None and e.get("filter") != {})
            pattern_counter[key] += 1
        top_queries = [(f"results={r}, limit={l}, filter={f}", c) for (r, l, f), c in pattern_counter.most_common(10)]

    out = {
        "total_queries": total,
        "latency_ms": took_ms,
        "results_count": results_count,
        "with_filter_pct": (with_filter / total * 100) if total else 0.0,
        "zero_results_pct": (zero_results / total * 100) if total else 0.0,
        "top_queries": [{"query": q, "count": c} for q, c in top_queries],
        "latency_mean_ms": sum(took_ms) / len(took_ms) if took_ms else 0,
        "latency_p50_ms": sorted(took_ms)[len(took_ms) // 2] if took_ms else 0,
        "latency_p95_ms": sorted(took_ms)[int(len(took_ms) * 0.95)] if took_ms else 0,
    }

    if HAS_MATPLOTLIB and figs_dir and took_ms:
        figs_dir.mkdir(parents=True, exist_ok=True)
        # Histograma latência
        fig, ax = plt.subplots(figsize=(8, 4))
        ax.hist(took_ms, bins=min(50, max(10, len(set(took_ms)))), edgecolor="black", alpha=0.7)
        ax.set_xlabel("Latência (ms)")
        ax.set_ylabel("Número de queries")
        ax.set_title("Distribuição de latência das queries")
        fig.tight_layout()
        fig.savefig(figs_dir / "query_latency_hist.png", dpi=120)
        plt.close(fig)

        # Distribuição número de resultados
        fig, ax = plt.subplots(figsize=(8, 4))
        rc_counts = Counter(results_count)
        ax.bar(list(rc_counts.keys()), list(rc_counts.values()), edgecolor="black", alpha=0.7)
        ax.set_xlabel("Número de resultados")
        ax.set_ylabel("Frequência")
        ax.set_title("Distribuição do número de resultados por query")
        fig.tight_layout()
        fig.savefig(figs_dir / "query_results_count.png", dpi=120)
        plt.close(fig)

        # Uso de filtros (pie)
        fig, ax = plt.subplots(figsize=(5, 5))
        ax.pie(
            [with_filter, total - with_filter],
            labels=["Com filtro", "Sem filtro"],
            autopct="%1.1f%%",
            startangle=90,
        )
        ax.set_title("Uso de filtros")
        fig.tight_layout()
        fig.savefig(figs_dir / "query_filter_usage.png", dpi=120)
        plt.close(fig)

    return out


# -----------------------------------------------------------------------------
# 2. Quality analysis
# -----------------------------------------------------------------------------


def run_evaluate_with_user_questions(
    questions: List[str],
    collection: str = "docs",
    server: str = "http://localhost:3000",
    embedding: str = "openai",
    no_llm: bool = True,
    report_path: Optional[Path] = None,
) -> Optional[Dict[str, Any]]:
    """Roda evaluate.py com lista de perguntas reais. Gera JSONL temporário (sem expected_docs)."""
    if not questions:
        return None
    rag_dir = SIMPLE_RAG
    if not (rag_dir / "evaluate.py").exists():
        return None
    tmp_questions = rag_dir / "_tmp_user_questions.jsonl"
    with open(tmp_questions, "w", encoding="utf-8") as f:
        for q in questions:
            f.write(json.dumps({"question": q, "expected_docs": [], "keywords": []}, ensure_ascii=False) + "\n")
    report_file = report_path or (rag_dir / "_tmp_eval_report_user.json")
    cmd = [
        sys.executable,
        str(rag_dir / "evaluate.py"),
        "--collection", collection,
        "--questions", str(tmp_questions),
        "--report", str(report_file),
        "--server", server,
        "--embedding", embedding,
    ]
    if no_llm:
        cmd.append("--no-llm")
    try:
        subprocess.run(cmd, cwd=str(rag_dir), check=True, capture_output=True, timeout=300)
        if report_file.exists():
            return json.loads(report_file.read_text(encoding="utf-8"))
    except (subprocess.CalledProcessError, FileNotFoundError, json.JSONDecodeError):
        pass
    finally:
        if tmp_questions.exists():
            tmp_questions.unlink(missing_ok=True)
    return None


def load_eval_report(path: Path) -> Optional[Dict[str, Any]]:
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return None


def quality_analysis(
    eval_report_before: Optional[Dict[str, Any]],
    eval_report_after: Optional[Dict[str, Any]],
    user_questions: List[str],
    run_eval: bool,
    collection: str,
    server: str,
    embedding: str,
    figs_dir: Optional[Path] = None,
) -> Dict[str, Any]:
    """Compara recall antes/depois; correlaciona latência com qualidade."""
    out = {
        "recall_before": None,
        "recall_after": None,
        "recall_delta": None,
        "mrr_before": None,
        "mrr_after": None,
        "latency_quality_correlation": None,
        "user_questions_eval": None,
    }

    if eval_report_before:
        m = eval_report_before.get("metrics") or {}
        out["recall_before"] = m.get("recall_at_5_mean")
        out["mrr_before"] = m.get("mrr_at_5_mean")
    if eval_report_after:
        m = eval_report_after.get("metrics") or {}
        out["recall_after"] = m.get("recall_at_5_mean")
        out["mrr_after"] = m.get("mrr_at_5_mean")

    if out["recall_before"] is not None and out["recall_after"] is not None:
        out["recall_delta"] = out["recall_after"] - out["recall_before"]

    if run_eval and user_questions:
        report = run_evaluate_with_user_questions(
            user_questions, collection=collection, server=server, embedding=embedding, no_llm=True
        )
        if report:
            out["user_questions_eval"] = report.get("metrics")

    # Correlação latência x qualidade: usar per_question de um report
    report_for_corr = eval_report_after or eval_report_before
    if report_for_corr and figs_dir and HAS_MATPLOTLIB:
        per_q = report_for_corr.get("per_question") or []
        latencies = [p.get("latency_sec") for p in per_q if p.get("latency_sec") is not None]
        recalls = [p.get("recall_at_5") for p in per_q if p.get("recall_at_5") is not None]
        if len(latencies) == len(recalls) and len(latencies) >= 3:
            import math
            n = len(latencies)
            mx, my = sum(latencies) / n, sum(recalls) / n
            sx = math.sqrt(sum((x - mx) ** 2 for x in latencies) / n)
            sy = math.sqrt(sum((y - my) ** 2 for y in recalls) / n)
            if sx and sy:
                r = sum((latencies[i] - mx) * (recalls[i] - my) for i in range(n)) / (n * sx * sy)
                out["latency_quality_correlation"] = round(r, 4)
            fig, ax = plt.subplots(figsize=(6, 4))
            ax.scatter([x * 1000 for x in latencies], recalls, alpha=0.7)
            ax.set_xlabel("Latência (ms)")
            ax.set_ylabel("Recall@5")
            ax.set_title("Correlação latência vs qualidade (Recall@5)")
            fig.tight_layout()
            fig.savefig(figs_dir / "latency_vs_quality.png", dpi=120)
            plt.close(fig)

    return out


# -----------------------------------------------------------------------------
# 3. Feedback analysis
# -----------------------------------------------------------------------------


def tokenize_pt(text: str) -> List[str]:
    """Tokenização simples para PT: palavras com 3+ chars, sem stopwords comuns."""
    if not text or not isinstance(text, str):
        return []
    stop = {"que", "com", "para", "uma", "mais", "muito", "sobre", "como", "quando", "sobre", "sobre", "isso", "essa", "este", "esta", "foi", "são", "tem", "por", "sem", "nos", "nas", "dos", "das", "the", "and", "for", "was", "were", "have", "has", "not", "but", "what", "when", "where"}
    text = re.sub(r"[^\w\sáéíóúàèìòùãõâêîôûç]", " ", text.lower())
    words = [w for w in text.split() if len(w) >= 3 and w not in stop]
    return words


def feedback_analysis(
    feedback_entries: List[Dict[str, Any]],
    form_responses: List[Dict[str, Any]],
    figs_dir: Optional[Path] = None,
    use_llm_themes: bool = False,
) -> Dict[str, Any]:
    """Agrega feedback; extrai temas (manual/LLM); word cloud."""
    useful = sum(1 for e in feedback_entries if e.get("useful") is True)
    not_useful = sum(1 for e in feedback_entries if e.get("useful") is False)
    total_fb = len(feedback_entries)
    comments = [e.get("comment") or "" for e in feedback_entries if e.get("comment")]

    # Form: escalas e textos abertos
    form_escalas: Dict[str, List[float]] = {}
    open_texts: List[str] = []
    for row in form_responses:
        for k, v in row.items():
            k_lower = k.lower()
            if k_lower in ("facilidade", "qualidade", "velocidade") and v:
                try:
                    form_escalas.setdefault(k, []).append(float(v))
                except (ValueError, TypeError):
                    pass
            if "funcionou" in k_lower or "esperava" in k_lower or "comentário" in k_lower or "observação" in k_lower:
                if v and isinstance(v, str):
                    open_texts.append(v)

    all_text = " ".join(comments + open_texts)
    words = tokenize_pt(all_text)
    word_freq = Counter(words)
    common_themes = [w for w, _ in word_freq.most_common(30)]

    out = {
        "feedback_total": total_fb,
        "useful_count": useful,
        "not_useful_count": not_useful,
        "satisfaction_pct": (useful / total_fb * 100) if total_fb else None,
        "form_responses_count": len(form_responses),
        "form_escalas": {k: {"mean": sum(v) / len(v), "n": len(v)} for k, v in form_escalas.items() if v},
        "common_words": common_themes[:20],
        "themes_llm": None,
    }

    if use_llm_themes and (comments or open_texts):
        # Placeholder: poderia chamar OpenAI/Anthropic para extrair temas. Por ora deixamos manual.
        out["themes_llm"] = "Configure OPENAI_API_KEY e use --llm-themes para extração automática (opcional)."

    if HAS_WORDCLOUD and figs_dir and words:
        figs_dir.mkdir(parents=True, exist_ok=True)
        wc = WordCloud(width=800, height=400, background_color="white", max_words=80).generate_from_frequencies(dict(word_freq))
        fig, ax = plt.subplots(figsize=(10, 5))
        ax.imshow(wc, interpolation="bilinear")
        ax.axis("off")
        ax.set_title("Palavras mais citadas (feedback e formulário)")
        fig.tight_layout()
        fig.savefig(figs_dir / "feedback_wordcloud.png", dpi=120)
        plt.close(fig)

    return out


# -----------------------------------------------------------------------------
# 4. Relatório final
# -----------------------------------------------------------------------------


def write_report(
    query_stats: Dict[str, Any],
    quality_stats: Dict[str, Any],
    feedback_stats: Dict[str, Any],
    bugs: List[str],
    next_steps: List[str],
    figs_dir: Path,
    output_path: Path,
) -> None:
    """Gera relatório Markdown com gráficos embedded."""
    figs_rel = output_path.parent / "figs"
    figs_rel.mkdir(parents=True, exist_ok=True)

    def img(name: str) -> str:
        p = figs_dir / name
        if p.exists():
            return f"![{name}]({figs_rel.name}/{name})\n"
        return ""

    lines = [
        "# Relatório de Análise do PoC",
        "",
        "## Sumário executivo",
        "",
        f"- **Total de queries analisadas:** {query_stats.get('total_queries', 0)}",
        f"- **Taxa de zero resultados:** {query_stats.get('zero_results_pct', 0):.1f}%",
        f"- **Uso de filtros:** {query_stats.get('with_filter_pct', 0):.1f}%",
        "",
    ]
    if quality_stats.get("recall_before") is not None:
        lines.append(f"- **Recall@5 (baseline):** {quality_stats['recall_before']:.2%}")
    if quality_stats.get("recall_after") is not None:
        lines.append(f"- **Recall@5 (após ajustes):** {quality_stats['recall_after']:.2%}")
    if quality_stats.get("recall_delta") is not None:
        d = quality_stats["recall_delta"]
        lines.append(f"- **Variação de recall:** {d:+.2%}")
    if feedback_stats.get("feedback_total"):
        sat = feedback_stats.get("satisfaction_pct")
        if sat is not None:
            lines.append(f"- **Feedback útil:** {feedback_stats.get('useful_count', 0)} / {feedback_stats['feedback_total']} ({sat:.0f}% satisfação)")
        else:
            lines.append(f"- **Feedback útil:** {feedback_stats.get('useful_count', 0)} / {feedback_stats['feedback_total']}")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Métricas de performance")
    lines.append("")
    lines.append(f"- Latência média: **{query_stats.get('latency_mean_ms', 0):.0f} ms**")
    lines.append(f"- Latência P50: **{query_stats.get('latency_p50_ms', 0):.0f} ms**")
    lines.append(f"- Latência P95: **{query_stats.get('latency_p95_ms', 0):.0f} ms**")
    lines.append("")
    lines.append("### Distribuição de latência")
    lines.append("")
    lines.append(img("query_latency_hist.png"))
    lines.append("### Número de resultados por query")
    lines.append("")
    lines.append(img("query_results_count.png"))
    lines.append("### Uso de filtros")
    lines.append("")
    lines.append(img("query_filter_usage.png"))
    lines.append("")
    if (quality_stats.get("latency_quality_correlation") is not None or (figs_dir / "latency_vs_quality.png").exists()):
        lines.append("### Correlação latência vs qualidade")
        lines.append("")
        lines.append(img("latency_vs_quality.png"))
        if quality_stats.get("latency_quality_correlation") is not None:
            lines.append(f"\nCorrelação (Pearson): **{quality_stats['latency_quality_correlation']}**\n")
    lines.append("### Top-10 queries mais frequentes")
    lines.append("")
    for i, item in enumerate(query_stats.get("top_queries", [])[:10], 1):
        lines.append(f"{i}. `{item['query'][:80]}{'...' if len(item['query']) > 80 else ''}` — {item['count']} ocorrências")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Feedback dos usuários")
    lines.append("")
    lines.append(f"- Respostas úteis: **{feedback_stats.get('useful_count', 0)}**")
    lines.append(f"- Respostas não úteis: **{feedback_stats.get('not_useful_count', 0)}**")
    if feedback_stats.get("form_escalas"):
        lines.append("- Médias do formulário (escalas 1–5):")
        for k, v in feedback_stats["form_escalas"].items():
            lines.append(f"  - {k}: **{v['mean']:.2f}** (n={v['n']})")
    lines.append("")
    lines.append("### Palavras mais citadas")
    lines.append("")
    lines.append(", ".join(f"**{w}**" for w in feedback_stats.get("common_words", [])[:15]))
    lines.append("")
    lines.append(img("feedback_wordcloud.png"))
    if feedback_stats.get("themes_llm"):
        lines.append("### Temas (extração)")
        lines.append("")
        lines.append(feedback_stats["themes_llm"])
        lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Bugs encontrados")
    lines.append("")
    if bugs:
        for b in bugs:
            lines.append(f"- {b}")
    else:
        lines.append("- (Nenhum bug registrado nesta análise. Inclua manualmente ou via integração com issue tracker.)")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Próximos passos")
    lines.append("")
    if next_steps:
        for s in next_steps:
            lines.append(f"- {s}")
    else:
        lines.append("- Revisar queries com zero resultados e ajustar índice ou embeddings.")
        lines.append("- Reduzir latência P95 (cache, índice, modelo de embedding).")
        lines.append("- Incorporar feedback negativo nas próximas iterações de avaliação.")
    lines.append("")

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("\n".join(lines), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description="Análise do PoC: queries, qualidade e feedback")
    parser.add_argument("--query-log", type=Path, default=REPO_ROOT / "data" / "logs" / "queries.log", help="Caminho do queries.log")
    parser.add_argument("--feedback", type=Path, default=REPO_ROOT / "data" / "logs" / "feedback.jsonl", help="Caminho do feedback.jsonl")
    parser.add_argument("--form", type=Path, help="CSV ou JSON com respostas do formulário (escalas + perguntas abertas)")
    parser.add_argument("--sessions-dir", type=Path, default=SIMPLE_RAG / "data" / "sessions", help="Diretório de sessões JSONL para extrair perguntas reais")
    parser.add_argument("--user-questions", type=Path, help="JSONL com uma pergunta por linha (question) para top-10 e avaliação")
    parser.add_argument("--eval-report-before", type=Path, help="Relatório evaluate.py antes dos ajustes")
    parser.add_argument("--eval-report-after", type=Path, help="Relatório evaluate.py depois dos ajustes")
    parser.add_argument("--run-eval-user", action="store_true", help="Rodar evaluate.py com perguntas reais (sessions ou --user-questions)")
    parser.add_argument("--collection", default="docs", help="Coleção para evaluate")
    parser.add_argument("--server", default="http://localhost:3000", help="URL do FerresDB")
    parser.add_argument("--embedding", default="openai", choices=["openai", "cohere", "local"])
    parser.add_argument("--llm-themes", action="store_true", help="Tentar extrair temas com LLM (requer API key)")
    parser.add_argument("--bugs", type=Path, help="Arquivo com um bug por linha (opcional)")
    parser.add_argument("--next-steps", type=Path, help="Arquivo com um próximo passo por linha (opcional)")
    parser.add_argument("--output-dir", "-o", type=Path, default=REPO_ROOT / "analysis" / "out", help="Diretório de saída (relatório + figs)")
    args = parser.parse_args()

    figs_dir = args.output_dir / "figs"
    figs_dir.mkdir(parents=True, exist_ok=True)

    # Query analysis
    entries = load_query_log(args.query_log)
    query_texts = None
    if args.user_questions and args.user_questions.exists():
        query_texts = [json.loads(l).get("question", "").strip() for l in args.user_questions.read_text(encoding="utf-8").splitlines() if l.strip()]
    if not query_texts and args.sessions_dir:
        query_texts = extract_questions_from_sessions(args.sessions_dir)
    query_stats = query_analysis(entries, query_texts=query_texts, figs_dir=figs_dir)

    # Quality analysis
    eval_before = load_eval_report(args.eval_report_before) if args.eval_report_before else None
    eval_after = load_eval_report(args.eval_report_after) if args.eval_report_after else None
    user_q = query_texts or ([] if not args.user_questions else [json.loads(l).get("question", "").strip() for l in args.user_questions.read_text(encoding="utf-8").splitlines() if l.strip()])
    quality_stats = quality_analysis(
        eval_before, eval_after,
        user_questions=user_q,
        run_eval=args.run_eval_user,
        collection=args.collection,
        server=args.server,
        embedding=args.embedding,
        figs_dir=figs_dir,
    )

    # Feedback analysis
    feedback_entries = load_feedback_jsonl(args.feedback)
    form_responses = load_form_responses(args.form) if args.form else []
    feedback_stats = feedback_analysis(
        feedback_entries, form_responses,
        figs_dir=figs_dir,
        use_llm_themes=args.llm_themes,
    )

    bugs = []
    if args.bugs and args.bugs.exists():
        bugs = [l.strip() for l in args.bugs.read_text(encoding="utf-8").splitlines() if l.strip()]
    next_steps = []
    if args.next_steps and args.next_steps.exists():
        next_steps = [l.strip() for l in args.next_steps.read_text(encoding="utf-8").splitlines() if l.strip()]

    report_path = args.output_dir / "report.md"
    write_report(query_stats, quality_stats, feedback_stats, bugs, next_steps, figs_dir, report_path)

    print(f"Relatório gerado: {report_path}")
    print(f"Gráficos: {figs_dir}")


if __name__ == "__main__":
    main()
