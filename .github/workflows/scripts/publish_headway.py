#!/usr/bin/env python3
"""
publish_headway.py
Publica uma entrada de changelog no Headwayapp via API REST.

Variáveis de ambiente obrigatórias:
  HEADWAYAPP_TOKEN  → token da API (Settings → API no painel Headwayapp)
  RELEASE_TITLE     → título da entrada
  RELEASE_BODY      → corpo em markdown
  RELEASE_DATE      → DD/MM/YYYY

Documentação da API: https://headwayapp.co/changelog-api
"""

import os
import sys
import json
import urllib.request
import urllib.error
from datetime import datetime

TOKEN = os.environ["HEADWAYAPP_TOKEN"]
TITLE = os.environ["RELEASE_TITLE"]
BODY = os.environ["RELEASE_BODY"]
DATE_STR = os.environ["RELEASE_DATE"]  # DD/MM/YYYY

# Headwayapp espera a data em ISO 8601
date_obj = datetime.strptime(DATE_STR, "%d/%m/%Y")
published_at = date_obj.strftime("%Y-%m-%dT00:00:00Z")

# Detecta categorias a partir das seções do body (### Added, ### Fixed, etc.)
categories = []
if "### Added" in BODY or "**Feature" in BODY:
    categories.append("new")
if "### Fixed" in BODY:
    categories.append("fix")
if "### Changed" in BODY or "**Optimization" in BODY or "**Performance" in BODY:
    categories.append("improvement")
if "### Security" in BODY or "**Security" in BODY:
    categories.append("security")

payload = {
    "data": {
        "title": TITLE,
        "body": BODY,
        "published_at": published_at,
        "status": "published",
        # Headwayapp aceita labels: new, fix, improvement, security
        "labels": categories or ["new"],
    }
}

url = "https://headwayapp.co/api/changelogs"
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
        body = resp.read().decode("utf-8")
        print(f"✅ Publicado no Headwayapp! Status: {resp.status}")
        print(body[:300])
except urllib.error.HTTPError as e:
    print(f"❌ Erro ao publicar no Headwayapp: {e.code} {e.reason}")
    print(e.read().decode("utf-8"))
    sys.exit(1)
