//! # FerresDB Server
//!
//! Servidor HTTP do FerresDB usando Axum.

pub mod api_keys;
pub mod auth;
pub mod users;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod request_validation;
pub mod metrics;
pub mod query_logger;
pub mod query_log_analytics;
pub mod routes;
pub mod state;

#[cfg(feature = "otel")]
pub mod tracing_otel;

