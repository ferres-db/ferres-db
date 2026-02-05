# Exemplo RAG (Retrieval-Augmented Generation) com FerresDB

Este exemplo implementa um pipeline RAG que:

1. **Ingestão**: reutiliza o pipeline de `../ingestion` para indexar documentos (.md, .pdf, .html) no FerresDB.
2. **Query**: recebe uma pergunta em texto, gera embedding, busca os top-k chunks relevantes, monta um prompt com contexto e chama um LLM (OpenAI ou Anthropic) para gerar a resposta, exibindo também as fontes.

## Componentes

- **Ingestão**: chama o script `examples/ingestion/ingest.py` (mesmo pipeline do “Dia 12”).
- **Query pipeline**:
  - Recebe pergunta em texto.
  - Gera embedding da pergunta (mesmo provedor usado na ingestão).
  - Busca chunks no FerresDB (vetorial ou, com `--rerank`, híbrida top-20 + rerank top-5).
  - Monta prompt com contexto.
  - Chama LLM (OpenAI ou Anthropic).
  - Retorna resposta + fontes (arquivo e score).

## Setup

### 1. Instalar dependências

Na raiz do repositório ou no diretório do exemplo:

```bash
cd examples/simple_rag
pip install -r requirements.txt
```

Para rodar a **ingestão** a partir daqui (`python app.py ingest ...`), instale também as dependências do pipeline de ingestão:

```bash
pip install -r ../ingestion/requirements.txt
```

### 2. Variáveis de ambiente

Copie o arquivo de exemplo e preencha as chaves:

```bash
cp .env.example .env
# Edite .env e defina OPENAI_API_KEY e/ou ANTHROPIC_API_KEY
```

- **OpenAI**: necessário para `--embedding openai` e `--llm openai`.
- **Anthropic**: necessário para `--llm anthropic`.
- **Cohere**: para `--embedding cohere` na ingestão ou `--reranker cohere` no modo query.

### 3. FerresDB em execução

O app fala com o FerresDB via HTTP. Inicie o servidor (por exemplo na porta 8080) antes de rodar ingestão ou query:

```bash
# Exemplo: a partir da raiz do projeto
cargo run --bin server
# ou use docker-compose, etc.
```

## Ingestão de documentos de exemplo

Crie uma pasta com documentos (por exemplo `./docs`) e rode a ingestão. Você pode usar o script de ingestão diretamente ou o subcomando `ingest` do app:

**Opção A – Script de ingestão (recomendado para primeira vez):**

```bash
cd examples/ingestion
pip install -r requirements.txt
python ingest.py --source ./docs --collection docs --embedding openai --chunker semantic --chunk-size 512 --server http://localhost:8080 --cache
```

**Opção B – Subcomando do app RAG:**

```bash
cd examples/simple_rag
python app.py ingest --source ./docs --collection docs --embedding openai --chunker semantic --chunk-size 512 --server http://localhost:8080 --cache
```

O pipeline descobre recursivamente arquivos `.md`, `.pdf` e `.html`, gera embeddings (com cache opcional) e insere os pontos na coleção. O **embedding** e o **--server** devem ser os mesmos que você for usar no modo query.

## Como rodar o app (modo query)

Modo interativo (padrão):

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

### Opções úteis

| Opção                | Descrição                                                                                |
| -------------------- | ---------------------------------------------------------------------------------------- |
| `--collection`, `-c` | Nome da coleção (obrigatório no modo query).                                             |
| `--llm`              | `openai` ou `anthropic`.                                                                 |
| `--llm-model`        | Modelo (ex.: `gpt-4o`, `claude-3-5-sonnet-20241022`).                                    |
| `--embedding`, `-e`  | Provedor de embedding (`openai`, `cohere`, `local`). Deve ser o mesmo usado na ingestão. |
| `--server`           | URL base do FerresDB (padrão: `http://localhost:3000`).                                  |
| `--top-k`            | Número de chunks a recuperar sem rerank (padrão: 5).                                      |
| `--rerank`           | Ativa busca híbrida (top-20) + rerank (top-5). **Requer coleção com BM25 habilitado.**   |
| `--reranker`         | Backend do reranker: `cross_encoder`, `cohere`, `llm` (default: cross_encoder).          |
| `--rerank-top-k`     | Documentos após rerank (default: 5).                                                     |
| `--retrieve-top-k`   | Documentos na etapa de retrieval quando `--rerank` (default: 20).                       |
| `--hybrid-alpha`     | Peso vetorial na busca híbrida 0..1 (default: 0.5).                                     |
| `--show-chunks`      | Mostrar os chunks recuperados (trecho de texto).                                         |
| `--no-stream`        | Desativar streaming da resposta do LLM.                                                  |
| `--history`          | Arquivo para salvar histórico de conversas (JSONL).                                      |

Exemplos:

```bash
# Anthropic + mostrar chunks
python app.py --collection docs --llm anthropic --show-chunks

# Salvar histórico
python app.py --collection docs --llm openai --history rag_history.jsonl

# Sem streaming
python app.py --collection docs --llm openai --no-stream

# Com rerank (busca híbrida 20 -> rerank 5)
python app.py --collection docs --llm openai --rerank --reranker cross_encoder
```

### Re-ranking

Com a flag `--rerank`, o pipeline usa **busca híbrida** (vetorial + BM25) para recuperar 20 candidatos e um **reranker** para reduzir ao top-5 antes de montar o contexto do LLM. A coleção precisa ter sido criada com **BM25 habilitado** (`enable_bm25`).

- `--reranker cross_encoder`: modelo local (sentence-transformers), sem API.
- `--reranker cohere`: API Cohere (requer `COHERE_API_KEY`).
- `--reranker llm`: usa o próprio LLM para pontuar relevância (mais lento e custoso).

## Benchmark de qualidade

O script `benchmark_rerank.py` mede **recall@5** antes e depois do rerank e **latência** (retrieval, retrieval+rerank).

**Dataset**: arquivo JSONL com uma linha por pergunta e IDs dos documentos relevantes (ground truth):

```json
{"question": "Como fazer deploy?", "relevant_ids": ["chunk-id-1", "chunk-id-2"]}
```

Copie o exemplo e preencha `relevant_ids` com IDs de pontos da sua coleção (os IDs retornados pela busca ou pela ingestão):

```bash
cp benchmark_questions.jsonl.example benchmark_questions.jsonl
# Edite benchmark_questions.jsonl com perguntas e relevant_ids
python benchmark_rerank.py --collection docs --dataset benchmark_questions.jsonl
python benchmark_rerank.py --collection docs --dataset benchmark_questions.jsonl --reranker cohere --server http://localhost:3000
```

Métricas exibidas: recall@5 (antes/depois do rerank), latência média e p95 da retrieval, latência do rerank e end-to-end retrieval+rerank.

## Features

- **Chunks recuperados**: use `--show-chunks` para ver o trecho de cada chunk usado no contexto.
- **Streaming**: a resposta do LLM é exibida em tempo real (use `--no-stream` para desativar).
- **Histórico**: com `--history <arquivo>`, cada pergunta/resposta e fontes é appendada em JSONL.
- **Re-ranking**: com `--rerank`, usa busca híbrida (top-20) e reranker para os 5 melhores chunks (requer BM25 na coleção).

## Troubleshooting

### "collection not found" ou 404 na busca

- Confirme que o FerresDB está rodando e que a URL em `--server` está correta.
- Rode a ingestão antes e use o mesmo `--collection` na query.

### Resposta genérica ou sem uso do contexto

- Use o **mesmo** `--embedding` na ingestão e no app (ex.: `openai` nos dois).
- Se a ingestão não tiver sido feita com o pipeline que grava `text` no metadata, reingira com o `ingest.py` atual (que já inclui `text` no metadata dos pontos).

### "OPENAI_API_KEY" / "ANTHROPIC_API_KEY" não definida

- Crie um `.env` a partir de `.env.example` e defina as chaves.
- Ou exporte no shell: `export OPENAI_API_KEY=sk-...`.

### Erro ao importar embeddings ou document_processor

- Se rodar `python app.py ingest` a partir de `examples/simple_rag`, o script chama `../ingestion/ingest.py`; o working directory da ingestão é `examples/ingestion`. Certifique-se de que em `examples/ingestion` estão `embeddings.py`, `document_processor.py` e dependências instaladas (`pip install -r ../ingestion/requirements.txt`).

### Porta do FerresDB

- O padrão do app é `http://localhost:3000`. Se o seu servidor usar outra porta ou host, use `--server http://localhost:8080` (ou a URL correta).

### "BM25" / "hybrid search" ao usar --rerank

- A busca híbrida exige que a coleção tenha sido criada com BM25 habilitado. Crie a coleção via API com `enable_bm25: true` (e opcionalmente `bm25_text_field`) ou use um pipeline de ingestão que já configure isso.
