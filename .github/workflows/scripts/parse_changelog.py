#!/usr/bin/env python3
"""
parse_changelog.py
Lê o CHANGELOG.md do FerresDB e detecta se há uma release nova (seção
`## [Released] - DD/MM/YYYY`) que ainda não tem uma GitHub Release criada.

Outputs (via $GITHUB_OUTPUT):
  has_new_release  → 'true' | 'false'
  version_tag      → ex: 'v2026.03.31'  (gerado a partir da data)
  release_title    → ex: 'FerresDB — 31/03/2026'
  release_body     → corpo markdown da release
  release_date     → ex: '31/03/2026'
"""

import os
import re
import subprocess
import sys
from datetime import datetime

CHANGELOG_PATH = "CHANGELOG.md"
DRY_RUN = os.environ.get("DRY_RUN", "false").lower() == "true"


def set_output(name: str, value: str):
    """Escreve um output para o GitHub Actions."""
    github_output = os.environ.get("GITHUB_OUTPUT", "")
    if github_output:
        # Suporta valores multiline com delimitador
        delimiter = "EOF_FERRESDB"
        with open(github_output, "a") as f:
            f.write(f"{name}<<{delimiter}\n{value}\n{delimiter}\n")
    else:
        # Fallback local
        print(f"OUTPUT {name}={value[:120]}...")


def get_existing_tags() -> set[str]:
    """Retorna as tags git já existentes no repositório."""
    try:
        result = subprocess.run(
            ["git", "tag", "--list", "v*"],
            capture_output=True, text=True, check=True
        )
        return set(result.stdout.strip().splitlines())
    except subprocess.CalledProcessError:
        return set()


def parse_changelog(content: str) -> list[dict]:
    """
    Parseia o CHANGELOG.md e retorna lista de releases no formato:
    [{ 'date': '09/02/2026', 'body': '...markdown...' }, ...]

    Suporta os formatos do FerresDB:
      ## [Released] - DD/MM/YYYY
      ## DD/MM/YYYY - DD/MM/YYYY   (formato semana)
      ## YYYY-Wxx (semana de ...)
    """
    releases = []

    # Regex para capturar seções de release (não Unreleased)
    # Captura: ## [Released] - 09/02/2026
    released_pattern = re.compile(
        r"^## \[Released\]\s*-\s*(\d{2}/\d{2}/\d{4})\s*$",
        re.MULTILINE
    )

    lines = content.splitlines()
    section_starts = []  # (line_index, date_str)

    for i, line in enumerate(lines):
        m = released_pattern.match(line)
        if m:
            section_starts.append((i, m.group(1)))

    for idx, (start_line, date_str) in enumerate(section_starts):
        # O corpo vai até a próxima seção ## ou fim do arquivo
        if idx + 1 < len(section_starts):
            end_line = section_starts[idx + 1][0]
        else:
            end_line = len(lines)

        body_lines = lines[start_line + 1:end_line]

        # Remove linhas vazias do início e fim
        while body_lines and not body_lines[0].strip():
            body_lines.pop(0)
        while body_lines and not body_lines[-1].strip():
            body_lines.pop()

        # Para quando há separador --- antes da próxima seção
        clean_body = []
        for line in body_lines:
            if line.strip() == "---":
                break
            clean_body.append(line)

        releases.append({
            "date": date_str,
            "body": "\n".join(clean_body).strip(),
        })

    return releases


def date_to_tag(date_str: str) -> str:
    """Converte '09/02/2026' → 'v2026.02.09'"""
    d = datetime.strptime(date_str, "%d/%m/%Y")
    return f"v{d.strftime('%Y.%m.%d')}"


def main():
    if not os.path.exists(CHANGELOG_PATH):
        print(f"❌ {CHANGELOG_PATH} não encontrado.")
        sys.exit(1)

    with open(CHANGELOG_PATH, "r", encoding="utf-8") as f:
        content = f.read()

    releases = parse_changelog(content)

    if not releases:
        print("ℹ️  Nenhuma seção [Released] encontrada no CHANGELOG.")
        set_output("has_new_release", "false")
        return

    # A release mais recente é a primeira da lista
    latest = releases[0]
    tag = date_to_tag(latest["date"])

    existing_tags = get_existing_tags()
    print(f"Tags existentes: {existing_tags or '(nenhuma)'}")
    print(f"Tag da release mais recente: {tag}")

    if tag in existing_tags:
        print(f"ℹ️  Release {tag} já existe. Nada a publicar.")
        set_output("has_new_release", "false")
        return

    if DRY_RUN:
        print(f"🔍 DRY RUN — release que seria criada: {tag}")
        print(f"Título: FerresDB — {latest['date']}")
        print("Body (primeiros 500 chars):")
        print(latest["body"][:500])
        set_output("has_new_release", "false")
        return

    # Trunca o body se muito longo (GitHub Release tem limite ~125 000 chars)
    body = latest["body"]
    if len(body) > 100_000:
        body = body[:100_000] + "\n\n_[changelog truncado — veja CHANGELOG.md para o texto completo]_"

    print(f"✅ Nova release detectada: {tag}")
    set_output("has_new_release", "true")
    set_output("version_tag", tag)
    set_output("release_title", f"FerresDB — {latest['date']}")
    set_output("release_body", body)
    set_output("release_date", latest["date"])


if __name__ == "__main__":
    main()
