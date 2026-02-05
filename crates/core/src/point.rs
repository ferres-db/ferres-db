//! # Point — unidade fundamental de dado vetorial
//!
//! Cada `Point` representa um vetor n-dimensional associado a um ID
//! (String livre — pode ser UUID, slug, hash, etc.), metadados JSON
//! arbitrários e um timestamp de criação.
//!
//! ## Decisões arquiteturais
//!
//! - **`id: String`** em vez de UUID tipado: permite que o chamador
//!   escolha o formato do identificador (UUID v4, ULID, slug, etc.)
//!   sem impor uma dependência no tipo.
//!
//! - **`metadata: serde_json::Value`**: aceita qualquer documento JSON
//!   válido (objeto, array, null). Mais flexível que `HashMap` pois
//!   suporta dados aninhados diretamente.
//!
//! - **`created_at: u64`**: timestamp Unix em segundos. Valor numérico
//!   simples que serializa de forma compacta e não depende de crate
//!   de datetime.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::FerresError;

/// Representa um ponto (vetor + metadados) no espaço vetorial.
///
/// O campo `vector` contém as coordenadas f32 para busca por
/// similaridade. `metadata` carrega contexto arbitrário em JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    pub created_at: u64,
}

impl Point {
    /// Cria um novo ponto com validação básica.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::Point;
    ///
    /// let point = Point::new(
    ///     "doc-1",
    ///     vec![0.1, 0.2, 0.3],
    ///     serde_json::json!({"text": "Hello world"})
    /// )?;
    ///
    /// assert_eq!(point.id, "doc-1");
    /// assert_eq!(point.dimension(), 3);
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Validações
    /// - `id` não pode ser vazio
    /// - `vector` não pode ser vazio (precisa ter ao menos 1 dimensão)
    /// - Nenhum componente do vetor pode ser NaN ou infinito
    ///
    /// `created_at` é preenchido automaticamente com o timestamp atual.
    pub fn new(
        id: impl Into<String>,
        vector: Vec<f32>,
        metadata: serde_json::Value,
    ) -> Result<Self, FerresError> {
        let id = id.into();

        if id.is_empty() {
            return Err(FerresError::InvalidPointId(
                "point id cannot be empty".into(),
            ));
        }

        if vector.is_empty() {
            return Err(FerresError::EmptyVector);
        }

        if let Some(pos) = vector.iter().position(|v| !v.is_finite()) {
            return Err(FerresError::InvalidVector {
                reason: format!("non-finite value at index {pos}"),
            });
        }

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs();

        Ok(Self {
            id,
            vector,
            metadata,
            created_at,
        })
    }

    /// Retorna a dimensionalidade do vetor.
    #[inline]
    pub fn dimension(&self) -> usize {
        self.vector.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_creation_with_valid_data() {
        let meta = serde_json::json!({"label": "test", "score": 0.95});
        let point = Point::new("point-1", vec![1.0, 2.0, 3.0], meta.clone()).unwrap();

        assert_eq!(point.id, "point-1");
        assert_eq!(point.vector, vec![1.0, 2.0, 3.0]);
        assert_eq!(point.metadata, meta);
        assert_eq!(point.dimension(), 3);
        assert!(point.created_at > 0);
    }

    #[test]
    fn point_rejects_empty_id() {
        let result = Point::new("", vec![1.0], serde_json::Value::Null);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("empty"),
            "error should mention empty id"
        );
    }

    #[test]
    fn point_rejects_empty_vector() {
        let result = Point::new("p1", vec![], serde_json::Value::Null);
        assert!(result.is_err());
    }

    #[test]
    fn point_rejects_nan_in_vector() {
        let result = Point::new("p1", vec![1.0, f32::NAN, 3.0], serde_json::Value::Null);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("non-finite"));
    }

    #[test]
    fn point_rejects_infinity_in_vector() {
        let result = Point::new("p1", vec![f32::INFINITY], serde_json::Value::Null);
        assert!(result.is_err());
    }

    #[test]
    fn point_serialization_roundtrip() {
        let meta = serde_json::json!({"tags": ["a", "b"]});
        let point = Point::new("abc-123", vec![0.1, 0.2], meta).unwrap();

        let json = serde_json::to_string(&point).unwrap();
        let restored: Point = serde_json::from_str(&json).unwrap();

        assert_eq!(point.id, restored.id);
        assert_eq!(point.vector, restored.vector);
        assert_eq!(point.metadata, restored.metadata);
        assert_eq!(point.created_at, restored.created_at);
    }

    #[test]
    fn point_unique_timestamps_or_same_second() {
        let p1 = Point::new("a", vec![1.0], serde_json::Value::Null).unwrap();
        let p2 = Point::new("b", vec![2.0], serde_json::Value::Null).unwrap();
        // Criados no mesmo segundo ou consecutivos
        assert!(p2.created_at >= p1.created_at);
    }
}
