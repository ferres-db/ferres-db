# Arquitetura do FerresDB Core

Este documento descreve a arquitetura interna do FerresDB Core, incluindo diagramas de componentes, fluxos de dados e decisões de design.

## Visão Geral

O FerresDB Core é organizado em camadas bem definidas:

```
┌─────────────────────────────────────────────────────────┐
│                    API Pública (VectorDB)               │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐             │
│  │ Create   │  │ Upsert   │  │ Search   │             │
│  │ Delete   │  │ Stats    │  │          │             │
│  └──────────┘  └──────────┘  └──────────┘             │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│                    Camada de Coleções                    │
│  ┌──────────────────────────────────────────────────┐  │
│  │  Collection                                      │  │
│  │  ├── HashMap<String, Point>  (acesso O(1) por ID)│  │
│  │  ├── Box<dyn ANNIndex>      (índice de busca)   │  │
│  │  └── Option<LRU Cache>      (cache opcional)    │  │
│  └──────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
                         │
         ┌───────────────┼───────────────┐
         ▼               ▼               ▼
┌──────────────┐ ┌──────────────┐ ┌──────────────┐
│   Point      │ │  ANNIndex    │ │   Storage    │
│              │ │  (HNSW)      │ │  (JSONL)     │
│  - id        │ │              │ │              │
│  - vector    │ │  - Cosine    │ │  - points.jsonl│
│  - metadata  │ │  - Euclidean │ │  - config.json│
│  - timestamp │ │  - DotProd   │ │  - index.bin  │
└──────────────┘ └──────────────┘ └──────────────┘
```

## Componentes Principais

### 1. VectorDB (API Principal)

**Responsabilidade**: Interface de alto nível para gerenciar múltiplas coleções.

**Características**:
- Gerencia um `HashMap<String, Collection>` em memória
- Auto-save após cada operação de escrita
- Carrega coleções existentes do disco no startup

**Localização**: `crates/core/src/lib.rs`

### 2. Collection

**Responsabilidade**: Container lógico de pontos vetoriais com índice de busca.

**Estrutura Interna**:
```rust
pub struct Collection {
    config: CollectionConfig,           // Configuração imutável
    points: HashMap<String, Point>,     // Armazenamento O(1) por ID
    index: Box<dyn ANNIndex>,           // Índice de busca (trait object)
    search_cache: Option<Mutex<LruCache>>, // Cache opcional
}
```

**Decisões de Design**:
- `Box<dyn ANNIndex>` permite trocar o backend sem alterar `Collection`
- `HashMap` para acesso rápido por ID (O(1))
- Cache LRU opcional para queries repetidas

**Localização**: `crates/core/src/collection.rs`

### 3. Point

**Responsabilidade**: Unidade fundamental de dado vetorial.

**Estrutura**:
```rust
pub struct Point {
    pub id: String,                      // ID único (livre formato)
    pub vector: Vec<f32>,                // Vetor n-dimensional
    pub metadata: serde_json::Value,     // Metadados JSON arbitrários
    pub created_at: u64,                 // Timestamp Unix
}
```

**Validações**:
- ID não pode ser vazio
- Vetor não pode ser vazio
- Componentes do vetor devem ser finitos (não NaN, não infinito)

**Localização**: `crates/core/src/point.rs`

### 4. ANNIndex (Trait)

**Responsabilidade**: Abstração para qualquer backend de busca aproximada.

**Métodos**:
- `build(&mut self, points: &[Point])` - Reconstrói índice completo
- `search(&self, query: &[f32], k: usize) -> Vec<(String, f32)>` - Busca k-NN
- `add_point(&mut self, point: &Point)` - Adiciona ponto incrementalmente
- `remove_point(&mut self, id: &str)` - Remove ponto (tombstone)

**Por que um Trait?**
- Permite múltiplas implementações (HNSW, brute-force, IVF, etc.)
- Facilita testes (mock implementations)
- Desacopla `Collection` do backend específico

**Localização**: `crates/core/src/search.rs`

### 5. HnswIndex

**Responsabilidade**: Implementação concreta usando algoritmo HNSW.

**Estrutura Interna**:
```rust
pub struct HnswIndex {
    inner: IndexVariant<'static>,        // HNSW com tipo de distância específico
    id_map: Vec<String>,                // DataId → String ID
    reverse_map: HashMap<String, usize>, // String ID → DataId
    tombstones: HashSet<String>,         // IDs removidos (lógica)
    distance: DistanceMetric,
    config: HnswConfig,
}
```

**Métricas Suportadas**:
- **Cosine**: Normaliza vetores para L2=1, usa distância L2
- **Euclidean**: Distância L2 nativa
- **DotProduct**: Produto escalar negado

**Tombstones**: HNSW não suporta remoção nativa. Pontos removidos são marcados como "tombstones" e filtrados nas buscas até o próximo `build()`.

**Localização**: `crates/core/src/search.rs`

### 6. Storage

**Responsabilidade**: Persistência em disco.

**Formato de Arquivos**:
```
<collection_name>/
├── config.json      # CollectionConfig serializado
├── points.jsonl     # Um Point por linha (JSON)
├── index.bin        # Snapshot binário de metadados (bincode)
└── checksum.md5     # Hash MD5 de points.jsonl (validação)
```

**Por que JSON-lines?**
- ✅ Append incremental (útil para WAL futuro)
- ✅ Streaming de leitura (não precisa carregar tudo na memória)
- ✅ Recuperação parcial (linhas completas são válidas mesmo após crash)
- ✅ Fácil de debugar (texto legível)
- ✅ Compatível com ferramentas Unix (`grep`, `wc -l`, etc.)

**Atomicidade**: Todas as escritas usam padrão temp-file + rename para garantir atomicidade no nível do filesystem.

**Localização**: `crates/core/src/storage.rs`

## Fluxos de Dados

### 1. Insert/Upsert

```
┌─────────┐
│ VectorDB│
│ upsert  │
└────┬────┘
     │
     ▼
┌─────────────────┐
│ Collection      │
│ - Valida dim    │
│ - Normaliza*    │
│ - Insere ponto  │
└────┬────────────┘
     │
     ├─────────────────┐
     ▼                 ▼
┌──────────┐    ┌──────────┐
│ HashMap  │    │ HNSW     │
│ (points) │    │ (index)  │
└──────────┘    └──────────┘
     │
     ▼
┌──────────┐
│ Storage  │
│ (save)   │
└──────────┘

* Normalização apenas para métrica Cosine
```

**Otimizações**:
- Para batches > 100 pontos: validação paralela com Rayon
- Para Cosine + batches grandes: normalização paralela antes da inserção
- HNSW não é thread-safe, então inserção é sempre sequencial

### 2. Search

```
┌─────────┐
│ VectorDB│
│ search  │
└────┬────┘
     │
     ▼
┌─────────────────┐
│ Collection      │
│ - Valida query  │
│ - Verifica cache│
└────┬────────────┘
     │
     ├──────────────┐
     │ Cache Hit?   │
     │              │
     ▼ No           ▼ Yes
┌──────────┐    ┌──────────┐
│ HNSW     │    │ Retorna  │
│ search   │    │ cached   │
└────┬─────┘    └──────────┘
     │
     ▼
┌──────────┐
│ Filtra   │
│ tombstones│
└────┬─────┘
     │
     ▼
┌──────────┐
│ Busca    │
│ metadados│
│ em HashMap│
└────┬─────┘
     │
     ▼
┌──────────┐
│ Retorna  │
│ SearchResult│
└──────────┘
```

**Otimizações**:
- Cache LRU opcional (configurável por coleção)
- Normalização do query apenas para Cosine
- Filtragem de tombstones após busca (não durante)

### 3. Delete

```
┌─────────┐
│ VectorDB│
│ delete  │
└────┬────┘
     │
     ▼
┌─────────────────┐
│ Collection      │
│ - Remove de HashMap│
│ - Marca tombstone│
└────┬────────────┘
     │
     ├─────────────────┐
     ▼                 ▼
┌──────────┐    ┌──────────┐
│ HashMap  │    │ HNSW     │
│ (remove) │    │ (tombstone)│
└──────────┘    └──────────┘
     │
     ▼
┌──────────┐
│ Storage  │
│ (save)   │
└──────────┘
```

**Nota**: O ponto permanece no grafo HNSW até o próximo `build()`, mas é filtrado nas buscas.

### 4. Load from Disk

```
┌──────────┐
│ VectorDB │
│ new()    │
└────┬─────┘
     │
     ▼
┌──────────┐
│ Storage  │
│ load     │
└────┬─────┘
     │
     ├─────────────────┐
     ▼                 ▼
┌──────────┐    ┌──────────┐
│ config.json│  │ points.jsonl│
└────┬─────┘    └────┬─────┘
     │               │
     ▼               ▼
┌──────────┐    ┌──────────┐
│ Parse    │    │ Parse    │
│ Config   │    │ Points   │
└────┬─────┘    └────┬─────┘
     │               │
     └───────┬───────┘
             ▼
     ┌──────────────┐
     │ Collection   │
     │ from_points  │
     └──────┬───────┘
            │
            ▼
     ┌──────────────┐
     │ HNSW.build() │
     │ (reconstrói) │
     └──────────────┘
```

**Validações**:
- Checksum MD5 de `points.jsonl` (se presente)
- Validação de `index.bin` (opcional, apenas para debug)
- Rebuild do índice HNSW sempre (garante consistência)

## Decisões de Design

### Por que HNSW?

**Hierarchical Navigable Small World** é um dos algoritmos ANN mais eficientes:

- ✅ **Performance**: O(log n) busca aproximada
- ✅ **Qualidade**: Recall > 95% com configuração adequada
- ✅ **Incremental**: Suporta inserção incremental (não precisa rebuild completo)
- ✅ **Maturidade**: Algoritmo bem estudado e amplamente usado

**Alternativas consideradas**:
- **Brute-force**: Muito lento para grandes coleções (O(n))
- **LSH**: Menor qualidade de recall
- **IVF**: Requer rebuild periódico, menos eficiente para inserção incremental

### Por que JSON-lines?

**Vantagens**:
- ✅ Append incremental (útil para WAL futuro)
- ✅ Streaming de leitura
- ✅ Recuperação parcial após crash
- ✅ Fácil de debugar e inspecionar
- ✅ Compatível com ferramentas Unix

**Alternativas consideradas**:
- **Bincode completo**: Mais rápido, mas não permite streaming/append
- **SQLite**: Overhead desnecessário para dados simples
- **Protocol Buffers**: Complexidade adicional, menos legível

**Tradeoff**: JSON-lines é mais lento que bincode para leitura completa, mas oferece flexibilidade e facilidade de debug.

### Por que Trait Object (`Box<dyn ANNIndex>`)?

**Vantagens**:
- ✅ Desacopla `Collection` do backend específico
- ✅ Facilita testes (mock implementations)
- ✅ Permite múltiplas implementações (HNSW, brute-force, etc.)

**Tradeoff**: Overhead de vtable (desprezível comparado ao custo da busca HNSW).

### Por que Tombstones em vez de Remoção Real?

**HNSW não suporta remoção nativa** do grafo. Opções:

1. **Tombstones** (escolhido): Marca como removido, filtra nas buscas
   - ✅ Simples de implementar
   - ✅ Não requer rebuild imediato
   - ❌ Pontos permanecem no grafo até rebuild

2. **Rebuild completo**: Remove e reconstrói o índice
   - ✅ Limpa completamente
   - ❌ Muito lento para coleções grandes

3. **Implementação custom**: Adicionar suporte de remoção ao HNSW
   - ✅ Remoção real
   - ❌ Complexidade muito alta, requer pesquisa

**Decisão**: Tombstones com rebuild periódico (ou manual via `build()`) oferece melhor tradeoff.

## Thread Safety

### Estruturas Thread-Safe

- ✅ `VectorDB`: Usa `HashMap` que requer `&mut` para modificações
- ✅ `Collection`: Métodos de leitura (`search`, `get`) são `&self`, escrita requer `&mut`
- ✅ `HnswIndex`: Thread-safe para leitura, mas inserção requer `&mut`

### Limitações

- ❌ **HNSW não é thread-safe**: Inserção paralela não é suportada
- ✅ **Busca é thread-safe**: Múltiplas threads podem buscar simultaneamente
- ✅ **Cache LRU**: Usa `Mutex` para thread-safety

### Recomendações

- Use `Arc<Mutex<VectorDB>>` ou `Arc<RwLock<VectorDB>>` para acesso multi-threaded
- Para alta concorrência, considere sharding de coleções
- Cache LRU ajuda em workloads read-heavy

## Performance

### Otimizações Implementadas

1. **Paralelização com Rayon**:
   - Validação de dimensão paralela (batches > 100)
   - Normalização L2 paralela para Cosine (batches > 100)

2. **Cache LRU**:
   - Cache configurável de resultados de busca
   - Reduz latência para queries repetidas

3. **Normalização otimizada**:
   - Cálculo em `f64` para evitar overflow em alta dimensão
   - Pré-normalização em batch para Cosine

### Limitações Conhecidas

- HNSW não suporta inserção paralela (limitação do `hnsw_rs`)
- Rebuild completo é necessário para limpar tombstones
- JSON-lines é mais lento que bincode para leitura completa

### Próximas Otimizações

- Write-ahead log (WAL) para melhor throughput de escrita
- Compressão de vetores (quantização)
- Batch insert otimizado no HNSW

## Extensibilidade

### Adicionar Nova Métrica de Distância

1. Adicione variante em `DistanceMetric` enum
2. Implemente tipo de distância compatível com `hnsw_rs` (ou outro backend)
3. Adicione caso em `IndexVariant` e `create_variant()`
4. Atualize `prepare_vector()` se necessário

### Adicionar Novo Backend ANN

1. Implemente trait `ANNIndex`
2. Use `Collection::with_index()` para criar coleção com novo backend
3. Opcional: Adicione factory method em `CollectionConfig`

### Adicionar Novo Formato de Storage

1. Implemente trait `Storage` (ou use `FileStorage` como exemplo)
2. Modifique `VectorDB::load_collections_from_disk()` e `save_collection()`
3. Adicione configuração para escolher formato

## Referências

- [HNSW Paper](https://arxiv.org/abs/1603.09320) - Hierarchical Navigable Small World
- [hnsw_rs](https://github.com/guillaume-be/hnsw_rs) - Biblioteca HNSW em Rust
- [Vector Database Survey](https://www.pinecone.io/learn/vector-database/) - Visão geral de vector databases

