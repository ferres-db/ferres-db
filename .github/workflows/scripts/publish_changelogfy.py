#!/usr/bin/env python3
"""
publish_changelogfy.py
Publica uma entrada de changelog no Changelogfy via API REST.

Variáveis de ambiente obrigatórias:
  CHANGELOGFY_TOKEN  → token da API (Settings → Integrations no painel)
  RELEASE_TITLE      → título da entrada
  RELEASE_BODY       → corpo em markdown
  RELEASE_DATE       → DD/MM/YYYY

Documentação da API: https://changelogfy.com/docs/api
"""

import os
import sys
import json
import urllib.request
import urllib.error
from datetime import datetime

TOKEN = os.environ["CHANGELOGFY_TOKEN"]
TITLE = os.environ["RELEASE_TITLE"]
BODY = os.environ["RELEASE_BODY"]
DATE_STR = os.environ["RELEASE_DATE"]

date_obj = datetime.strptime(DATE_STR, "%d/%m/%Y")
published_at = date_obj.strftime("%Y-%m-%d")

# Mapeia seções do CHANGELOG para tipos do Changelogfy
entry_type = "new"  # default
if "### Fixed" in BODY and "### Added" not in BODY:
    entry_type = "fix"
elif "### Changed" in BODY and "### Added" not in BODY:
    entry_type = "improvement"

payload = {
    "title": TITLE,
    "content": BODY,
    "date": published_at,
    "type": entry_type,          # new | fix | improvement | security
    "status": "published",
}

url = "https://api.changelogfy.com/v1/posts"
data = json.dumps(payload).encode("utf-8")

req = urllib.request.Request(
    url,
    data=data,
    headers={
        "Content-Type": "application/json",
        "Authorization": f"Bearer {TOKEN}",
    },
    method="POST",
)

try:
    with urllib.request.urlopen(req) as resp:
        body_resp = resp.read().decode("utf-8")
        print(f"✅ Publicado no Changelogfy! Status: {resp.status}")
        print(body_resp[:300])
except urllib.error.HTTPError as e:
    print(f"❌ Erro ao publicar no Changelogfy: {e.code} {e.reason}")
    print(e.read().decode("utf-8"))
    sys.exit(1)
