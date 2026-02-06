# ADR-004: Tombstones em vez de remoção real do HNSW

**Status**: Aceito  
**Data**: 2024

## Contexto

HNSW não suporta remoção nativa do grafo. Precisávamos de uma estratégia para remover pontos.

## Decisão

Usar "tombstones" (marcação lógica) em vez de remoção física ou rebuild completo.

## Alternativas Consideradas

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

## Consequências

- Remoção é O(1) (apenas marcação)
- Buscas filtram tombstones automaticamente
- Rebuild periódico (ou manual) limpa completamente
- Tradeoff aceitável entre performance e simplicidade

## Notas

Para coleções com muitas remoções, recomenda-se rebuild periódico. Podemos implementar rebuild automático baseado em threshold de tombstones no futuro.
