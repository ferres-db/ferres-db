# Análise do PoC

Script para análise pós-PoC: queries, qualidade (recall/MRR) e feedback dos usuários. Gera relatório Markdown com gráficos.

## Instalação

```bash
pip install -r analysis/requirements.txt
```

## Uso

**Mínimo** (apenas log de queries):

```bash
python analysis/analyze_poc.py --query-log data/logs/queries.log --output-dir analysis/out
```

**Completo** (queries + feedback + sessões + relatórios de avaliação):

```bash
python analysis/analyze_poc.py \
  --query-log data/logs/queries.log \
  --feedback data/logs/feedback.jsonl \
  --sessions-dir examples/simple_rag/data/sessions \
  --eval-report-before examples/simple_rag/eval_report.json \
  --eval-report-after examples/simple_rag/eval_report_new.json \
  --form feedback_form.csv \
  --run-eval-user \
  --output-dir analysis/out
```

**Opções principais**

| Opção | Descrição |
|-------|-----------|
| `--query-log` | Caminho do `queries.log` (JSONL do servidor) |
| `--feedback` | Caminho do `feedback.jsonl` (Útil/Não útil + comentário) |
| `--form` | CSV ou JSON com respostas do formulário (escalas + perguntas abertas) |
| `--sessions-dir` | Diretório com sessões JSONL para extrair perguntas reais |
| `--user-questions` | JSONL com `{"question": "..."}` para top-10 e avaliação |
| `--eval-report-before` / `--eval-report-after` | Relatórios do `evaluate.py` para comparar recall antes/depois |
| `--run-eval-user` | Rodar `evaluate.py` com perguntas reais (sessions ou --user-questions) |
| `--bugs` | Arquivo com um bug por linha (incluído no relatório) |
| `--next-steps` | Arquivo com um próximo passo por linha |
| `--output-dir` | Diretório de saída (`report.md` + `figs/`) |

## O que o script gera

1. **Query analysis**
   - Histograma de latência, distribuição de número de resultados, uso de filtros
   - Top-10 queries mais frequentes (por texto, se houver sessões/user-questions, senão por padrão)
   - Taxa de zero resultados (%)

2. **Quality analysis**
   - Comparação de recall/MRR antes vs depois (se forem passados os dois relatórios)
   - Opcional: roda `evaluate.py` com perguntas reais
   - Correlação latência vs qualidade (scatter + Pearson) quando há `per_question` no relatório

3. **Feedback analysis**
   - Agregação de feedback inline (útil/não útil) e formulário (escalas 1–5)
   - Palavras mais citadas e word cloud (comentários + respostas abertas do formulário)
   - Temas comuns: lista de palavras; opcional `--llm-themes` para extração com LLM

4. **Relatório**
   - `report.md`: sumário executivo, métricas de performance, feedback, bugs, próximos passos
   - `figs/*.png`: gráficos referenciados no Markdown

## Formato do formulário (CSV)

Exemplo de colunas compatíveis com o roteiro em `docs/user_testing.md`:

- `facilidade`, `qualidade`, `velocidade` (1–5)
- `o_que_funcionou_bem`, `o_que_nao_funcionou`, `o_que_esperava` (texto livre)

Ou JSON/JSONL com as mesmas chaves.
