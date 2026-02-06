# Guia de Contribuição

Obrigado por considerar contribuir com o FerresDB Core. Este guia cobre ambiente de desenvolvimento, testes, padrões de código e processo de PR.

## Pré-requisitos

- **Rust**: 1.70+ (`rustup` recomendado)
- **Python**: 3.10+ (para scripts, exemplos e testes E2E)
- **Make** (opcional): para alvos do `Makefile`

## Configuração do ambiente

### 1. Clone e build

```bash
git clone <repo-url>
cd ferres-db-core
cargo build
```

### 2. Executar testes

```bash
# Testes unitários e de integração (workspace)
cargo test --workspace

# Ou via Makefile
make test
```

### 3. Servidor local

```bash
cargo run --bin ferres-db-server
# ou: make run
```

O servidor sobe em `http://localhost:8080`. Documentação da API: [docs/api.md](docs/api.md).

## Estrutura do projeto

| Pasta / Crate     | Descrição                                                |
| ----------------- | -------------------------------------------------------- |
| `crates/core`     | Motor vetorial: VectorDB, Collection, HNSW, Storage, WAL |
| `crates/server`   | Servidor HTTP (Axum), handlers, métricas                 |
| `crates/sdk-rust` | Cliente Rust (HTTP, busca híbrida)                       |
| `docs/`           | Documentação (API, arquitetura, ADRs)                    |
| `examples/`       | Ingestão, simple_rag (Python)                            |
| `tests/`          | Testes E2E e fixtures                                    |

Arquitetura e decisões: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/architecture.md](docs/architecture.md), [docs/ADR/](docs/ADR/).

## Padrões de código

### Rust

- **Formatação**: `cargo fmt` antes de commitar.
- **Lint**: `cargo clippy --workspace` sem warnings.
- **Doc**: Documentar APIs públicas com `///`; exemplos em doc tests quando fizer sentido.

### Convenções

- Tratamento de erros com `thiserror` e tipos específicos (`FerresError`).
- Logs com `tracing` (evitar `println!` em código de produção).
- Testes unitários no mesmo arquivo (`#[cfg(test)] mod tests`) ou em `tests/` para integração.

## Testes

### Unitários e integração (Rust)

```bash
cargo test --workspace
```

Inclui property tests em `crates/server/tests/`. Garanta que todos passem antes do PR.

### E2E (Python)

```bash
cd tests/e2e
pip install -r requirements.txt
pytest
```

Recomendado rodar com o servidor já em execução (ou via script que sobe/derruba o servidor, se houver).

### Benchmarks

```bash
# Gerar corpus de teste (se necessário)
python tests/fixtures/generate_corpus.py

cd crates/core
cargo bench
```

## Fluxo de contribuição

1. **Issue** (recomendado): Abra uma issue descrevendo a mudança ou correção.
2. **Branch**: Crie uma branch a partir de `main` (ex.: `feature/nome` ou `fix/descricao`).
3. **Alterações**: Implemente com testes e documentação quando aplicável.
4. **Checks locais**:
   - `cargo fmt`
   - `cargo clippy --workspace`
   - `cargo test --workspace`
5. **Commit**: Mensagens claras; prefira imperativo (“Add X” em vez de “Added X”).
6. **Pull Request**: Descreva o que foi feito e referencie a issue, se houver. Um maintainer fará o review.

## Documentação

- **API REST**: [docs/api.md](docs/api.md)
- **Arquitetura**: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/architecture.md](docs/architecture.md)
- **Decisões (ADRs)**: [docs/ADR/](docs/ADR/)
- **SDK e clientes**: [docs/sdk.md](docs/sdk.md)

Ao adicionar comportamento novo ou mudar contratos, atualize a documentação correspondente e, se for decisão de design, considere um novo ADR em `docs/ADR/`.

## Dúvidas

Em caso de dúvidas sobre arquitetura ou onde implementar algo, consulte a documentação em `docs/` ou abra uma issue.

Obrigado por contribuir.
