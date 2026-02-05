//! # FerresDB Server
//!
//! Servidor HTTP do FerresDB usando Axum.

pub mod error;
pub mod handlers;
pub mod middleware;
pub mod metrics;
pub mod query_logger;
pub mod routes;
pub mod state;

