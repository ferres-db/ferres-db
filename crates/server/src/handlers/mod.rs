//! # Handlers — lógica de negócio dos endpoints HTTP
//!
//! Cada módulo contém os handlers para um grupo de rotas relacionadas.

pub mod audit;
pub mod auth;
pub mod collections;
pub mod users;
pub mod debug;
pub mod health;
pub mod keys;
pub mod metrics;
pub mod points;
pub mod save;
pub mod stats;
pub mod streaming;

