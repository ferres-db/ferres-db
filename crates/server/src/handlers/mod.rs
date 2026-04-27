//! # Handlers — lógica de negócio dos endpoints HTTP
//!
//! Cada módulo contém os handlers para um grupo de rotas relacionadas.

pub mod audit;
pub mod auth;
pub mod backup;
pub mod collections;
pub mod debug;
pub mod graph;
pub mod health;
pub mod keys;
pub mod llm_credentials;
pub mod llm_proxy;
pub mod metrics;
pub mod points;
pub mod reindex;
pub mod restore;
pub mod save;
pub mod settings;
pub mod stats;
pub mod streaming;
pub mod users;
