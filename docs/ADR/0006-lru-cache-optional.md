# ADR-006: Cache LRU opcional para resultados de busca

**Status**: Aceito  
**Data**: 2024

## Contexto

Queries repetidas são comuns em aplicações reais. Cache pode melhorar latência significativamente.

## Decisão

Implementar cache LRU opcional configurável por coleção.

## Alternativas Consideradas

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

## Consequências

- Reduz latência para queries repetidas (10-100x mais rápido)
- Uso de memória configurável
- Overhead mínimo quando desabilitado
- Cache pode retornar resultados desatualizados se pontos forem modificados (tradeoff aceitável)

## Notas

Para aplicações com queries sempre diferentes, cache deve ser desabilitado (`search_cache_size = 0`).
