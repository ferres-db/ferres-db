//! # FerresDB Server
//!
//! Servidor HTTP do FerresDB usando Axum.

pub mod api_keys;
pub mod audit;
pub mod auth;
pub mod cloud_settings;
pub mod users;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod permissions;
pub mod request_validation;
pub mod metrics;
pub mod query_logger;
pub mod query_log_analytics;
pub mod routes;
pub mod warmup;
pub mod state;

#[cfg(feature = "otel")]
pub mod tracing_otel;

#[cfg(feature = "grpc")]
pub mod grpc;

#[cfg(feature = "grpc")]
pub mod replication;

#[cfg(feature = "mcp")]
pub mod mcp;
