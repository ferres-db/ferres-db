# Architecture Decision Records (ADRs)

Este documento registra as decisões arquiteturais importantes do FerresDB Core, explicando o contexto, alternativas consideradas e consequências.

## ADR-001: Uso de HNSW em vez de implementação própria

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Precisávamos de um algoritmo ANN eficiente para busca vetorial.

**Decisão**: Usar a biblioteca `hnsw_rs` em vez de implementar HNSW do zero.

**Alternativas Consideradas**:

1. **Implementar HNSW do zero**
   - ✅ Controle total sobre implementação
   - ❌ Muito tempo de desenvolvimento
   - ❌ Risco de bugs e performance subótima
   - ❌ Manutenção contínua necessária

2. **Usar outra biblioteca ANN** (ex: `usearch`, `faiss-rs`)
   - ✅ Implementação madura
   - ❌ `usearch`: Menos features, menor comunidade
   - ❌ `faiss-rs`: Bindings para C++, complexidade adicional

3. **Usar `hnsw_rs`** (escolhido)
   - ✅ Biblioteca Rust nativa (sem FFI)
   - ✅ API limpa e bem documentada
   - ✅ Suporte a múltiplas métricas de distância
   - ✅ Ativa manutenção
   - ❌ Menos controle sobre detalhes internos

**Consequências**:
- Desenvolvimento mais rápido (focamos na API e storage)
- Dependência externa (mas bem mantida)
- Performance excelente (benchmarks confirmam)
- Facilita manutenção (bugs corrigidos upstream)

**Notas**: Se precisarmos de features específicas não suportadas pelo `hnsw_rs`, podemos considerar fork ou implementação própria no futuro.

---

## ADR-002: Formato JSON-lines para persistência

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Precisávamos de um formato de persistência que permitisse append incremental e fosse fácil de debugar.

**Decisão**: Usar formato JSON-lines (`.jsonl`) para armazenar pontos.

**Alternativas Consideradas**:

1. **Bincode completo**
   - ✅ Serialização muito rápida
   - ✅ Tamanho compacto
   - ❌ Não permite append incremental
   - ❌ Difícil de debugar (binário)
   - ❌ Recuperação parcial difícil após crash

2. **SQLite**
   - ✅ ACID transactions
   - ✅ Queries SQL
   - ❌ Overhead desnecessário para dados simples
   - ❌ Dependência externa pesada
   - ❌ Complexidade adicional

3. **Protocol Buffers**
   - ✅ Compacto e eficiente
   - ✅ Schema versionado
   - ❌ Menos legível que JSON
   - ❌ Requer schema definitions
   - ❌ Append incremental mais complexo

4. **JSON-lines** (escolhido)
   - ✅ Append incremental simples
   - ✅ Streaming de leitura
   - ✅ Recuperação parcial após crash
   - ✅ Fácil de debugar (texto legível)
   - ✅ Compatível com ferramentas Unix
   - ❌ Mais lento que bincode para leitura completa
   - ❌ Tamanho maior que bincode

**Consequências**:
- Facilita debugging e inspeção manual
- Permite implementação futura de WAL (write-ahead log)
- Tradeoff de performance aceitável para benefícios de flexibilidade
- Formato bem conhecido e suportado

**Notas**: Para coleções muito grandes (>10M pontos), podemos considerar formato binário otimizado no futuro, mas JSON-lines é adequado para maioria dos casos.

---

## ADR-003: Trait Object para abstração de índice ANN

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Queríamos desacoplar `Collection` do backend específico de busca (HNSW).

**Decisão**: Usar `Box<dyn ANNIndex>` (trait object) em vez de tipo genérico ou enum.

**Alternativas Consideradas**:

1. **Tipo genérico** (`Collection<I: ANNIndex>`)
   - ✅ Zero-cost abstraction (monomorphização)
   - ✅ Performance máxima
   - ❌ `Collection` não pode ser armazenada em `HashMap` facilmente
   - ❌ Complexidade de tipos aumenta

2. **Enum com variantes** (`enum IndexType { Hnsw, BruteForce }`)
   - ✅ Despacho estático (rápido)
   - ✅ Sem heap allocation
   - ❌ Menos extensível (requer modificar enum para novo backend)
   - ❌ Match statements em todos os métodos

3. **Trait Object** (`Box<dyn ANNIndex>`) (escolhido)
   - ✅ Extensível (qualquer tipo pode implementar)
   - ✅ Fácil de testar (mock implementations)
   - ✅ Desacopla `Collection` do backend
   - ❌ Overhead de vtable (desprezível comparado ao custo HNSW)
   - ❌ Heap allocation (uma vez, aceitável)

**Consequências**:
- Facilita testes unitários (mock `ANNIndex`)
- Permite múltiplas implementações sem modificar `Collection`
- Overhead de vtable é desprezível (<1% do tempo de busca)
- Código mais limpo e extensível

**Notas**: Se profiling mostrar que vtable overhead é significativo, podemos considerar enum com variantes específicas, mas isso é improvável dado o custo da busca HNSW.

---

## ADR-004: Tombstones em vez de remoção real do HNSW

**Status**: Aceito  
**Data**: 2024  
**Contexto**: HNSW não suporta remoção nativa do grafo. Precisávamos de uma estratégia para remover pontos.

**Decisão**: Usar "tombstones" (marcação lógica) em vez de remoção física ou rebuild completo.

**Alternativas Consideradas**:

1. **Rebuild completo após cada remoção**
   - ✅ Limpa completamente o índice
   - ✅ Sem pontos órfãos
   - ❌ Muito lento para coleções grandes (O(n log n))
   - ❌ Bloqueia todas as operações durante rebuild

2. **Implementação custom de remoção no HNSW**
   - ✅ Remoção real
   - ✅ Performance aceitável
   - ❌ Complexidade muito alta
   - ❌ Requer pesquisa e desenvolvimento significativo
   - ❌ Risco de bugs

3. **Tombstones** (escolhido)
   - ✅ Simples de implementar
   - ✅ Não bloqueia operações
   - ✅ Remoção imediata (filtrada nas buscas)
   - ❌ Pontos permanecem no grafo até rebuild
   - ❌ Uso de memória ligeiramente maior

**Consequências**:
- Remoção é O(1) (apenas marcação)
- Buscas filtram tombstones automaticamente
- Rebuild periódico (ou manual) limpa completamente
- Tradeoff aceitável entre performance e simplicidade

**Notas**: Para coleções com muitas remoções, recomenda-se rebuild periódico. Podemos implementar rebuild automático baseado em threshold de tombstones no futuro.

---

## ADR-005: Normalização L2 para métrica Cosine

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Busca por similaridade de cosseno pode ser otimizada normalizando vetores para norma L2=1.

**Decisão**: Normalizar vetores para norma L2=1 quando a métrica é Cosine, transformando busca de cosseno em busca L2.

**Alternativas Consideradas**:

1. **Usar distância Cosine nativa do HNSW**
   - ✅ Sem normalização necessária
   - ❌ Menos estável numericamente
   - ❌ Pode ter problemas com vetores de norma zero

2. **Normalizar apenas na busca** (não na inserção)
   - ✅ Vetores originais preservados
   - ❌ Normalização repetida em cada busca
   - ❌ Performance pior

3. **Normalizar na inserção e busca** (escolhido)
   - ✅ Estabilidade numérica melhor
   - ✅ Busca mais rápida (vetores já normalizados)
   - ✅ Transforma Cosine em L2 (mais eficiente)
   - ❌ Vetores normalizados armazenados (não originais)

**Consequências**:
- Performance melhor para Cosine (busca L2 é mais eficiente)
- Estabilidade numérica melhorada
- Vetores originais não são preservados (mas isso é aceitável para Cosine)
- Cálculo em `f64` evita overflow em alta dimensão

**Notas**: Se precisarmos preservar vetores originais, podemos armazenar ambos (normalizado + original), mas isso dobra o uso de memória.

---

## ADR-006: Cache LRU opcional para resultados de busca

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Queries repetidas são comuns em aplicações reais. Cache pode melhorar latência significativamente.

**Decisão**: Implementar cache LRU opcional configurável por coleção.

**Alternativas Consideradas**:

1. **Sem cache**
   - ✅ Simplicidade
   - ✅ Sem overhead de memória
   - ❌ Latência alta para queries repetidas

2. **Cache sempre habilitado**
   - ✅ Melhor performance para queries repetidas
   - ❌ Overhead de memória mesmo quando não necessário
   - ❌ Menos flexível

3. **Cache opcional configurável** (escolhido)
   - ✅ Flexível (habilitado apenas quando necessário)
   - ✅ Configurável por coleção
   - ✅ Pode ser desabilitado para economizar memória
   - ❌ Complexidade adicional no código

**Consequências**:
- Reduz latência para queries repetidas (10-100x mais rápido)
- Uso de memória configurável
- Overhead mínimo quando desabilitado
- Cache pode retornar resultados desatualizados se pontos forem modificados (tradeoff aceitável)

**Notas**: Para aplicações com queries sempre diferentes, cache deve ser desabilitado (`search_cache_size = 0`).

---

## ADR-007: Paralelização com Rayon para batches grandes

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Batch inserts grandes podem se beneficiar de paralelização.

**Decisão**: Paralelizar validação e normalização para batches > 100 pontos usando Rayon.

**Alternativas Consideradas**:

1. **Sem paralelização**
   - ✅ Simplicidade
   - ✅ Sem overhead de sincronização
   - ❌ Performance pior para batches grandes

2. **Paralelização sempre**
   - ✅ Melhor performance
   - ❌ Overhead para batches pequenos
   - ❌ Complexidade desnecessária

3. **Paralelização condicional** (escolhido)
   - ✅ Melhor performance para batches grandes
   - ✅ Sem overhead para batches pequenos
   - ✅ Threshold configurável (100 pontos)
   - ❌ Complexidade adicional

**Consequências**:
- Melhoria de ~50% no tempo de indexação para batches grandes (10k+ pontos)
- Overhead mínimo para batches pequenos (<100 pontos)
- Aproveita múltiplos cores da CPU
- HNSW ainda requer inserção sequencial (não thread-safe)

**Notas**: Se `hnsw_rs` adicionar suporte a inserção paralela no futuro, podemos paralelizar também a inserção.

---

## ADR-008: Validação rigorosa de vetores

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Vetores inválidos (NaN, infinito, dimensão incorreta) podem causar bugs difíceis de debugar.

**Decisão**: Validar rigorosamente todos os vetores na criação do `Point` e nas operações de `Collection`.

**Validações Implementadas**:
- ID não pode ser vazio
- Vetor não pode ser vazio
- Componentes do vetor devem ser finitos (não NaN, não infinito)
- Dimensão deve corresponder à configuração da coleção

**Alternativas Consideradas**:

1. **Validação mínima**
   - ✅ Performance máxima
   - ❌ Bugs difíceis de debugar
   - ❌ Comportamento indefinido com dados inválidos

2. **Validação rigorosa** (escolhido)
   - ✅ Erros claros e específicos
   - ✅ Previne bugs difíceis de debugar
   - ✅ Fail-fast (erro imediato em vez de comportamento estranho)
   - ❌ Overhead mínimo de validação

**Consequências**:
- Erros claros facilitam debugging
- Previne comportamento indefinido
- Overhead de validação é desprezível (<1% do tempo total)
- Código mais robusto e confiável

**Notas**: Validação pode ser desabilitada em builds de release se necessário (não recomendado).

---

## ADR-009: Atomic writes com temp-file + rename

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Precisávamos garantir atomicidade de escritas no disco para evitar corrupção em caso de crash.

**Decisão**: Usar padrão temp-file + rename para todas as escritas em disco.

**Alternativas Consideradas**:

1. **Write direto**
   - ✅ Simplicidade
   - ❌ Risco de corrupção em crash durante escrita
   - ❌ Arquivo parcial pode ser lido

2. **fsync após cada write**
   - ✅ Durabilidade garantida
   - ❌ Muito lento (I/O síncrono)
   - ❌ Ainda não garante atomicidade

3. **Temp-file + rename** (escolhido)
   - ✅ Atomicidade garantida no nível do filesystem
   - ✅ Performance aceitável
   - ✅ Padrão bem conhecido e confiável
   - ❌ Requer espaço temporário em disco

**Consequências**:
- Garante atomicidade (arquivo completo ou não existe)
- Previne corrupção em caso de crash
- Performance aceitável (rename é rápido na maioria dos filesystems)
- Padrão amplamente usado e confiável

**Notas**: Em filesystems que não garantem atomicidade de rename (ex: NFS), pode haver edge cases, mas isso é raro.

---

## ADR-010: Checksum MD5 para validação de integridade

**Status**: Aceito  
**Data**: 2024  
**Contexto**: Precisávamos detectar corrupção de dados em `points.jsonl`.

**Decisão**: Calcular e armazenar checksum MD5 de `points.jsonl` para validação no load.

**Alternativas Consideradas**:

1. **Sem validação**
   - ✅ Simplicidade
   - ❌ Não detecta corrupção
   - ❌ Bugs silenciosos possíveis

2. **SHA-256 ou SHA-512**
   - ✅ Mais seguro criptograficamente
   - ❌ Mais lento que MD5
   - ❌ Overkill para validação de integridade (não segurança)

3. **MD5** (escolhido)
   - ✅ Rápido
   - ✅ Adequado para detecção de corrupção
   - ✅ Amplamente suportado
   - ❌ Não é seguro criptograficamente (mas não é necessário aqui)

**Consequências**:
- Detecta corrupção de dados automaticamente
- Overhead mínimo (<1% do tempo de save/load)
- Erros claros quando corrupção é detectada
- MD5 é adequado para validação de integridade (não segurança)

**Notas**: Se precisarmos de segurança criptográfica no futuro, podemos migrar para SHA-256, mas MD5 é suficiente para detecção de corrupção.

---

## Resumo de Decisões

| ADR | Decisão | Status |
|-----|---------|--------|
| ADR-001 | Usar `hnsw_rs` em vez de implementação própria | ✅ Aceito |
| ADR-002 | Formato JSON-lines para persistência | ✅ Aceito |
| ADR-003 | Trait Object para abstração de índice | ✅ Aceito |
| ADR-004 | Tombstones para remoção | ✅ Aceito |
| ADR-005 | Normalização L2 para Cosine | ✅ Aceito |
| ADR-006 | Cache LRU opcional | ✅ Aceito |
| ADR-007 | Paralelização condicional com Rayon | ✅ Aceito |
| ADR-008 | Validação rigorosa de vetores | ✅ Aceito |
| ADR-009 | Atomic writes com temp-file + rename | ✅ Aceito |
| ADR-010 | Checksum MD5 para validação | ✅ Aceito |

## Decisões Futuras (Rascunho)

### ADR-011: Write-ahead log (WAL) para melhor durabilidade

**Status**: Proposto  
**Contexto**: Save completo após cada operação é lento para workloads write-heavy.

**Proposta**: Implementar WAL para escritas incrementais, com checkpoint periódico.

**Benefícios**:
- Melhor throughput de escrita
- Durabilidade garantida
- Recuperação após crash

**Tradeoffs**:
- Complexidade adicional
- Overhead de I/O para WAL
- Necessidade de compactação periódica

---

### ADR-012: Compressão de vetores (quantização)

**Status**: Proposto  
**Contexto**: Vetores ocupam muita memória em coleções grandes.

**Proposta**: Implementar quantização (ex: int8) para reduzir uso de memória.

**Benefícios**:
- Redução de 4x no uso de memória (f32 → int8)
- Performance melhor (menos cache misses)

**Tradeoffs**:
- Perda de precisão
- Overhead de conversão
- Complexidade adicional

---

## Referências

- [Architecture Decision Records](https://adr.github.io/) - Formato ADR
- [Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) - Artigo original sobre ADRs
