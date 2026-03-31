//! Wrapper VectorStore para orquestração RAG (LangChain, LlamaIndex, etc.).
//!
//! Implementa a interface esperada por ferramentas de orquestração de busca:
//! adicionar vetores com metadados e consultar por similaridade.

use crate::{FerresDbClient, PointInput, SdkError, SearchResultItem};
use async_trait::async_trait;

/// Documento retornado pelo VectorStore (id + metadata, ex. texto para RAG).
#[derive(Debug, Clone)]
pub struct VectorStoreDoc {
    pub id: String,
    pub metadata: serde_json::Value,
}

impl From<SearchResultItem> for VectorStoreDoc {
    fn from(item: SearchResultItem) -> Self {
        VectorStoreDoc {
            id: item.id,
            metadata: item.metadata,
        }
    }
}

/// Interface de VectorStore para integração com frameworks RAG.
///
/// Segue o contrato esperado por orquestradores de busca (LangChain, LlamaIndex):
/// ingestão de vetores com metadados e consulta por similaridade.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Adiciona documentos (vetores + metadados) ao store.
    /// Os IDs devem ser únicos; repetidos sobrescrevem.
    async fn add_vectors(
        &self,
        ids: &[String],
        vectors: &[Vec<f32>],
        metadatas: Option<&[serde_json::Value]>,
    ) -> Result<u32, SdkError>;

    /// Busca os `k` documentos mais similares ao vetor de consulta.
    /// Retorna apenas os documentos (sem score).
    async fn similarity_search(
        &self,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<VectorStoreDoc>, SdkError>;

    /// Busca os `k` documentos mais similares ao vetor de consulta,
    /// retornando (documento, score).
    async fn similarity_search_with_score(
        &self,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<(VectorStoreDoc, f32)>, SdkError>;
}

/// Implementação do VectorStore usando FerresDB como backend.
///
/// Use com orquestradores RAG: crie a coleção (ou use existente), adicione
/// vetores via `add_vectors` e consulte com `similarity_search` /
/// `similarity_search_with_score`.
#[derive(Debug, Clone)]
pub struct FerresDbVectorStore {
    client: FerresDbClient,
    collection_name: String,
}

impl FerresDbVectorStore {
    /// Cria um VectorStore apontando para a coleção indicada.
    /// A coleção deve existir e ter a mesma dimensão dos vetores que você inserir.
    pub fn new(client: FerresDbClient, collection_name: impl Into<String>) -> Self {
        Self {
            client,
            collection_name: collection_name.into(),
        }
    }

    /// Cria o VectorStore e a coleção se não existir.
    /// `dimension` e `distance` são usados apenas na criação.
    pub async fn ensure_collection(
        client: FerresDbClient,
        collection_name: impl Into<String>,
        dimension: u32,
        distance: &str,
    ) -> Result<Self, SdkError> {
        let name = collection_name.into();
        // Tenta criar; se 409 (already exists), ignora
        let res = client
            .create_collection(&name, dimension, distance, false)
            .await;
        match res {
            Ok(_) => {}
            Err(SdkError::Api { status, .. }) if status.as_u16() == 409 => {}
            Err(e) => return Err(e),
        }
        Ok(Self::new(client, name))
    }
}

#[async_trait]
impl VectorStore for FerresDbVectorStore {
    async fn add_vectors(
        &self,
        ids: &[String],
        vectors: &[Vec<f32>],
        metadatas: Option<&[serde_json::Value]>,
    ) -> Result<u32, SdkError> {
        let points: Vec<PointInput> = ids
            .iter()
            .zip(vectors.iter())
            .enumerate()
            .map(|(i, (id, vector))| PointInput {
                id: id.clone(),
                vector: vector.clone(),
                metadata: metadatas.and_then(|m| m.get(i).cloned()),
            })
            .collect();
        let resp = self
            .client
            .upsert_points(&self.collection_name, &points)
            .await?;
        Ok(resp.upserted)
    }

    async fn similarity_search(
        &self,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<VectorStoreDoc>, SdkError> {
        let resp = self
            .client
            .search(&self.collection_name, query_vector, k, None)
            .await?;
        Ok(resp.results.into_iter().map(VectorStoreDoc::from).collect())
    }

    async fn similarity_search_with_score(
        &self,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<(VectorStoreDoc, f32)>, SdkError> {
        let resp = self
            .client
            .search(&self.collection_name, query_vector, k, None)
            .await?;
        Ok(resp
            .results
            .into_iter()
            .map(|item| {
                let score = item.score;
                (VectorStoreDoc::from(item), score)
            })
            .collect())
    }
}
