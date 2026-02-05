# Pipeline de Ingestão

Exemplo de pipeline para carregar documentos (Markdown, PDF, HTML) e segmentá-los em chunks para indexação no FerresDB.

## Uso

```python
from document_processor import (
    DocumentLoader,
    FixedSizeChunker,
    MarkdownHeaderChunker,
    SemanticChunker,  # requer sentence-transformers
)

loader = DocumentLoader()

# Carregar documentos
docs_md = loader.load_markdown("arquivo.md")
docs_pdf = loader.load_pdf("arquivo.pdf")   # um Document por página
docs_html = loader.load_html("pagina.html")

# Segmentar com estratégia fixa
chunker = FixedSizeChunker(chunk_size=512, overlap=50)
for doc in docs_md:
    chunks = chunker.chunk(doc)

# Ou por headers Markdown
header_chunker = MarkdownHeaderChunker()
chunks = header_chunker.chunk(docs_md[0])
```

## Estrutura

- **Document**: `id`, `content`, `metadata`
- **Chunk**: `id`, `text`, `metadata` (inclui `source_doc_id`, `chunk_index`)
- **DocumentLoader**: `load_markdown`, `load_pdf` (pypdf), `load_html` (BeautifulSoup)
- **ChunkStrategy**: interface `chunk(document) -> List[Chunk]`
  - **FixedSizeChunker**: tamanho fixo com overlap
  - **MarkdownHeaderChunker**: quebra por `##` headers
  - **SemanticChunker**: quebra por similaridade semântica (sentence-transformers)

## Embeddings

Provedores de embedding (OpenAI, Cohere, local com sentence-transformers) com cache SQLite, rate limiting, retry e barra de progresso:

```python
from embeddings import (
    OpenAIEmbeddings,
    LocalEmbeddings,
    CohereEmbeddings,
    embed_texts,
)

provider = LocalEmbeddings()  # ou OpenAIEmbeddings(api_key="..."), CohereEmbeddings(api_key="...")
vectors = embed_texts(
    provider,
    ["texto 1", "texto 2"],
    use_cache=True,
    requests_per_minute=60,
    retries=3,
    show_progress=True,
)
```

### Estimativa de custo

Antes de gerar embeddings com API paga, calcule o custo e confirme:

```bash
# Textos em arquivo (um por linha ou JSONL com campo "text")
py estimate_embedding_cost.py --provider openai --texts-file chunks.txt

# Textos na linha de comando
py estimate_embedding_cost.py --provider openai --texts "foo" "bar"

# Pular confirmação (só exibir)
py estimate_embedding_cost.py --provider openai --texts-file chunks.txt -y

# Após confirmar, rodar embeddings (--run)
py estimate_embedding_cost.py --provider openai --texts-file chunks.txt --run
```

Variáveis de ambiente: `OPENAI_API_KEY`, `COHERE_API_KEY`.

## Testes

```bash
pip install -r requirements.txt
pytest tests -v
```

O fixture PDF de 10 páginas é gerado automaticamente pelo `conftest.py` se `reportlab` estiver instalado.
