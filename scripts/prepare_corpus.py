#!/usr/bin/env python3
"""
Prepara corpus real para PoC: coleta, limpeza, deduplicação e validação.

Fontes suportadas:
  1. Documentação interna (READMEs, docs/, wikis)
  2. Base de conhecimento (FAQs, tickets em Markdown/HTML)
  3. Artigos técnicos (blog posts salvos em .md/.html)

Uso:
  python scripts/prepare_corpus.py --sources docs/ README.md --output data/corpus_poc
  python scripts/prepare_corpus.py --config corpus_sources.toml --output data/corpus_poc

Saída: diretório com um .md por documento + manifest.json (contagem, tamanho, categorias).
Target: 500-1000 documentos reais.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# Opcional: BeautifulSoup para HTML
try:
    from bs4 import BeautifulSoup
except ImportError:
    BeautifulSoup = None  # type: ignore

# ---------------------------------------------------------------------------
# Constantes
# ---------------------------------------------------------------------------

SUPPORTED_EXTENSIONS = {".md", ".html", ".htm"}
MIN_DOC_CHARS = 50
MIN_ALPHA_RATIO = 0.1  # Mínimo de caracteres alfabéticos (evita docs só com números/símbolos)
DEFAULT_ENCODING = "utf-8"
MANIFEST_FILENAME = "manifest.json"


@dataclass
class RawDocument:
    """Documento bruto carregado da fonte."""

    source_path: str
    content: str
    category: str
    format: str  # "markdown" | "html"


@dataclass
class PreparedDocument:
    """Documento após limpeza e validação."""

    id: str  # hash ou slug
    content: str
    source_path: str
    category: str
    format: str
    size_bytes: int


# ---------------------------------------------------------------------------
# Coleta (crawl)
# ---------------------------------------------------------------------------


def discover_files(
    roots: List[Path],
    extensions: set[str],
    follow_symlinks: bool = False,
) -> List[Tuple[Path, str]]:
    """
    Descobre arquivos nos diretórios/arquivos indicados.
    Retorna lista de (path, categoria).
    Categoria é derivada do primeiro root que contém o path.
    """
    seen: set[Path] = set()
    result: List[Tuple[Path, str]] = []

    for root in roots:
        root = root.resolve()
        if not root.exists():
            continue
        if root.is_file():
            if root.suffix.lower() in extensions:
                result.append((root, _category_from_path(root, root.parent)))
            continue
        for path in root.rglob("*"):
            if not path.is_file() or (path.is_symlink() and not follow_symlinks):
                continue
            if path.suffix.lower() not in extensions or path in seen:
                continue
            seen.add(path)
            result.append((path, _category_from_path(path, root)))
    return result


def _category_from_path(path: Path, root: Path) -> str:
    """Deriva categoria do path (internal_docs, knowledge_base, articles)."""
    rel = path.relative_to(root) if root != path else path.name
    parts = rel.parts
    if not parts:
        return "internal_docs"
    first = parts[0].lower()
    if first in ("faq", "faqs", "kb", "knowledge", "tickets", "support"):
        return "knowledge_base"
    if first in ("blog", "articles", "posts", "tech"):
        return "articles"
    if first == "docs" or "readme" in path.name.lower():
        return "internal_docs"
    return "internal_docs"


# ---------------------------------------------------------------------------
# Carregamento e limpeza
# ---------------------------------------------------------------------------


def load_text(path: Path) -> str:
    """Carrega conteúdo de arquivo com fallback de encoding."""
    try:
        return path.read_text(encoding=DEFAULT_ENCODING, errors="replace")
    except Exception:
        return path.read_text(encoding="latin-1", errors="replace")


def clean_html(content: str) -> str:
    """Extrai e limpa texto de HTML."""
    if BeautifulSoup is None:
        # Fallback: remove tags com regex
        text = re.sub(r"<script[^>]*>[\s\S]*?</script>", "", content, flags=re.IGNORECASE)
        text = re.sub(r"<style[^>]*>[\s\S]*?</style>", "", text, flags=re.IGNORECASE)
        text = re.sub(r"<[^>]+>", " ", text)
        text = re.sub(r"\s+", " ", text)
        return text.strip()
    soup = BeautifulSoup(content, "html.parser")
    for tag in soup(["script", "style", "nav", "footer", "header"]):
        tag.decompose()
    text = soup.get_text(separator="\n", strip=True)
    return normalize_whitespace(text)


def clean_markdown(content: str) -> str:
    """Normaliza Markdown (remove apenas artefatos, preserva estrutura útil)."""
    text = content
    # Remove blocos de imagem que são a única coisa na linha (opcional: mantém alt text)
    text = re.sub(r"^!\[[^\]]*\]\([^)]+\)\s*$", "", text, flags=re.MULTILINE)
    return normalize_whitespace(text)


def normalize_whitespace(text: str) -> str:
    """Colapsa espaços e normaliza quebras de linha."""
    text = re.sub(r"[ \t]+", " ", text)
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text.strip()


def load_and_clean(path: Path, category: str) -> Optional[RawDocument]:
    """Carrega um arquivo e retorna RawDocument com conteúdo limpo."""
    try:
        raw = load_text(path)
    except Exception:
        return None
    suffix = path.suffix.lower()
    if suffix in (".html", ".htm"):
        content = clean_html(raw)
        fmt = "html"
    else:
        content = clean_markdown(raw)
        fmt = "markdown"
    return RawDocument(
        source_path=str(path.resolve()),
        content=content,
        category=category,
        format=fmt,
    )


# ---------------------------------------------------------------------------
# Deduplicação e validação
# ---------------------------------------------------------------------------


def content_hash(text: str) -> str:
    """Hash normalizado do conteúdo para deduplicação."""
    normalized = re.sub(r"\s+", " ", text.strip().lower())
    return hashlib.sha256(normalized.encode(DEFAULT_ENCODING)).hexdigest()[:16]


def has_minimum_text(text: str, min_chars: int = MIN_DOC_CHARS) -> bool:
    """Verifica se o documento tem texto suficiente (não é só imagem/numérico)."""
    if len(text.strip()) < min_chars:
        return False
    alpha = sum(1 for c in text if c.isalpha())
    total = len(text.strip()) or 1
    return (alpha / total) >= MIN_ALPHA_RATIO


# ---------------------------------------------------------------------------
# Escrita e manifesto
# ---------------------------------------------------------------------------


def safe_filename(doc_id: str, extension: str = ".md") -> str:
    """Nome de arquivo seguro para o documento."""
    return f"{doc_id}{extension}"


def write_corpus(output_dir: Path, documents: List[PreparedDocument]) -> None:
    """Escreve cada documento como um arquivo .md no diretório de saída."""
    output_dir.mkdir(parents=True, exist_ok=True)
    for doc in documents:
        path = output_dir / safe_filename(doc.id)
        path.write_text(doc.content, encoding=DEFAULT_ENCODING)


def build_manifest(
    documents: List[PreparedDocument],
    output_dir: Path,
    sources_used: List[str],
    stats: Dict[str, Any],
) -> Dict[str, Any]:
    """Monta o manifesto com contagem, tamanho e categorias."""
    by_category: Dict[str, int] = {}
    by_format: Dict[str, int] = {}
    for doc in documents:
        by_category[doc.category] = by_category.get(doc.category, 0) + 1
        by_format[doc.format] = by_format.get(doc.format, 0) + 1
    total_chars = sum(len(d.content) for d in documents)
    total_bytes = sum(d.size_bytes for d in documents)
    return {
        "version": 1,
        "num_documents": len(documents),
        "total_size_bytes": total_bytes,
        "total_size_chars": total_chars,
        "categories": by_category,
        "formats": by_format,
        "sources": sources_used,
        "stats": {
            **stats,
            "min_alpha_ratio": MIN_ALPHA_RATIO,
        },
    }


def load_config(config_path: Path) -> Tuple[List[Path], List[str]]:
    """
    Carrega configuração TOML ou JSON com listas de fontes.
    Retorna (lista de paths, lista de labels para manifesto).
    """
    path_str = str(config_path.resolve())
    if config_path.suffix.lower() == ".json":
        data = json.loads(config_path.read_text(encoding=DEFAULT_ENCODING))
        sources = data.get("sources", [])
        paths = [Path(s) if isinstance(s, str) else Path(s.get("path", s.get("dir", ""))) for s in sources]
        return (paths, [str(p) for p in paths])
    if config_path.suffix.lower() == ".toml":
        try:
            import tomllib
            with open(config_path, "rb") as f:
                data = tomllib.load(f)
        except ImportError:
            try:
                import toml
                data = toml.load(path_str)
            except ImportError:
                data = {}
        sources = data.get("sources", [])
        paths, labels = [], []
        for s in sources:
            if isinstance(s, str):
                paths.append(Path(s))
                labels.append(s)
            elif isinstance(s, dict):
                p = s.get("path") or s.get("dir")
                if p:
                    paths.append(Path(p))
                    labels.append(str(p))
        return (paths, labels)
    return ([], [])


# ---------------------------------------------------------------------------
# Pipeline principal
# ---------------------------------------------------------------------------


def run(
    sources: List[Path],
    output_dir: Path,
    extensions: set[str] | None = None,
    min_chars: int = MIN_DOC_CHARS,
    dry_run: bool = False,
) -> Dict[str, Any]:
    """
    Executa coleta, limpeza, deduplicação e validação.
    Retorna o manifesto (também salvo em output_dir/manifest.json).
    """
    ext = extensions or SUPPORTED_EXTENSIONS
    discovered = discover_files(sources, ext)
    if not discovered:
        return {
            "num_documents": 0,
            "total_size_bytes": 0,
            "categories": {},
            "formats": {},
            "sources": [str(p) for p in sources],
            "stats": {"discovered": 0, "loaded": 0, "duplicates_skipped": 0, "invalid_skipped": 0},
        }

    loaded = 0
    invalid = 0
    duplicates = 0
    seen_hashes: set[str] = set()
    prepared: List[PreparedDocument] = []

    for path, category in discovered:
        raw = load_and_clean(path, category)
        if raw is None:
            invalid += 1
            continue
        loaded += 1
        if not has_minimum_text(raw.content, min_chars):
            invalid += 1
            continue
        h = content_hash(raw.content)
        if h in seen_hashes:
            duplicates += 1
            continue
        seen_hashes.add(h)
        doc = PreparedDocument(
            id=h,
            content=raw.content,
            source_path=raw.source_path,
            category=raw.category,
            format=raw.format,
            size_bytes=len(raw.content.encode(DEFAULT_ENCODING)),
        )
        prepared.append(doc)

    stats = {
        "discovered": len(discovered),
        "loaded": loaded,
        "duplicates_skipped": duplicates,
        "invalid_skipped": invalid,
        "min_doc_chars": min_chars,
    }

    if not dry_run and prepared:
        write_corpus(output_dir, prepared)

    manifest = build_manifest(
        prepared,
        output_dir,
        sources_used=[str(p) for p in sources],
        stats=stats,
    )

    if not dry_run:
        manifest_path = output_dir / MANIFEST_FILENAME
        output_dir.mkdir(parents=True, exist_ok=True)
        manifest_path.write_text(
            json.dumps(manifest, indent=2, ensure_ascii=False),
            encoding=DEFAULT_ENCODING,
        )

    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Prepara corpus real para PoC: coleta, limpeza, deduplicação e validação.",
    )
    parser.add_argument(
        "--sources",
        "-s",
        nargs="+",
        help="Diretórios ou arquivos fonte (ex.: docs/ README.md).",
    )
    parser.add_argument(
        "--config",
        "-c",
        type=Path,
        help="Arquivo de configuração TOML/JSON com chave 'sources' (lista de paths).",
    )
    parser.add_argument(
        "--output",
        "-o",
        type=Path,
        default=Path("data/corpus_poc"),
        help="Diretório de saída do corpus e do manifest.json (default: data/corpus_poc).",
    )
    parser.add_argument(
        "--extensions",
        nargs="+",
        default=[".md", ".html", ".htm"],
        help="Extensões aceitas (default: .md .html .htm).",
    )
    parser.add_argument(
        "--min-chars",
        type=int,
        default=MIN_DOC_CHARS,
        help=f"Tamanho mínimo do documento em caracteres (default: {MIN_DOC_CHARS}).",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Só mostra estatísticas, não grava arquivos.",
    )
    args = parser.parse_args()

    sources: List[Path] = []
    if args.config and args.config.exists():
        sources, _ = load_config(args.config)
    if args.sources:
        sources.extend(Path(p).resolve() for p in args.sources)
    if not sources:
        # Default: docs do projeto + READMEs
        repo_root = Path(__file__).resolve().parent.parent
        default_sources = [
            repo_root / "docs",
            repo_root / "README.md",
            repo_root / "CHANGELOG.md",
            repo_root / "examples",
            repo_root / "tests" / "e2e" / "fixtures" / "corpus_10",
        ]
        for d in default_sources:
            if d.exists():
                sources.append(d)
        if not sources:
            print("Nenhuma fonte definida. Use --sources ou --config.", file=sys.stderr)
            sys.exit(1)
        print("Usando fontes padrão do projeto:", [str(p) for p in sources])

    ext_set = set(e if e.startswith(".") else f".{e}" for e in args.extensions)

    manifest = run(
        sources=sources,
        output_dir=args.output.resolve(),
        extensions=ext_set,
        min_chars=args.min_chars,
        dry_run=args.dry_run,
    )

    print("Manifesto:")
    print(f"  Documentos: {manifest['num_documents']}")
    print(f"  Tamanho total: {manifest['total_size_bytes']} bytes")
    print(f"  Categorias: {manifest['categories']}")
    print(f"  Formatos: {manifest['formats']}")
    print(f"  Stats: {manifest.get('stats', {})}")
    if not args.dry_run and manifest["num_documents"] > 0:
        print(f"  Saída: {args.output.resolve()}")
        print(f"  Manifest: {args.output.resolve() / MANIFEST_FILENAME}")


if __name__ == "__main__":
    main()
