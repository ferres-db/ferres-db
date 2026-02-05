# Exemplo RAG (Retrieval-Augmented Generation) com FerresDB

Pipeline RAG que combina **ingestão** de documentos (.md, .pdf, .html) no FerresDB com **query** em texto: embedding da pergunta, busca de chunks relevantes, prompt com contexto e LLM (OpenAI ou Anthropic) para resposta e fontes.

## Tutorial passo a passo

### Passo 1 — Pré-requisitos

- Python 3 (recomendado 3.10+)
- FerresDB em execução (servidor HTTP)
- Chaves de API: OpenAI e/ou Anthropic (e opcionalmente Cohere para embedding/reranker)

### Passo 2 — Instalar dependências

```bash
cd examples/simple_rag
pip install -r requirements.txt
```

Para usar o subcomando **ingest** (`python app.py ingest ...`), instale também as dependências do pipeline de ingestão:

```bash
pip install -r ../ingestion/requirements.txt
```

### Passo 3 — Configuração

Copie o arquivo de exemplo e defina as chaves no `.env`:

```bash
cp .env.example .env
# Edite .env: OPENAI_API_KEY, ANTHROPIC_API_KEY (e COHERE_API_KEY se usar Cohere)
```

- **OpenAI**: `--embedding openai` e `--llm openai`
- **Anthropic**: `--llm anthropic`
- **Cohere**: `--embedding cohere` na ingestão ou `--reranker cohere` no query

### Passo 4 — Subir o FerresDB

O app comunica com o FerresDB via HTTP. Inicie o servidor antes de ingestão ou query:

```bash
# Na raiz do projeto
cargo run --bin server
# ou: make run / docker-compose up -d
```

Por padrão o servidor usa a porta 8080. Se usar outra, informe com `--server` (ex.: `--server http://localhost:3000`).

### Passo 5 — Ingestão de documentos

Crie uma pasta com documentos (ex.: `./docs`) e rode a ingestão. Use o **mesmo** `--embedding` e `--server` que for usar no modo query.

**Opção A — Script de ingestão (recomendado na primeira vez):**

```bash
cd examples/ingestion
pip install -r requirements.txt
python ingest.py --source ./docs --collection docs --embedding openai --chunker semantic --chunk-size 512 --server http://localhost:8080 --cache
```

**Opção B — Subcomando do app RAG:**

```bash
cd examples/simple_rag
python app.py ingest --source ./docs --collection docs --embedding openai --chunker semantic --chunk-size 512 --server http://localhost:8080 --cache
```

O pipeline descobre recursivamente `.md`, `.pdf` e `.html`, gera embeddings (cache opcional) e insere os pontos na coleção.

### Passo 6 — Rodar o app (modo query)

Modo interativo:

```bash
python app.py --collection docs --llm openai
```

Exemplo de sessão:

```
RAG (FerresDB + LLM). Coleção: docs | LLM: openai | Embedding: openai
Digite sua pergunta (enter). Sair: quit, exit ou Ctrl+D.

RAG> Como fazer deploy?
[buscando contexto...]
[gerando resposta...]
Resposta: Para fazer deploy, siga os passos...

Fontes:
  - docs/deploy.md (score: 0.89)
  - docs/quickstart.md (score: 0.76)
```

*(Opcional: adicionar screenshots ou GIF da sessão aqui no futuro.)*

### Passo 7 — (Opcional) Re-rank e benchmark

- **Re-rank**: use `--rerank` para busca híbrida (top-20) + reranker (top-5). A coleção precisa ter sido criada com BM25 (`enable_bm25: true`). Ex.: `python app.py --collection docs --llm openai --rerank --reranker cross_encoder`
- **Benchmark**: use `benchmark_rerank.py` com um JSONL de perguntas e `relevant_ids`. Copie `benchmark_questions.jsonl.example` para `benchmark_questions.jsonl`, preencha e execute `python benchmark_rerank.py --collection docs --dataset benchmark_questions.jsonl`.

---

## Opções úteis

| Opção                | Descrição                                                                                |
| -------------------- | ---------------------------------------------------------------------------------------- |
| `--collection`, `-c` | Nome da coleção (obrigatório no modo query).                                             |
| `--llm`              | `openai` ou `anthropic`.                                                                 |
| `--llm-model`        | Modelo (ex.: `gpt-4o`, `claude-3-5-sonnet-20241022`).                                    |
| `--embedding`, `-e`  | Provedor de embedding (`openai`, `cohere`, `local`). Deve ser o mesmo usado na ingestão. |
| `--server`           | URL base do FerresDB (padrão: `http://localhost:3000`).                                  |
| `--top-k`            | Número de chunks a recuperar sem rerank (padrão: 5).                                     |
| `--rerank`           | Ativa busca híbrida (top-20) + rerank (top-5). Requer coleção com BM25 habilitado.        |
| `--reranker`         | Backend: `cross_encoder`, `cohere`, `llm` (default: cross_encoder).                      |
| `--rerank-top-k`     | Documentos após rerank (default: 5).                                                     |
| `--retrieve-top-k`   | Documentos na etapa de retrieval com `--rerank` (default: 20).                           |
| `--hybrid-alpha`     | Peso vetorial na busca híbrida 0..1 (default: 0.5).                                       |
| `--show-chunks`      | Mostrar os chunks recuperados (trecho de texto).                                         |
| `--no-stream`        | Desativar streaming da resposta do LLM.                                                 |
| `--history`           | Arquivo para salvar histórico de conversas (JSONL).                                      |

Exemplos rápidos:

```bash
python app.py --collection docs --llm anthropic --show-chunks
python app.py --collection docs --llm openai --history rag_history.jsonl
python app.py --collection docs --llm openai --rerank --reranker cross_encoder
```

---

## Re-ranking

Com `--rerank`, o pipeline usa **busca híbrida** (vetorial + BM25) para 20 candidatos e um **reranker** para os 5 melhores antes do contexto do LLM. A coleção deve ter sido criada com **BM25 habilitado** (`enable_bm25: true`).

- `--reranker cross_encoder`: modelo local (sentence-transformers), sem API.
- `--reranker cohere`: API Cohere (requer `COHERE_API_KEY`).
- `--reranker llm`: usa o LLM para pontuar relevância (mais lento e custoso).

---

## Features

- **Chunks recuperados**: `--show-chunks` para ver o trecho de cada chunk no contexto.
- **Streaming**: resposta do LLM em tempo real (`--no-stream` para desativar).
- **Histórico**: `--history <arquivo>` grava pergunta/resposta e fontes em JSONL.
- **Re-ranking**: `--rerank` com busca híbrida + reranker (requer BM25 na coleção).

---

## Troubleshooting

| Problema | Solução |
| -------- | ------- |
| **"collection not found" ou 404 na busca** | Confirme que o FerresDB está rodando e que `--server` está correto. Rode a ingestão antes e use o mesmo `--collection` na query. |
| **Resposta genérica ou sem uso do contexto** | Use o **mesmo** `--embedding` na ingestão e no app. Reingira com o pipeline atual (ingestão grava `text` no metadata dos pontos). |
| **"OPENAI_API_KEY" / "ANTHROPIC_API_KEY" não definida** | Crie `.env` a partir de `.env.example` e defina as chaves, ou exporte no shell: `export OPENAI_API_KEY=sk-...`. |
| **Erro ao importar `embeddings` ou `document_processor`** | Ao rodar `python app.py ingest` a partir de `examples/simple_rag`, o working directory da ingestão é `examples/ingestion`. Instale dependências: `pip install -r ../ingestion/requirements.txt` e garanta que `embeddings.py` e `document_processor.py` estão em `examples/ingestion`. |
| **Porta do FerresDB** | O app usa por padrão `http://localhost:3000`. Se o servidor estiver em outra porta (ex.: 8080), use `--server http://localhost:8080`. |
| **Erro "BM25" / "hybrid search" com `--rerank`** | A busca híbrida exige coleção criada com `enable_bm25: true`. Crie a coleção via API com esse parâmetro ou use um pipeline de ingestão que já configure BM25. |

---

## Próximos passos (features futuras)

- Suporte a mais formatos de documento (e.g. DOCX, TXT).
- Cache de respostas do LLM por hash do contexto para perguntas repetidas.
- Histórico de conversação no prompt (multi-turn).
- Outros backends de embedding e LLM (local, Ollama, etc.).
- Interface web para perguntas e visualização de fontes.
- Avaliação automática de qualidade (faithfulness, relevance) em lote.
