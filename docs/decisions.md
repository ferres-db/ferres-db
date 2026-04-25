# Architecture Decision Records (ADRs)

Decisions are documented in individual files in **[docs/ADR/](ADR/)**. This file maintains a summary and the legacy history; for the full content of each decision, consult the files in the ADR folder.

## ADR-001: Using HNSW instead of a custom implementation

**Status**: Accepted  
**Date**: 2024  
**Context**: We needed an efficient ANN algorithm for vector search.

**Decision**: Use the `hnsw_rs` library instead of implementing HNSW from scratch.

**Alternatives Considered**:

1. **Implement HNSW from scratch**
   - ✅ Full control over implementation
   - ❌ Very long development time
   - ❌ Risk of bugs and suboptimal performance
   - ❌ Continuous maintenance required

2. **Use another ANN library** (e.g. `usearch`, `faiss-rs`)
   - ✅ Mature implementation
   - ❌ `usearch`: Fewer features, smaller community
   - ❌ `faiss-rs`: C++ bindings, additional complexity

3. **Use `hnsw_rs`** (chosen)
   - ✅ Native Rust library (no FFI)
   - ✅ Clean and well-documented API
   - ✅ Support for multiple distance metrics
   - ✅ Actively maintained
   - ❌ Less control over internal details

**Consequences**:

- Faster development (we focused on API and storage)
- External dependency (but well maintained)
- Excellent performance (benchmarks confirm)
- Easier maintenance (bugs fixed upstream)

**Notes**: If we need specific features not supported by `hnsw_rs`, we can consider forking or a custom implementation in the future.

---

## ADR-002: JSON-lines format for persistence

**Status**: Accepted  
**Date**: 2024  
**Context**: We needed a persistence format that allowed incremental append and was easy to debug.

**Decision**: Use JSON-lines format (`.jsonl`) to store points.

**Alternatives Considered**:

1. **Full bincode**
   - ✅ Very fast serialization
   - ✅ Compact size
   - ❌ Does not allow incremental append
   - ❌ Hard to debug (binary)
   - ❌ Partial recovery after crash is difficult

2. **SQLite**
   - ✅ ACID transactions
   - ✅ SQL queries
   - ❌ Unnecessary overhead for simple data
   - ❌ Heavy external dependency
   - ❌ Additional complexity

3. **Protocol Buffers**
   - ✅ Compact and efficient
   - ✅ Versioned schema
   - ❌ Less readable than JSON
   - ❌ Requires schema definitions
   - ❌ Incremental append is more complex

4. **JSON-lines** (chosen)
   - ✅ Simple incremental append
   - ✅ Streaming reads
   - ✅ Partial recovery after crash
   - ✅ Easy to debug (human-readable text)
   - ✅ Compatible with Unix tools
   - ❌ Slower than bincode for full reads
   - ❌ Larger size than bincode

**Consequences**:

- Facilitates debugging and manual inspection
- Enables future WAL (write-ahead log) implementation
- Acceptable performance tradeoff for flexibility benefits
- Well-known and supported format

**Notes**: For very large collections (>10M points), we may consider an optimized binary format in the future, but JSON-lines is adequate for most cases.

---

## ADR-003: Trait Object for ANN index abstraction

**Status**: Accepted  
**Date**: 2024  
**Context**: We wanted to decouple `Collection` from the specific search backend (HNSW).

**Decision**: Use `Box<dyn ANNIndex>` (trait object) instead of a generic type or enum.

**Alternatives Considered**:

1. **Generic type** (`Collection<I: ANNIndex>`)
   - ✅ Zero-cost abstraction (monomorphization)
   - ✅ Maximum performance
   - ❌ `Collection` cannot easily be stored in a `HashMap`
   - ❌ Type complexity increases

2. **Enum with variants** (`enum IndexType { Hnsw, BruteForce }`)
   - ✅ Static dispatch (fast)
   - ✅ No heap allocation
   - ❌ Less extensible (requires modifying enum for new backends)
   - ❌ Match statements in every method

3. **Trait Object** (`Box<dyn ANNIndex>`) (chosen)
   - ✅ Extensible (any type can implement)
   - ✅ Easy to test (mock implementations)
   - ✅ Decouples `Collection` from backend
   - ❌ vtable overhead (negligible compared to HNSW cost)
   - ❌ Heap allocation (once, acceptable)

**Consequences**:

- Facilitates unit tests (mock `ANNIndex`)
- Allows multiple implementations without modifying `Collection`
- vtable overhead is negligible (<1% of search time)
- Cleaner and more extensible code

**Notes**: If profiling shows that vtable overhead is significant, we can consider an enum with specific variants, but this is unlikely given the cost of HNSW search.

---

## ADR-004: Tombstones instead of real HNSW removal

**Status**: Accepted  
**Date**: 2024  
**Context**: HNSW does not support native graph removal. We needed a strategy for removing points.

**Decision**: Use "tombstones" (logical marking) instead of physical removal or a full rebuild.

**Alternatives Considered**:

1. **Full rebuild after each removal**
   - ✅ Completely cleans the index
   - ✅ No orphan points
   - ❌ Very slow for large collections (O(n log n))
   - ❌ Blocks all operations during rebuild

2. **Custom HNSW removal implementation**
   - ✅ Real removal
   - ✅ Acceptable performance
   - ❌ Very high complexity
   - ❌ Requires significant research and development
   - ❌ Risk of bugs

3. **Tombstones** (chosen)
   - ✅ Simple to implement
   - ✅ Does not block operations
   - ✅ Immediate removal (filtered in searches)
   - ❌ Points remain in the graph until rebuild
   - ❌ Slightly higher memory usage

**Consequences**:

- Removal is O(1) (just marking)
- Searches filter tombstones automatically
- Periodic (or manual) rebuild cleans completely
- Acceptable tradeoff between performance and simplicity

**Notes**: For collections with many removals, periodic rebuild is recommended. We can implement automatic rebuild based on a tombstone threshold in the future.

---

## ADR-005: L2 normalization for Cosine metric

**Status**: Accepted  
**Date**: 2024  
**Context**: Cosine similarity search can be optimized by normalizing vectors to L2 norm = 1.

**Decision**: Normalize vectors to L2 norm = 1 when the metric is Cosine, transforming cosine search into L2 search.

**Alternatives Considered**:

1. **Use native Cosine distance from HNSW**
   - ✅ No normalization required
   - ❌ Less numerically stable
   - ❌ May have issues with zero-norm vectors

2. **Normalize only at search time** (not at insertion)
   - ✅ Original vectors preserved
   - ❌ Repeated normalization at every search
   - ❌ Worse performance

3. **Normalize at insertion and search** (chosen)
   - ✅ Better numerical stability
   - ✅ Faster search (vectors already normalized)
   - ✅ Transforms Cosine into L2 (more efficient)
   - ❌ Normalized vectors stored (not originals)

**Consequences**:

- Better performance for Cosine (L2 search is more efficient)
- Improved numerical stability
- Original vectors are not preserved (but this is acceptable for Cosine)
- Calculation in `f64` avoids overflow at high dimension

**Notes**: If we need to preserve original vectors, we can store both (normalized + original), but this doubles memory usage.

---

## ADR-006: Optional LRU cache for search results

**Status**: Accepted  
**Date**: 2024  
**Context**: Repeated queries are common in real applications. Cache can significantly improve latency.

**Decision**: Implement an optional configurable LRU cache per collection.

**Alternatives Considered**:

1. **No cache**
   - ✅ Simplicity
   - ✅ No memory overhead
   - ❌ High latency for repeated queries

2. **Always-on cache**
   - ✅ Better performance for repeated queries
   - ❌ Memory overhead even when not needed
   - ❌ Less flexible

3. **Optional configurable cache** (chosen)
   - ✅ Flexible (enabled only when needed)
   - ✅ Configurable per collection
   - ✅ Can be disabled to save memory
   - ❌ Additional code complexity

**Consequences**:

- Reduces latency for repeated queries (10–100× faster)
- Configurable memory usage
- Minimal overhead when disabled
- Cache may return stale results if points are modified (acceptable tradeoff)

**Notes**: For applications where queries are always different, cache should be disabled (`search_cache_size = 0`).

---

## ADR-007: Parallelization with Rayon for large batches

**Status**: Accepted  
**Date**: 2024  
**Context**: Large batch inserts can benefit from parallelization.

**Decision**: Parallelize validation and normalization for batches > 100 points using Rayon.

**Alternatives Considered**:

1. **No parallelization**
   - ✅ Simplicity
   - ✅ No synchronization overhead
   - ❌ Worse performance for large batches

2. **Always parallelize**
   - ✅ Better performance
   - ❌ Overhead for small batches
   - ❌ Unnecessary complexity

3. **Conditional parallelization** (chosen)
   - ✅ Better performance for large batches
   - ✅ No overhead for small batches
   - ✅ Configurable threshold (100 points)
   - ❌ Additional complexity

**Consequences**:

- ~50% improvement in indexing time for large batches (10k+ points)
- Minimal overhead for small batches (<100 points)
- Takes advantage of multiple CPU cores
- HNSW still requires sequential insertion (not thread-safe)

**Notes**: If `hnsw_rs` adds support for parallel insertion in the future, we can parallelize the insertion as well.

---

## ADR-008: Strict vector validation

**Status**: Accepted  
**Date**: 2024  
**Context**: Invalid vectors (NaN, infinity, incorrect dimension) can cause hard-to-debug bugs.

**Decision**: Strictly validate all vectors when creating `Point` and in `Collection` operations.

**Implemented Validations**:

- ID cannot be empty
- Vector cannot be empty
- Vector components must be finite (not NaN, not infinity)
- Dimension must match the collection configuration

**Alternatives Considered**:

1. **Minimal validation**
   - ✅ Maximum performance
   - ❌ Hard-to-debug bugs
   - ❌ Undefined behavior with invalid data

2. **Strict validation** (chosen)
   - ✅ Clear and specific errors
   - ✅ Prevents hard-to-debug bugs
   - ✅ Fail-fast (immediate error instead of strange behavior)
   - ❌ Minimal validation overhead

**Consequences**:

- Clear errors facilitate debugging
- Prevents undefined behavior
- Validation overhead is negligible (<1% of total time)
- More robust and reliable code

**Notes**: Validation can be disabled in release builds if necessary (not recommended).

---

## ADR-009: Atomic writes with temp-file + rename

**Status**: Accepted  
**Date**: 2024  
**Context**: We needed to guarantee write atomicity on disk to avoid corruption in case of crash.

**Decision**: Use the temp-file + rename pattern for all disk writes.

**Alternatives Considered**:

1. **Direct write**
   - ✅ Simplicity
   - ❌ Risk of corruption on crash during write
   - ❌ Partial file may be read

2. **fsync after each write**
   - ✅ Guaranteed durability
   - ❌ Very slow (synchronous I/O)
   - ❌ Still does not guarantee atomicity

3. **Temp-file + rename** (chosen)
   - ✅ Atomicity guaranteed at filesystem level
   - ✅ Acceptable performance
   - ✅ Well-known and reliable pattern
   - ❌ Requires temporary disk space

**Consequences**:

- Guarantees atomicity (complete file or nothing)
- Prevents corruption in case of crash
- Acceptable performance (rename is fast on most filesystems)
- Widely used and reliable pattern

**Notes**: On filesystems that do not guarantee rename atomicity (e.g. NFS), there may be edge cases, but this is rare.

---

## ADR-010: MD5 checksum for integrity validation

**Status**: Accepted  
**Date**: 2024  
**Context**: We needed to detect data corruption in `points.jsonl`.

**Decision**: Calculate and store MD5 checksum of `points.jsonl` for validation on load.

**Alternatives Considered**:

1. **No validation**
   - ✅ Simplicity
   - ❌ Does not detect corruption
   - ❌ Silent bugs possible

2. **SHA-256 or SHA-512**
   - ✅ More cryptographically secure
   - ❌ Slower than MD5
   - ❌ Overkill for integrity validation (not security)

3. **MD5** (chosen)
   - ✅ Fast
   - ✅ Adequate for corruption detection
   - ✅ Widely supported
   - ❌ Not cryptographically secure (but not needed here)

**Consequences**:

- Automatically detects data corruption
- Minimal overhead (<1% of save/load time)
- Clear errors when corruption is detected
- MD5 is adequate for integrity validation (not security)

**Notes**: If we need cryptographic security in the future, we can migrate to SHA-256, but MD5 is sufficient for corruption detection.

---

## Decision Summary

| ADR     | Decision                                           | Status      |
| ------- | -------------------------------------------------- | ----------- |
| ADR-001 | Use `hnsw_rs` instead of custom implementation     | ✅ Accepted |
| ADR-002 | JSON-lines format for persistence                  | ✅ Accepted |
| ADR-003 | Trait Object for index abstraction                 | ✅ Accepted |
| ADR-004 | Tombstones for removal                             | ✅ Accepted |
| ADR-005 | L2 normalization for Cosine                        | ✅ Accepted |
| ADR-006 | Optional LRU cache                                 | ✅ Accepted |
| ADR-007 | Conditional parallelization with Rayon             | ✅ Accepted |
| ADR-008 | Strict vector validation                           | ✅ Accepted |
| ADR-009 | Atomic writes with temp-file + rename              | ✅ Accepted |
| ADR-010 | MD5 checksum for validation                        | ✅ Accepted |

## Future Decisions (Draft)

### ADR-011: Write-ahead log (WAL) for better durability

**Status**: Proposed  
**Context**: Full save after each operation is slow for write-heavy workloads.

**Proposal**: Implement WAL for incremental writes, with periodic checkpointing.

**Benefits**:

- Better write throughput
- Guaranteed durability
- Recovery after crash

**Tradeoffs**:

- Additional complexity
- I/O overhead for WAL
- Need for periodic compaction

---

### ADR-012: Vector compression (quantization)

**Status**: Proposed  
**Context**: Vectors consume a lot of memory in large collections.

**Proposal**: Implement quantization (e.g. int8) to reduce memory usage.

**Benefits**:

- 4× reduction in memory usage (f32 → int8)
- Better performance (fewer cache misses)

**Tradeoffs**:

- Loss of precision
- Conversion overhead
- Additional complexity

---

## References

- [Architecture Decision Records](https://adr.github.io/) - ADR format
- [Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) - Original article on ADRs
