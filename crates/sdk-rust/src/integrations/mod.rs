//! Integrações com ecossistema RAG e orquestração de busca.
//!
//! Este módulo expõe wrappers compatíveis com interfaces comuns de VectorStore
//! (LangChain, LlamaIndex, etc.), permitindo usar o FerresDB como backend de
//! armazenamento vetorial em pipelines RAG.

mod vector_store;

pub use vector_store::{FerresDbVectorStore, VectorStore, VectorStoreDoc};
pub use crate::{FerresDbClient, SdkError};
