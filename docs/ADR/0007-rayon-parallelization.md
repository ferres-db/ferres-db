# ADR-007: Paralelização com Rayon para batches grandes

**Status**: Aceito  
**Data**: 2024

## Contexto

Batch inserts grandes podem se beneficiar de paralelização.

## Decisão

Paralelizar validação e normalização para batches > 100 pontos usando Rayon.

## Alternativas Consideradas

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

## Consequências

- Melhoria de ~50% no tempo de indexação para batches grandes (10k+ pontos)
- Overhead mínimo para batches pequenos (<100 pontos)
- Aproveita múltiplos cores da CPU
- HNSW ainda requer inserção sequencial (não thread-safe)

## Notas

Se `hnsw_rs` adicionar suporte a inserção paralela no futuro, podemos paralelizar também a inserção.
