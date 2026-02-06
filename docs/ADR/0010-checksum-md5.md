# ADR-010: Checksum MD5 para validação de integridade

**Status**: Aceito  
**Data**: 2024

## Contexto

Precisávamos detectar corrupção de dados em `points.jsonl`.

## Decisão

Calcular e armazenar checksum MD5 de `points.jsonl` para validação no load.

## Alternativas Consideradas

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

## Consequências

- Detecta corrupção de dados automaticamente
- Overhead mínimo (<1% do tempo de save/load)
- Erros claros quando corrupção é detectada
- MD5 é adequado para validação de integridade (não segurança)

## Notas

Se precisarmos de segurança criptográfica no futuro, podemos migrar para SHA-256, mas MD5 é suficiente para detecção de corrupção.
