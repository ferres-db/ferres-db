//! # Request Validation — validação centralizada de payloads
//!
//! Limites únicos para prevenir DoS (vetores gigantes, batches excessivos).
//! Usado por handlers de points (upsert, delete, search).

use crate::error::{ApiError, ApiResult};
use crate::handlers::points::UpsertPointsRequest;

// ─── Limites (Segurança & Resiliência) ───────────────────────────────────

/// Dimensão máxima de um vetor (evita OOM e DoS).
pub const MAX_VECTOR_DIM: usize = 4096;

/// Número máximo de pontos por batch de upsert.
pub const MAX_POINTS_PER_BATCH: usize = 1000;

/// Número máximo de IDs por batch de delete.
pub const MAX_IDS_PER_DELETE_BATCH: usize = 10_000;

/// Limite máximo de resultados em uma busca (search/hybrid).
pub const MAX_SEARCH_LIMIT: usize = 10_000;

// ─── Validação de Upsert ─────────────────────────────────────────────────

/// Valida o payload de upsert antes de qualquer processamento.
/// Rejeita batches grandes e vetores com dimensão excessiva (previne OOM).
pub fn validate_upsert_request(body: &UpsertPointsRequest) -> ApiResult<()> {
    if body.points.len() > MAX_POINTS_PER_BATCH {
        return Err(ApiError::invalid_payload(format!(
            "batch too large: max {MAX_POINTS_PER_BATCH} points per request"
        )));
    }
    for (i, p) in body.points.iter().enumerate() {
        if p.vector.len() > MAX_VECTOR_DIM {
            return Err(ApiError::invalid_payload(format!(
                "vector dimension too large at point index {i}: max {MAX_VECTOR_DIM}"
            )));
        }
    }
    Ok(())
}

/// Valida um único vetor (dimensão máxima). Útil para search/hybrid.
pub fn validate_vector_dimension(vector: &[f32]) -> ApiResult<()> {
    if vector.len() > MAX_VECTOR_DIM {
        return Err(ApiError::invalid_payload(format!(
            "vector dimension too large: max {MAX_VECTOR_DIM}"
        )));
    }
    Ok(())
}

/// Valida o número de IDs em um delete em batch (evita DoS).
pub fn validate_delete_batch_size(ids_len: usize) -> ApiResult<()> {
    if ids_len == 0 {
        return Err(ApiError::invalid_payload("ids cannot be empty"));
    }
    if ids_len > MAX_IDS_PER_DELETE_BATCH {
        return Err(ApiError::invalid_payload(format!(
            "batch too large: max {MAX_IDS_PER_DELETE_BATCH} ids per delete request"
        )));
    }
    Ok(())
}

/// Valida o limite de resultados em uma busca.
pub fn validate_search_limit(limit: usize) -> ApiResult<()> {
    if limit > MAX_SEARCH_LIMIT {
        return Err(ApiError::invalid_payload(format!(
            "limit too large: max {MAX_SEARCH_LIMIT}"
        )));
    }
    if limit == 0 {
        return Err(ApiError::invalid_payload("limit must be greater than 0"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::handlers::points::PointInput;

    #[test]
    fn test_validate_upsert_batch_too_large() {
        let mut points = Vec::new();
        for i in 0..MAX_POINTS_PER_BATCH + 1 {
            points.push(PointInput {
                id: format!("id-{i}"),
                vector: vec![0.0; 4],
                metadata: serde_json::Value::Null,
                namespace: None,
                ttl: None,
            });
        }
        let body = UpsertPointsRequest { points };
        assert!(validate_upsert_request(&body).is_err());
    }

    #[test]
    fn test_validate_upsert_vector_too_large() {
        let body = UpsertPointsRequest {
            points: vec![PointInput {
                id: "x".to_string(),
                vector: vec![0.0; MAX_VECTOR_DIM + 1],
                metadata: serde_json::Value::Null,
                namespace: None,
                ttl: None,
            }],
        };
        assert!(validate_upsert_request(&body).is_err());
    }

    #[test]
    fn test_validate_upsert_ok() {
        let body = UpsertPointsRequest {
            points: vec![PointInput {
                id: "x".to_string(),
                vector: vec![0.0; 128],
                metadata: serde_json::Value::Null,
                namespace: None,
                ttl: None,
            }],
        };
        assert!(validate_upsert_request(&body).is_ok());
    }

    #[test]
    fn test_validate_search_limit_zero() {
        assert!(validate_search_limit(0).is_err());
    }

    #[test]
    fn test_validate_search_limit_over_max() {
        assert!(validate_search_limit(MAX_SEARCH_LIMIT + 1).is_err());
    }

    #[test]
    fn test_validate_search_limit_ok() {
        assert!(validate_search_limit(100).is_ok());
        assert!(validate_search_limit(MAX_SEARCH_LIMIT).is_ok());
    }
}
