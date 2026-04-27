//! Helpers de tempo do servidor.
//!
//! Este módulo re-exporta os helpers definidos em [`ferres_db_core::time`]
//! para que o código do servidor use o mesmo ponto de entrada e mantenha
//! comportamento consistente em caso de relógio retrocedido (ver `core::time`).

pub use ferres_db_core::time::{unix_duration, unix_now, unix_now_millis};
