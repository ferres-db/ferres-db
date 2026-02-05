//! # FerresDB SDK
//!
//! SDK Rust para interação com o FerresDB.
//! Fornece um client type-safe para operações CRUD em coleções e busca vetorial.

// Re-exporta tipos do core para que consumidores do SDK
// não precisem depender diretamente do crate core.
pub use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, Point};

// TODO: implementar FerresDbClient com conexão HTTP/gRPC ao server
