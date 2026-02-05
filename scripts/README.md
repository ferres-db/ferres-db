# Scripts

Scripts auxiliares para o projeto FerresDB.

## prepare_corpus.py

Prepara um **corpus real para PoC** (proof of concept): coleta fontes, limpa HTML/Markdown, remove duplicatas e valida qualidade. Saída pronta para o pipeline de ingestão (`examples/ingestion/ingest.py`).

### Fontes suportadas

1. **Documentação interna** — READMEs, `docs/`, wikis (Markdown/HTML)
2. **Base de conhecimento** — FAQs, tickets resolvidos (`.md` ou `.html` em pastas como `faq/`, `kb/`)
3. **Artigos técnicos** — blog posts salvos (`.md`/`.html` em `articles/`, `blog/`)

### O que o script faz

1. **Coleta** — Varre diretórios/arquivos indicados (recursivo para pastas)
2. **Limpeza** — Extrai texto de HTML (BeautifulSoup ou fallback), normaliza Markdown (whitespace, blocos de imagem vazios)
3. **Deduplicação** — Hash SHA256 do conteúdo normalizado; documentos idênticos são ignorados
4. **Validação** — Descarta documentos com &lt; 50 caracteres ou sem texto relevante (ex.: só imagens/números)
5. **Manifesto** — Gera `manifest.json` com: número de documentos, tamanho total, categorias e formatos

### Uso

```bash
# A partir da raiz do repositório
python scripts/prepare_corpus.py --sources docs/ README.md examples/ --output data/corpus_poc

# Usando arquivo de configuração (JSON ou TOML)
python scripts/prepare_corpus.py --config scripts/corpus_sources.example.json --output data/corpus_poc

# Sem argumentos: usa fontes padrão (docs/, README, CHANGELOG, examples/, corpus_10)
python scripts/prepare_corpus.py --output data/corpus_poc

# Apenas estatísticas (não grava arquivos)
python scripts/prepare_corpus.py --sources docs/ --dry-run
```

### Opções

| Opção             | Descrição                                                |
| ----------------- | -------------------------------------------------------- |
| `--sources`, `-s` | Diretórios ou arquivos fonte (pode repetir)              |
| `--config`, `-c`  | Arquivo JSON/TOML com chave `sources` (lista de paths)   |
| `--output`, `-o`  | Diretório de saída (default: `data/corpus_poc`)          |
| `--extensions`    | Extensões aceitas (default: `.md` `.html` `.htm`)        |
| `--min-chars`     | Tamanho mínimo por documento em caracteres (default: 50) |
| `--dry-run`       | Só imprime estatísticas, não grava arquivos              |

### Saída

- **Diretório de saída** — Um arquivo `.md` por documento (id = hash do conteúdo)
- **manifest.json** — Contagem, tamanho total, categorias, formatos e estatísticas (descobertos, duplicatas, inválidos)

### Target

Objetivo típico: **500–1000 documentos reais**. Para atingir esse volume, inclua várias fontes no `--sources` ou no arquivo de configuração (por exemplo, múltiplas pastas de documentação, FAQ e artigos).

### Ingestão após a preparação

```bash
cd examples/ingestion
python ingest.py --source ../../data/corpus_poc --collection docs --embedding openai
```

Dependência opcional para limpeza de HTML melhor: `pip install beautifulsoup4`.
