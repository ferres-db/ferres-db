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

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::FerresError;

/// Representa um ponto (vetor + metadados) no espaço vetorial.
///
/// O campo `vector` contém o vetor principal (obrigatório) para busca por
/// similaridade. O campo opcional `vectors` permite múltiplos vetores
/// nomeados por documento (ex.: `title_vector`, `content_vector`), usados
/// em buscas contra um campo específico. `metadata` carrega contexto
/// arbitrário em JSON. O campo opcional `namespace` permite isolamento
/// lógico por tenant (multitenancy) na mesma coleção.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    pub created_at: u64,
    /// Namespace lógico para multitenancy; quando presente, a chave única é (namespace, id).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Timestamp Unix (segundos) em que o ponto expira; None = sem expiração (TTL).
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// Vetores nomeados adicionais (ex.: "title_vector", "content_vector").
    /// Cada vetor deve ter a mesma dimensão da coleção. Busca pode ser feita
    /// contra o vetor principal (`vector`) ou contra um destes campos.
    #[serde(default)]
    pub vectors: Option<HashMap<String, Vec<f32>>>,
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
            namespace: None,
            expires_at: None,
            vectors: None,
        })
    }

    /// Retorna o vetor a usar para um campo vetorial dado.
    ///
    /// - `None` ou `"default"`: vetor principal (`vector`).
    /// - Outro nome: entrada em `vectors` com essa chave, se existir.
    #[inline]
    pub fn vector_for_field(&self, field: Option<&str>) -> Option<&[f32]> {
        match field {
            None | Some("default") => Some(self.vector.as_slice()),
            Some(name) => self
                .vectors
                .as_ref()
                .and_then(|m| m.get(name))
                .map(|v| v.as_slice()),
        }
    }

    /// Chave interna de armazenamento: (namespace, id) quando namespace existe, senão id.
    /// Usada pelo mapa da coleção e pelo índice ANN para identificar pontos de forma única.
    #[inline]
    pub fn storage_id(&self) -> String {
        Self::storage_id_from_parts(self.namespace.as_deref(), &self.id)
    }

    /// Constrói a chave de armazenamento a partir de namespace e id.
    /// Quando `namespace` é `Some`, retorna `"{namespace}\0{id}"` para evitar colisões.
    pub fn storage_id_from_parts(namespace: Option<&str>, id: &str) -> String {
        namespace
            .map(|n| format!("{}\0{}", n, id))
            .unwrap_or_else(|| id.to_string())
    }

    /// Decompõe uma chave de armazenamento em (namespace, id lógico).
    /// Se a chave contém `\0`, retorna `(Some(namespace), id)`; senão `(None, key)`.
    pub fn parse_storage_id(storage_id: &str) -> (Option<String>, String) {
        if let Some(pos) = storage_id.find('\0') {
            let (ns, id) = storage_id.split_at(pos);
            (Some(ns.to_string()), id[1..].to_string())
        } else {
            (None, storage_id.to_string())
        }
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
        assert_eq!(point.namespace, restored.namespace);
        assert_eq!(point.expires_at, restored.expires_at);
    }

    #[test]
    fn point_serialization_roundtrip_with_namespace() {
        let mut point = Point::new("doc-1", vec![0.1, 0.2], serde_json::Value::Null).unwrap();
        point.namespace = Some("tenant-a".into());

        let json = serde_json::to_string(&point).unwrap();
        let restored: Point = serde_json::from_str(&json).unwrap();

        assert_eq!(point.id, restored.id);
        assert_eq!(point.namespace, restored.namespace);
        assert_eq!(restored.namespace.as_deref(), Some("tenant-a"));
    }

    #[test]
    fn point_deserialize_legacy_without_namespace_field() {
        let json = r#"{"id":"legacy","vector":[1.0],"metadata":null,"created_at":1}"#;
        let point: Point = serde_json::from_str(json).unwrap();
        assert_eq!(point.id, "legacy");
        assert_eq!(point.namespace, None);
    }

    #[test]
    fn point_serialization_roundtrip_with_expires_at() {
        let mut point = Point::new("ttl-1", vec![0.1, 0.2], serde_json::Value::Null).unwrap();
        point.expires_at = Some(123);

        let json = serde_json::to_string(&point).unwrap();
        let restored: Point = serde_json::from_str(&json).unwrap();

        assert_eq!(point.id, restored.id);
        assert_eq!(point.expires_at, restored.expires_at);
        assert_eq!(restored.expires_at, Some(123));
    }

    #[test]
    fn point_deserialize_legacy_without_expires_at_field() {
        let json = r#"{"id":"legacy","vector":[1.0],"metadata":null,"created_at":1}"#;
        let point: Point = serde_json::from_str(json).unwrap();
        assert_eq!(point.id, "legacy");
        assert_eq!(point.expires_at, None);
    }

    #[test]
    fn point_deserialize_legacy_without_vectors_field() {
        let json = r#"{"id":"legacy","vector":[1.0],"metadata":null,"created_at":1}"#;
        let point: Point = serde_json::from_str(json).unwrap();
        assert_eq!(point.id, "legacy");
        assert_eq!(point.vectors, None);
        assert_eq!(point.vector_for_field(None).unwrap(), &[1.0_f32]);
    }

    #[test]
    fn point_vector_for_field_default_and_named() {
        let mut vectors = HashMap::new();
        vectors.insert("title_vector".to_string(), vec![2.0, 3.0]);
        vectors.insert("content_vector".to_string(), vec![4.0, 5.0]);
        let point = Point {
            id: "p1".into(),
            vector: vec![1.0, 0.0],
            metadata: serde_json::Value::Null,
            created_at: 0,
            namespace: None,
            expires_at: None,
            vectors: Some(vectors),
        };
        assert_eq!(point.vector_for_field(None).unwrap(), &[1.0_f32, 0.0_f32]);
        assert_eq!(point.vector_for_field(Some("default")).unwrap(), &[1.0_f32, 0.0_f32]);
        assert_eq!(point.vector_for_field(Some("title_vector")).unwrap(), &[2.0_f32, 3.0_f32]);
        assert_eq!(point.vector_for_field(Some("content_vector")).unwrap(), &[4.0_f32, 5.0_f32]);
        assert!(point.vector_for_field(Some("other")).is_none());
    }

    #[test]
    fn point_storage_id_without_namespace() {
        let point = Point::new("p1", vec![1.0], serde_json::Value::Null).unwrap();
        assert_eq!(point.storage_id(), "p1");
        assert_eq!(Point::storage_id_from_parts(None, "p1"), "p1");
    }

    #[test]
    fn point_storage_id_with_namespace() {
        let mut point = Point::new("p1", vec![1.0], serde_json::Value::Null).unwrap();
        point.namespace = Some("tenant".into());
        assert_eq!(point.storage_id(), "tenant\0p1");
        assert_eq!(
            Point::storage_id_from_parts(Some("tenant"), "p1"),
            "tenant\0p1"
        );
    }

    #[test]
    fn point_unique_timestamps_or_same_second() {
        let p1 = Point::new("a", vec![1.0], serde_json::Value::Null).unwrap();
        let p2 = Point::new("b", vec![2.0], serde_json::Value::Null).unwrap();
        // Criados no mesmo segundo ou consecutivos
        assert!(p2.created_at >= p1.created_at);
    }
}
