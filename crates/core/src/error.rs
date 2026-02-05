//! # Tipos de erro do FerresDB Core
//!
//! Usa `thiserror` para derivar `Display` e `Error` automaticamente.
//! Cada variante mapeia uma classe de falha distinta, facilitando
//! pattern matching no server e no SDK.

/// Erros que podem ocorrer nas operações do core.
#[derive(Debug, thiserror::Error)]
pub enum FerresError {
    /// O vetor inserido tem dimensão diferente da configurada na coleção.
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    /// O ID do ponto é inválido (vazio, formato incorreto, etc).
    #[error("invalid point id: {0}")]
    InvalidPointId(String),

    /// O vetor está vazio (zero dimensões).
    #[error("vector cannot be empty")]
    EmptyVector,

    /// O vetor contém valores inválidos (NaN, infinito, etc).
    #[error("invalid vector: {reason}")]
    InvalidVector { reason: String },

    /// A coleção requisitada não existe.
    #[error("collection not found: {0}")]
    CollectionNotFound(String),

    /// Uma coleção com esse nome já existe.
    #[error("collection already exists: {0}")]
    CollectionAlreadyExists(String),

    /// O ponto requisitado não existe na coleção.
    #[error("point not found: {0}")]
    PointNotFound(String),

    /// Tentativa de busca ou operação em um índice que ainda não foi construído.
    #[error("index not built: call build() before searching")]
    IndexNotBuilt,

    /// Erro na camada de armazenamento (I/O, serialização, etc).
    #[error("storage error: {0}")]
    Storage(String),
}
