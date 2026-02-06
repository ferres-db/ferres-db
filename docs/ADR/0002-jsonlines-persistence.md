# ADR-002: Formato JSON-lines para persistência

**Status**: Aceito  
**Data**: 2024

## Contexto

Precisávamos de um formato de persistência que permitisse append incremental e fosse fácil de debugar.

## Decisão

Usar formato JSON-lines (`.jsonl`) para armazenar pontos.

## Alternativas Consideradas

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

## Consequências

- Facilita debugging e inspeção manual
- Permite implementação futura de WAL (write-ahead log)
- Tradeoff de performance aceitável para benefícios de flexibilidade
- Formato bem conhecido e suportado

## Notas

Para coleções muito grandes (>10M pontos), podemos considerar formato binário otimizado no futuro, mas JSON-lines é adequado para maioria dos casos.
