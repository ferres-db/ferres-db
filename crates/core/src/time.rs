//! # Time helpers — clock-safe Unix timestamp accessors
//!
//! O padrão `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` causa panic
//! se o relógio do sistema retroceder antes da época Unix (cenário improvável,
//! porém possível em VMs/containers com clock skew). Este módulo centraliza o
//! acesso ao relógio e degrada para `0` em vez de panic.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Retorna o tempo Unix em segundos. Em caso de relógio retrocedido
/// (improvável mas possível), retorna `0` em vez de panic.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Retorna o tempo Unix em milissegundos, com mesmo fallback de `unix_now`.
pub fn unix_now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Retorna o `Duration` completo desde a época Unix, ou `Duration::ZERO`
/// caso o relógio esteja antes de `UNIX_EPOCH`.
pub fn unix_duration() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn unix_now_is_positive() {
        assert!(unix_now() > 0);
    }

    #[test]
    fn unix_now_millis_is_positive() {
        assert!(unix_now_millis() > 0);
    }

    #[test]
    fn unix_duration_is_positive() {
        assert!(unix_duration() > Duration::ZERO);
    }

    #[test]
    fn millis_is_consistent_with_secs() {
        let secs = unix_now();
        let millis = unix_now_millis();
        assert!(millis as u64 / 1000 >= secs);
    }
}
