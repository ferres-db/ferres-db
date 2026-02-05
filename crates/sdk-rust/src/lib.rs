//! # FerresDB SDK
//!
//! SDK Rust para interação com o FerresDB.
//! Fornece um client type-safe para operações CRUD em coleções, busca vetorial e busca híbrida.

// Re-exporta tipos do core para que consumidores do SDK
// não precisem depender diretamente do crate core.
pub use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, Point};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Client ─────────────────────────────────────────────────────────────

/// Cliente HTTP para o FerresDB.
///
/// Conecta a um servidor FerresDB via REST e expõe métodos para busca vetorial e híbrida.
#[derive(Debug, Clone)]
pub struct FerresDbClient {
    base_url: String,
    client: reqwest::Client,
}

impl FerresDbClient {
    /// Cria um cliente apontando para a URL base do servidor (ex: `http://localhost:8080`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Busca híbrida: combina resultados vetoriais e BM25 (keyword) via RRF.
    ///
    /// A coleção deve ter sido criada com `enable_bm25: true`.
    /// `alpha` em [0, 1]: peso da busca vetorial; (1 - alpha) é o peso da busca keyword.
    pub async fn hybrid_search(
        &self,
        collection_name: &str,
        query_text: &str,
        query_vector: &[f32],
        limit: usize,
        alpha: f32,
    ) -> Result<HybridSearchResponse, SdkError> {
        let url = format!(
            "{}/api/v1/collections/{}/search/hybrid",
            self.base_url, collection_name
        );
        let body = HybridSearchRequest {
            query_text: query_text.to_string(),
            query_vector: query_vector.to_vec(),
            limit,
            alpha,
        };
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(SdkError::Request)?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(SdkError::Api {
                status,
                message: text,
            });
        }
        let data = resp.json().await.map_err(SdkError::Decode)?;
        Ok(data)
    }
}

/// Erros do SDK.
#[derive(Debug, Error)]
pub enum SdkError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("API error (status {status}): {message}")]
    Api {
        status: reqwest::StatusCode,
        message: String,
    },
    #[error("failed to decode response: {0}")]
    Decode(reqwest::Error),
}

// ─── Request/Response types (espelho do server) ─────────────────────────

#[derive(Debug, Serialize)]
struct HybridSearchRequest {
    query_text: String,
    query_vector: Vec<f32>,
    limit: usize,
    alpha: f32,
}

/// Resposta da busca híbrida (e da busca vetorial no mesmo formato).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResponse {
    pub results: Vec<SearchResultItem>,
    pub took_ms: u64,
}

/// Um resultado de busca (id, score, metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultItem {
    pub id: String,
    pub score: f32,
    pub metadata: serde_json::Value,
}
