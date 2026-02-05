# FerresDB Core

Motor de busca vetorial de alta performance escrito em Rust, projetado para aplicações de busca semântica, RAG (Retrieval-Augmented Generation) e sistemas de recomendação.

## 🚀 Visão Geral

O FerresDB Core é o motor de busca vetorial que fornece:

- **Busca por similaridade vetorial** usando algoritmo HNSW (Hierarchical Navigable Small World)
- **Múltiplas métricas de distância**: Cosine, Euclidean (L2) e Dot Product
- **Persistência em disco** com formato JSON-lines para recuperação e backup
- **API de alto nível** (`VectorDB`) para gerenciar múltiplas coleções
- **Otimizações de performance**: paralelização com Rayon, cache LRU opcional
- **Validação robusta** de dados e tratamento de erros com tipos específicos

### Características Principais

- ✅ **Alta performance**: Busca em milissegundos mesmo com milhões de vetores
- ✅ **Thread-safe**: Pronto para uso em servidores multi-threaded
- ✅ **Persistente**: Dados salvos automaticamente em disco após cada operação
- ✅ **Write-Ahead Log (WAL)**: Garante durabilidade e recuperação após crash
- ✅ **Snapshots periódicos**: A cada 1000 operações, cria snapshot completo e trunca WAL
- ✅ **Crash recovery**: Recuperação automática do estado consistente após falhas
- ✅ **Extensível**: Trait `ANNIndex` permite trocar o backend de busca
- ✅ **Type-safe**: Tipos de erro específicos facilitam tratamento e debugging

## 📦 Quick Start

### Instalação

Adicione ao seu `Cargo.toml`:

```toml
[dependencies]
ferres-db-core = { path = "crates/core" }
```

### Exemplo Básico

```rust
use ferres_db_core::{VectorDB, CollectionConfig, DistanceMetric, Point};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Cria uma instância do VectorDB
    let mut db = VectorDB::new("./data".into())?;

    // Configura uma nova coleção
    let config = CollectionConfig {
        name: "embeddings".into(),
        dimension: 384,  // Dimensão dos vetores (ex: all-MiniLM-L6-v2)
        distance: DistanceMetric::Cosine,
        hnsw: Default::default(),  // Usa configuração padrão do HNSW
        search_cache_size: 100,    // Cache de 100 queries
    };

    // Cria a coleção
    db.create_collection(config)?;

    // Insere pontos
    let points = vec![
        Point::new(
            "doc-1",
            vec![0.1; 384],  // Vetor de exemplo
            serde_json::json!({"text": "Primeiro documento"})
        )?,
        Point::new(
            "doc-2",
            vec![0.2; 384],
            serde_json::json!({"text": "Segundo documento"})
        )?,
    ];
    db.upsert_points("embeddings", points)?;

    // Busca os 5 pontos mais similares
    let query_vector = vec![0.15; 384];
    let results = db.search("embeddings", query_vector, 5)?;

    for result in results {
        println!("ID: {}, Score: {:.4}, Metadata: {}",
                 result.id, result.score, result.metadata);
    }

    Ok(())
}
```

### Exemplo com Embeddings Reais

```rust
use ferres_db_core::{VectorDB, CollectionConfig, DistanceMetric, Point};

// Assumindo que você tem uma função que gera embeddings
fn generate_embedding(text: &str) -> Vec<f32> {
    // Use sua biblioteca de embeddings (sentence-transformers, etc)
    // Retorna um vetor de 384 dimensões
    vec![0.0; 384]  // Placeholder
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = VectorDB::new("./data".into())?;

    let config = CollectionConfig {
        name: "documents".into(),
        dimension: 384,
        distance: DistanceMetric::Cosine,
        hnsw: Default::default(),
        search_cache_size: 0,
    };
    db.create_collection(config)?;

    // Indexa documentos
    let documents = vec![
        "Rust é uma linguagem de programação",
        "Python é popular para machine learning",
        "Vector databases são úteis para RAG",
    ];

    let mut points = Vec::new();
    for (i, doc) in documents.iter().enumerate() {
        let embedding = generate_embedding(doc);
        let point = Point::new(
            format!("doc-{}", i),
            embedding,
            serde_json::json!({"text": doc})
        )?;
        points.push(point);
    }
    db.upsert_points("documents", points)?;

    // Busca semântica
    let query_embedding = generate_embedding("linguagem de programação");
    let results = db.search("documents", query_embedding, 3)?;

    println!("Documentos mais similares:");
    for result in results {
        println!("  - {} (similaridade: {:.4})",
                 result.metadata["text"], result.score);
    }

    Ok(())
}
```

## 📊 Benchmarks

O FerresDB inclui benchmarks completos usando [Criterion.rs](https://github.com/bheisler/criterion.rs).

### Executando Benchmarks

```bash
# Gere os corpus de teste primeiro
python tests/fixtures/generate_corpus.py

# Execute os benchmarks
cd crates/core
cargo bench
```

### Resultados Esperados

**Hardware de referência**: CPU moderna (Intel i7/AMD Ryzen), 16GB RAM

#### Indexação (Throughput)

| Tamanho | Pontos/segundo | Tempo total |
| ------- | -------------- | ----------- |
| 1K      | ~50K-100K      | ~10-20ms    |
| 10K     | ~30K-60K       | ~150-300ms  |
| 100K    | ~20K-40K       | ~2.5-5s     |

#### Busca (Latência)

| Métrica  | P50        | P95         | P99         |
| -------- | ---------- | ----------- | ----------- |
| Latência | ~100-500μs | ~200-1000μs | ~500-2000μs |

_Nota: Resultados variam com hardware, configuração HNSW e dimensão dos vetores._

### Visualizando Resultados

Os benchmarks geram relatórios HTML em `target/criterion/`. Abra `target/criterion/index.html` no navegador para gráficos detalhados.

## 🗺️ Roadmap

### Versão 0.1.0 (Atual) ✅

- [x] Motor de busca HNSW com múltiplas métricas
- [x] Persistência em disco (JSON-lines)
- [x] API de alto nível (`VectorDB`)
- [x] Validação de dados e tratamento de erros
- [x] Paralelização para batches grandes
- [x] Cache LRU opcional

### Versão 0.2.0 (Planejado)

- [x] Write-ahead log (WAL) para melhor durabilidade ✅
- [ ] Compressão de vetores (quantização)
- [ ] Índices secundários para busca por metadados
- [ ] Suporte a transações
- [ ] Backup e restore incrementais

### Versão 0.3.0 (Futuro)

- [ ] Sharding automático de coleções grandes
- [ ] Replicação e alta disponibilidade
- [ ] Suporte a múltiplos backends ANN (IVF, FAISS)
- [ ] API gRPC nativa
- [ ] Dashboard de métricas

## 📚 Documentação

- **[API Docs](https://docs.rs/ferres-db-core)** - Documentação completa da API (gere com `cargo doc --open`)
- **[Architecture Guide](docs/architecture.md)** - Arquitetura interna e decisões de design
- **[ADRs](docs/decisions.md)** - Architecture Decision Records

## 🏗️ Arquitetura

```
ferres-db-core/
├── crates/
│   ├── core/          # Motor vetorial: pontos, coleções, HNSW, storage
│   ├── server/        # Servidor HTTP/gRPC (em desenvolvimento)
│   └── sdk-rust/      # SDK Rust para consumidores do FerresDB
├── docs/
│   ├── architecture.md # Arquitetura e fluxos de dados
│   └── decisions.md   # ADRs (Architecture Decision Records)
├── examples/
└── tests/
```

### Módulos do Core

| Módulo          | Responsabilidade                                   |
| --------------- | -------------------------------------------------- |
| `point.rs`      | Estrutura `Point` — vetor f32 + ID + metadata JSON |
| `collection.rs` | `Collection` — gerencia pontos e índice ANN        |
| `search.rs`     | Trait `ANNIndex` + implementação HNSW              |
| `storage.rs`    | Persistência em disco via JSON-lines               |
| `error.rs`      | Tipos de erro com `thiserror`                      |
| `lib.rs`        | API principal `VectorDB` + re-exports              |

## 🐳 Docker

### Build e Execução com Docker

O projeto inclui um Dockerfile multi-stage otimizado e docker-compose.yml para facilitar o deploy.

#### Usando Makefile (recomendado)

```bash
# Construir imagem Docker
make docker-build

# Executar container
make docker-run

# Ver logs
make docker-logs

# Parar container
make docker-stop

# Limpar imagens e volumes
make docker-clean
```

#### Usando Docker Compose diretamente

```bash
# Build e start
docker-compose up -d

# Ver logs
docker-compose logs -f

# Parar
docker-compose down

# Parar e remover volumes
docker-compose down -v
```

#### Variáveis de Ambiente

O servidor pode ser configurado via variáveis de ambiente:

- `HOST`: Host para bind (padrão: `0.0.0.0`)
- `PORT`: Porta do servidor (padrão: `8080`)
- `STORAGE_PATH`: Caminho para dados persistentes (padrão: `/data`)
- `LOG_LEVEL`: Nível de log (padrão: `info`)

#### Health Check

O container inclui health check automático no endpoint `/health`:

```bash
# Verificar saúde do container
curl http://localhost:8080/health
```

#### Volumes

Os dados são persistidos no volume `ferres-data`. Para backup:

```bash
# Backup do volume
docker run --rm -v ferres-db-core_ferres-data:/data -v $(pwd):/backup \
  debian:bookworm-slim tar czf /backup/ferres-backup.tar.gz /data
```

## 🔧 Build e Desenvolvimento

### Build

```bash
# Build de desenvolvimento
cargo build

# Build otimizado (release)
cargo build --release

# Build usando Makefile
make build

# Executar localmente
make run

# Testes
make test
# ou
cargo test

# Documentação
cargo doc --open
```

### CLI

O FerresDB inclui uma CLI completa. Veja [README.md](README.md#cli-interface-de-linha-de-comando) para detalhes.

## 🤝 Contribuindo

Contribuições são bem-vindas! Por favor:

1. Abra uma issue descrevendo a mudança proposta
2. Faça fork do repositório
3. Crie uma branch para sua feature
4. Adicione testes para novas funcionalidades
5. Execute `cargo test` e `cargo clippy`
6. Abra um Pull Request

## 📄 Licença

MIT

## 🙏 Agradecimentos

- [hnsw_rs](https://github.com/guillaume-be/hnsw_rs) - Biblioteca HNSW em Rust
- [Criterion.rs](https://github.com/bheisler/criterion.rs) - Framework de benchmarks
