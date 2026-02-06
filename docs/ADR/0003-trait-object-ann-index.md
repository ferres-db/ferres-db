# ADR-003: Trait Object para abstração de índice ANN

**Status**: Aceito  
**Data**: 2024

## Contexto

Queríamos desacoplar `Collection` do backend específico de busca (HNSW).

## Decisão

Usar `Box<dyn ANNIndex>` (trait object) em vez de tipo genérico ou enum.

## Alternativas Consideradas

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

## Consequências

- Facilita testes unitários (mock `ANNIndex`)
- Permite múltiplas implementações sem modificar `Collection`
- Overhead de vtable é desprezível (<1% do tempo de busca)
- Código mais limpo e extensível

## Notas

Se profiling mostrar que vtable overhead é significativo, podemos considerar enum com variantes específicas, mas isso é improvável dado o custo da busca HNSW.
