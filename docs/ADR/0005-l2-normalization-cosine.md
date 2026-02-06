# ADR-005: Normalização L2 para métrica Cosine

**Status**: Aceito  
**Data**: 2024

## Contexto

Busca por similaridade de cosseno pode ser otimizada normalizando vetores para norma L2=1.

## Decisão

Normalizar vetores para norma L2=1 quando a métrica é Cosine, transformando busca de cosseno em busca L2.

## Alternativas Consideradas

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

## Consequências

- Performance melhor para Cosine (busca L2 é mais eficiente)
- Estabilidade numérica melhorada
- Vetores originais não são preservados (mas isso é aceitável para Cosine)
- Cálculo em `f64` evita overflow em alta dimensão

## Notas

Se precisarmos preservar vetores originais, podemos armazenar ambos (normalizado + original), mas isso dobra o uso de memória.
