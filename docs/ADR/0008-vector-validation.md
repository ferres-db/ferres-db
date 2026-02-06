# ADR-008: Validação rigorosa de vetores

**Status**: Aceito  
**Data**: 2024

## Contexto

Vetores inválidos (NaN, infinito, dimensão incorreta) podem causar bugs difíceis de debugar.

## Decisão

Validar rigorosamente todos os vetores na criação do `Point` e nas operações de `Collection`.

**Validações implementadas:**

- ID não pode ser vazio
- Vetor não pode ser vazio
- Componentes do vetor devem ser finitos (não NaN, não infinito)
- Dimensão deve corresponder à configuração da coleção

## Alternativas Consideradas

1. **Validação mínima**
   - ✅ Performance máxima
   - ❌ Bugs difíceis de debugar
   - ❌ Comportamento indefinido com dados inválidos

2. **Validação rigorosa** (escolhido)
   - ✅ Erros claros e específicos
   - ✅ Previne bugs difíceis de debugar
   - ✅ Fail-fast (erro imediato em vez de comportamento estranho)
   - ❌ Overhead mínimo de validação

## Consequências

- Erros claros facilitam debugging
- Previne comportamento indefinido
- Overhead de validação é desprezível (<1% do tempo total)
- Código mais robusto e confiável

## Notas

Validação pode ser desabilitada em builds de release se necessário (não recomendado).
