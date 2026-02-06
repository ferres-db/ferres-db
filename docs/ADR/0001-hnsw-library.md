# ADR-001: Uso de HNSW em vez de implementação própria

**Status**: Aceito  
**Data**: 2024

## Contexto

Precisávamos de um algoritmo ANN eficiente para busca vetorial.

## Decisão

Usar a biblioteca `hnsw_rs` em vez de implementar HNSW do zero.

## Alternativas Consideradas

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

## Consequências

- Desenvolvimento mais rápido (focamos na API e storage)
- Dependência externa (mas bem mantida)
- Performance excelente (benchmarks confirmam)
- Facilita manutenção (bugs corrigidos upstream)

## Notas

Se precisarmos de features específicas não suportadas pelo `hnsw_rs`, podemos considerar fork ou implementação própria no futuro.
