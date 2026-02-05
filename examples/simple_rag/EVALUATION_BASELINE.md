# Baseline de avaliação RAG

Este documento registra o **baseline** do pipeline RAG (Retrieval-Augmented Generation) antes de usuários testarem, e define o **target mínimo** de qualidade.

## Dataset de avaliação

- **Arquivo:** `eval_questions.jsonl`
- **Formato por linha:** `question`, `expected_docs` (ids dos docs esperados no top-5), `keywords` (palavras que devem aparecer na resposta)
- **Tamanho:** 29 perguntas representativas sobre a documentação (FerresDB, ingestão, RAG, API, arquitetura, ADRs)

## Métricas

| Métrica            | Descrição                                                                                       |
| ------------------ | ----------------------------------------------------------------------------------------------- |
| **Recall@5**       | Proporção dos documentos esperados que aparecem no top-5 recuperado (média sobre as perguntas). |
| **MRR@5**          | Mean Reciprocal Rank: 1/rank do primeiro documento esperado encontrado (média).                 |
| **Latência**       | Tempo médio por pergunta (retrieval + LLM quando aplicável), em ms.                             |
| **Keyword recall** | Proporção das keywords que aparecem na resposta gerada (quando LLM está ligado).                |

## Target mínimo

- **Recall@5 ≥ 60%** — pelo menos 60% dos documentos esperados devem estar no top-5 para o conjunto de perguntas.
- **MRR@5** — usado como indicador complementar; quanto maior, melhor.
- **Latência** — registrar para acompanhar regressões; sem target rígido no baseline.

## Como rodar a avaliação

Com o FerresDB em execução e a coleção ingerida (ex.: `docs`):

```bash
cd examples/simple_rag

# Avaliação completa (retrieval + LLM) — requer OPENAI_API_KEY ou ANTHROPIC_API_KEY
python evaluate.py --collection docs --server http://localhost:8080 --report eval_report.json

# Apenas retrieval (sem LLM) — mais rápido, sem custo de API
python evaluate.py --collection docs --server http://localhost:8080 --no-llm --report eval_report_retrieval.json
```

Porta do servidor: use `--server http://localhost:8080` se o FerresDB estiver na porta 8080, ou `--server http://localhost:3000` conforme seu ambiente.

## Resultados do baseline

_(Preencher após rodar `evaluate.py` pela primeira vez.)_

**Data do baseline:** _YYYY-MM-DD_

**Configuração:** coleção `docs`, embedding OpenAI, chunker semantic, top_k=5.

| Métrica          | Valor    | Target |
| ---------------- | -------- | ------ |
| Recall@5 (média) | _XX.X%_  | ≥ 60%  |
| MRR@5 (média)    | _X.XXXX_ | —      |
| Latência (média) | _XXX_ ms | —      |
| Keyword recall   | _XX.X%_  | —      |
| Erros            | _N_      | 0      |

**Observações:** _Ex.: recall acima do target; MRR bom para perguntas factuais; latência aceitável._

---

Para atualizar o baseline após mudanças (modelo, chunker, corpus), rode novamente `evaluate.py` e atualize esta seção.
