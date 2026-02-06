# ADR-009: Atomic writes com temp-file + rename

**Status**: Aceito  
**Data**: 2024

## Contexto

Precisávamos garantir atomicidade de escritas no disco para evitar corrupção em caso de crash.

## Decisão

Usar padrão temp-file + rename para todas as escritas em disco.

## Alternativas Consideradas

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

## Consequências

- Garante atomicidade (arquivo completo ou não existe)
- Previne corrupção em caso de crash
- Performance aceitável (rename é rápido na maioria dos filesystems)
- Padrão amplamente usado e confiável

## Notas

Em filesystems que não garantem atomicidade de rename (ex: NFS), pode haver edge cases, mas isso é raro.
