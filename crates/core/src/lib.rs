//! # FerresDB Core
//!
//! Motor de busca vetorial escrito em Rust. Este crate contém a lógica
//! central do FerresDB:
//!
//! - **[`point`]** — estrutura `Point` (id String + vetor f32 + metadata JSON + timestamp)
//! - **[`collection`]** — `Collection` com `Box<dyn ANNIndex>` e validação de dimensão
//! - **[`search`]** — trait `ANNIndex` + implementação HNSW com 3 métricas de distância
//! - **[`storage`]** — persistência em disco (JSON-lines)
//! - **[`error`]** — tipos de erro com `thiserror`
//! - **[`VectorDB`]** — API principal de alto nível para gerenciar coleções e buscas
//!
//! ## Exemplo rápido
//!
//! ```rust,no_run
//! use ferres_db_core::{VectorDB, CollectionConfig, DistanceMetric, Point};
//!
//! let mut db = VectorDB::new("./data".into()).unwrap();
//!
//! let config = CollectionConfig {
//!     name: "embeddings".into(),
//!     dimension: 384,
//!     distance: DistanceMetric::Cosine,
//!     hnsw: Default::default(),
//!     search_cache_size: 0,
//! };
//!
//! db.create_collection(config).unwrap();
//! let point = Point::new("doc-1", vec![0.0; 384], serde_json::json!(null)).unwrap();
//! db.upsert_points("embeddings", vec![point]).unwrap();
//!
//! let results = db.search("embeddings", vec![0.0; 384], 5).unwrap();
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

pub mod collection;
pub mod error;
pub mod point;
pub mod search;
pub mod storage;
pub mod wal;

// Re-exporta os tipos mais usados na raiz do crate para ergonomia.
pub use collection::{Collection, CollectionConfig};
pub use error::FerresError;
pub use point::Point;
pub use search::{ANNIndex, DistanceMetric, HnswConfig, HnswIndex};
pub use storage::{CollectionMeta, DiskStorage, FileStorage};
pub use wal::{Wal, WalEntry, WalOperation, recover_collection};

// MetadataFilter e SearchResult já são públicos e definidos neste módulo

// ─── SearchResult ────────────────────────────────────────────────────

/// Resultado de uma busca vetorial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// ID do ponto encontrado.
    pub id: String,
    /// Score de similaridade (menor = mais similar para distâncias).
    pub score: f32,
    /// Metadados do ponto.
    pub metadata: serde_json::Value,
    /// Vetor do ponto (opcional, pode ser omitido para economizar espaço).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

// ─── MetadataFilter ────────────────────────────────────────────────────

/// Filtro de metadata para buscas vetoriais.
///
/// Suporta apenas equality checks (v1). Múltiplos campos são combinados
/// com lógica AND (todos os campos devem corresponder).
///
/// # Exemplo
///
/// ```rust,no_run
/// use ferres_db_core::MetadataFilter;
/// use serde_json::json;
///
/// // Filtra pontos onde category == "tech" AND status == "active"
/// let filter = MetadataFilter::from_json(json!({
///     "category": "tech",
///     "status": "active"
/// })).unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFilter {
    /// Mapa de campo -> valor esperado (equality).
    /// Campos ausentes ou com valores diferentes são filtrados.
    conditions: std::collections::HashMap<String, serde_json::Value>,
}

impl MetadataFilter {
    /// Cria um filtro a partir de um objeto JSON.
    ///
    /// O JSON deve ser um objeto onde cada chave é um campo de metadata
    /// e o valor é o valor esperado (equality check).
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::MetadataFilter;
    /// use serde_json::json;
    ///
    /// let filter = MetadataFilter::from_json(json!({
    ///     "category": "tech"
    /// })).unwrap();
    /// ```
    pub fn from_json(value: serde_json::Value) -> Result<Self, FerresError> {
        match value {
            serde_json::Value::Object(map) => {
                let conditions: std::collections::HashMap<String, serde_json::Value> = map
                    .into_iter()
                    .map(|(k, v)| (k, v))
                    .collect();
                Ok(Self { conditions })
            }
            serde_json::Value::Null => {
                // Filtro vazio = sem filtro
                Ok(Self {
                    conditions: std::collections::HashMap::new(),
                })
            }
            _ => Err(FerresError::InvalidVector {
                reason: "filter must be a JSON object or null".to_string(),
            }),
        }
    }

    /// Cria um filtro vazio (sem condições, não filtra nada).
    pub fn empty() -> Self {
        Self {
            conditions: std::collections::HashMap::new(),
        }
    }

    /// Verifica se o filtro está vazio (não filtra nada).
    pub fn is_empty(&self) -> bool {
        self.conditions.is_empty()
    }

    /// Verifica se um ponto passa no filtro.
    ///
    /// Retorna `true` se o ponto atende a todas as condições (AND lógico).
    /// Campos ausentes no metadata são tratados como não correspondentes.
    fn matches(&self, metadata: &serde_json::Value) -> bool {
        if self.is_empty() {
            return true;
        }

        // Todos os campos do filtro devem corresponder
        self.conditions.iter().all(|(key, expected_value)| {
            match metadata.get(key) {
                Some(actual_value) => actual_value == expected_value,
                None => false, // Campo não existe = não corresponde
            }
        })
    }
}

// ─── CollectionStats ───────────────────────────────────────────────────

/// Estatísticas de uma coleção.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStats {
    /// Número de pontos na coleção.
    pub num_points: usize,
    /// Tamanho estimado do índice em bytes.
    pub index_size_bytes: usize,
    /// Timestamp Unix da última atualização (segundos).
    pub last_updated: u64,
}

// ─── CollectionInfo ──────────────────────────────────────────────────────

/// Informações básicas de uma coleção para listagem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionInfo {
    /// Nome da coleção.
    pub name: String,
    /// Dimensão dos vetores na coleção.
    pub dimension: usize,
    /// Número de pontos na coleção.
    pub num_points: usize,
    /// Timestamp Unix de criação (segundos). Usa o timestamp do ponto mais antigo.
    pub created_at: u64,
}

// ─── VectorDB ──────────────────────────────────────────────────────────

/// API principal de alto nível para gerenciar coleções e realizar buscas vetoriais.
///
/// Gerencia múltiplas coleções em memória e persiste automaticamente
/// modificações em disco após cada operação de escrita.
pub struct VectorDB {
    collections: HashMap<String, Collection>,
    wals: HashMap<String, wal::Wal>,
    storage_path: PathBuf,
}

impl VectorDB {
    /// Cria uma nova instância do VectorDB e carrega coleções existentes do disco.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::VectorDB;
    ///
    /// let db = VectorDB::new("./data".into())?;
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Erros
    /// - Retorna erro se o diretório não puder ser criado ou acessado.
    pub fn new(storage_path: PathBuf) -> Result<Self, FerresError> {
        info!(path = %storage_path.display(), "initializing VectorDB");
        
        // Cria o diretório se não existir
        std::fs::create_dir_all(&storage_path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to create storage directory {}: {e}",
                storage_path.display()
            ))
        })?;

        let mut db = Self {
            collections: HashMap::new(),
            wals: HashMap::new(),
            storage_path,
        };

        // Carrega coleções existentes do disco
        db.load_collections_from_disk()?;

        info!(
            collections = db.collections.len(),
            "VectorDB initialized"
        );

        Ok(db)
    }

    /// Cria uma nova coleção.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{VectorDB, CollectionConfig, DistanceMetric};
    ///
    /// let mut db = VectorDB::new("./data".into())?;
    ///
    /// let config = CollectionConfig {
    ///     name: "embeddings".into(),
    ///     dimension: 384,
    ///     distance: DistanceMetric::Cosine,
    ///     hnsw: Default::default(),
    ///     search_cache_size: 100,
    /// };
    ///
    /// db.create_collection(config)?;
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Validações
    /// - Verifica se a coleção já existe.
    /// - Valida a configuração (dimensão > 0, etc).
    ///
    /// # Erros
    /// - `CollectionAlreadyExists` se uma coleção com o mesmo nome já existir.
    pub fn create_collection(&mut self, config: CollectionConfig) -> Result<(), FerresError> {
        if self.collections.contains_key(&config.name) {
            warn!(collection = %config.name, "attempted to create existing collection");
            return Err(FerresError::CollectionAlreadyExists(config.name.clone()));
        }

        if config.dimension == 0 {
            return Err(FerresError::InvalidVector {
                reason: "dimension must be greater than 0".to_string(),
            });
        }

        info!(
            collection = %config.name,
            dimension = config.dimension,
            distance = ?config.distance,
            "creating collection"
        );

        let collection = Collection::new(config.clone());
        self.collections.insert(collection.name().to_string(), collection);

        // Abre WAL para a nova coleção
        let collection_dir = self.storage_path.join("collections").join(&config.name);
        let wal_handle = wal::Wal::open(&collection_dir, wal::Wal::DEFAULT_SNAPSHOT_THRESHOLD)?;
        self.wals.insert(config.name.clone(), wal_handle);

        // Auto-save após criação (snapshot inicial)
        self.save_collection(&config.name)?;

        info!(collection = %config.name, "collection created");
        Ok(())
    }

    /// Remove uma coleção e seus dados do disco.
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    pub fn delete_collection(&mut self, name: &str) -> Result<(), FerresError> {
        if !self.collections.contains_key(name) {
            warn!(collection = %name, "attempted to delete non-existent collection");
            return Err(FerresError::CollectionNotFound(name.to_string()));
        }

        info!(collection = %name, "deleting collection");

        // Remove do disco
        let collection_dir = self.storage_path.join("collections").join(name);
        if collection_dir.exists() {
            std::fs::remove_dir_all(&collection_dir).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to delete collection directory {}: {e}",
                    collection_dir.display()
                ))
            })?;
        }

        // Remove da memória
        self.collections.remove(name);
        self.wals.remove(name);

        info!(collection = %name, "collection deleted");
        Ok(())
    }

    /// Insere ou atualiza pontos em uma coleção.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{VectorDB, Point};
    ///
    /// let mut db = VectorDB::new("./data".into())?;
    ///
    /// let points = vec![
    ///     Point::new("doc-1", vec![0.1; 384], serde_json::json!({"text": "Hello"}))?,
    ///     Point::new("doc-2", vec![0.2; 384], serde_json::json!({"text": "World"}))?,
    /// ];
    ///
    /// db.upsert_points("embeddings", points)?;
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    /// - Valida a dimensão de cada ponto.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    /// - `DimensionMismatch` se algum ponto tiver dimensão incorreta.
    pub fn upsert_points(
        &mut self,
        collection: &str,
        points: Vec<Point>,
    ) -> Result<(), FerresError> {
        // Fase 1: valida dimensões (borrow imutável de collections)
        let prepared_points = {
            let col = self
                .collections
                .get(collection)
                .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

            if points.len() > 100 {
                use rayon::prelude::*;

                let validation_errors: Vec<_> = points
                    .par_iter()
                    .filter_map(|point| col.validate_dimension(&point.vector).err())
                    .collect();

                if !validation_errors.is_empty() {
                    return Err(validation_errors.into_iter().next().unwrap());
                }

                // Para métrica Cosine, normaliza vetores em paralelo
                let config = col.config();
                if config.distance == crate::search::DistanceMetric::Cosine {
                    use crate::search::normalize_vectors_parallel;

                    let vectors: Vec<Vec<f32>> = points.iter().map(|p| p.vector.clone()).collect();
                    let normalized_vectors = normalize_vectors_parallel(&vectors);

                    points
                        .into_iter()
                        .zip(normalized_vectors)
                        .map(|(mut p, nv)| {
                            p.vector = nv;
                            p
                        })
                        .collect::<Vec<_>>()
                } else {
                    points
                }
            } else {
                for point in &points {
                    col.validate_dimension(&point.vector)?;
                }
                points
            }
        };

        info!(
            collection = %collection,
            points = prepared_points.len(),
            "upserting points"
        );

        // Fase 2: WAL — registra todas as operações ANTES da mutação
        if let Some(wal) = self.wals.get_mut(collection) {
            for point in &prepared_points {
                wal.append_upsert(point)?;
            }
        }

        // Fase 3: insere pontos em memória
        let col = self
            .collections
            .get_mut(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        for point in prepared_points {
            if let Err(e) = col.insert(point) {
                error!(
                    collection = %collection,
                    error = %e,
                    "failed to insert point"
                );
                return Err(e);
            }
        }
        let total_points = col.len();

        // Fase 4: snapshot se threshold atingido
        let should_snapshot = self
            .wals
            .get(collection)
            .map_or(false, |w| w.should_snapshot());
        if should_snapshot {
            self.save_collection(collection)?;
            if let Some(wal) = self.wals.get_mut(collection) {
                wal.truncate_after_snapshot()?;
            }
        }

        info!(
            collection = %collection,
            total_points,
            "points upserted"
        );

        Ok(())
    }

    /// Remove pontos de uma coleção pelos IDs.
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    /// - `PointNotFound` se algum ID não existir (após tentar remover todos).
    pub fn delete_points(
        &mut self,
        collection: &str,
        ids: Vec<String>,
    ) -> Result<(), FerresError> {
        if !self.collections.contains_key(collection) {
            return Err(FerresError::CollectionNotFound(collection.to_string()));
        }

        info!(
            collection = %collection,
            ids = ids.len(),
            "deleting points"
        );

        // WAL: registra deletes ANTES da mutação
        if let Some(wal) = self.wals.get_mut(collection) {
            for id in &ids {
                wal.append_delete(id)?;
            }
        }

        let col = self
            .collections
            .get_mut(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        let (deleted_count, total_points) = {
            let mut not_found = Vec::new();
            for id in &ids {
                if let Err(e) = col.remove(id) {
                    if matches!(e, FerresError::PointNotFound(_)) {
                        not_found.push(id.clone());
                    } else {
                        return Err(e);
                    }
                }
            }

            if !not_found.is_empty() {
                warn!(
                    collection = %collection,
                    not_found = ?not_found,
                    "some points were not found during deletion"
                );
                if not_found.len() == ids.len() {
                    return Err(FerresError::PointNotFound(not_found.join(", ")));
                }
            }

            (ids.len() - not_found.len(), col.len())
        };

        // Snapshot se threshold atingido
        let should_snapshot = self
            .wals
            .get(collection)
            .map_or(false, |w| w.should_snapshot());
        if should_snapshot {
            self.save_collection(collection)?;
            if let Some(wal) = self.wals.get_mut(collection) {
                wal.truncate_after_snapshot()?;
            }
        }

        info!(
            collection = %collection,
            deleted = deleted_count,
            total_points,
            "points deleted"
        );

        Ok(())
    }

    /// Busca os `limit` pontos mais similares ao vetor de consulta.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::VectorDB;
    ///
    /// let db = VectorDB::new("./data".into())?;
    ///
    /// let query_vector = vec![0.15; 384];
    /// let results = db.search("embeddings", query_vector, 5)?;
    ///
    /// for result in results {
    ///     println!("ID: {}, Score: {:.4}", result.id, result.score);
    /// }
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    /// - Valida a dimensão do vetor de consulta.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    /// - `DimensionMismatch` se o vetor de consulta tiver dimensão incorreta.
    pub fn search(
        &self,
        collection: &str,
        query: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, FerresError> {
        let col = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        // Valida dimensão do query
        col.validate_dimension(&query)?;

        info!(
            collection = %collection,
            limit,
            "performing search"
        );

        // Realiza a busca
        let results = col.search(&query, limit)?;

        // Constrói SearchResults com metadados e vetores opcionais
        let search_results: Vec<SearchResult> = results
            .into_iter()
            .filter_map(|(id, score)| {
                let point = col.get(&id)?;
                Some(SearchResult {
                    id,
                    score,
                    metadata: point.metadata.clone(),
                    vector: None, // Por padrão não inclui o vetor para economizar espaço
                })
            })
            .collect();

        info!(
            collection = %collection,
            results = search_results.len(),
            "search completed"
        );

        Ok(search_results)
    }

    /// Busca os `limit` pontos mais similares ao vetor de consulta com filtro de metadata.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{VectorDB, MetadataFilter};
    /// use serde_json::json;
    ///
    /// let db = VectorDB::new("./data".into())?;
    ///
    /// let query_vector = vec![0.15; 384];
    /// let filter = MetadataFilter::from_json(json!({
    ///     "category": "tech"
    /// }))?;
    ///
    /// let results = db.search_with_filter("embeddings", query_vector, 5, Some(filter))?;
    ///
    /// for result in results {
    ///     println!("ID: {}, Score: {:.4}", result.id, result.score);
    /// }
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Estratégia de Implementação
    ///
    /// 1. Busca `limit * 10` resultados do índice ANN para ter candidatos suficientes
    /// 2. Aplica o filtro de metadata em memória sobre os candidatos
    /// 3. Retorna apenas os top-`limit` resultados que passam no filtro
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    /// - Valida a dimensão do vetor de consulta.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    /// - `DimensionMismatch` se o vetor de consulta tiver dimensão incorreta.
    pub fn search_with_filter(
        &self,
        collection: &str,
        query: Vec<f32>,
        limit: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<SearchResult>, FerresError> {
        let col = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        // Valida dimensão do query
        col.validate_dimension(&query)?;

        // Se o filtro está vazio ou None, usa busca normal
        let filter = filter.unwrap_or_else(MetadataFilter::empty);
        if filter.is_empty() {
            return self.search(collection, query, limit);
        }

        info!(
            collection = %collection,
            limit,
            filter_conditions = filter.conditions.len(),
            "performing filtered search"
        );

        // Busca mais resultados para ter candidatos suficientes após filtro
        // Multiplica por 10 para aumentar chances de ter `limit` resultados após filtro
        let search_limit = limit.saturating_mul(10);
        // Limita ao número máximo de pontos na coleção
        let max_points = col.len();
        let search_limit = search_limit.min(max_points.max(limit));

        // Realiza a busca ampliada
        let results = col.search(&query, search_limit)?;

        // Constrói SearchResults e aplica filtro
        let filtered_results: Vec<SearchResult> = results
            .into_iter()
            .filter_map(|(id, score)| {
                let point = col.get(&id)?;
                
                // Aplica filtro de metadata
                if !filter.matches(&point.metadata) {
                    return None;
                }

                Some(SearchResult {
                    id,
                    score,
                    metadata: point.metadata.clone(),
                    vector: None,
                })
            })
            .take(limit) // Limita aos top-k após filtro
            .collect();

        info!(
            collection = %collection,
            requested_limit = limit,
            filtered_results = filtered_results.len(),
            "filtered search completed"
        );

        Ok(filtered_results)
    }

    /// Retorna um ponto específico de uma coleção pelo ID.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::VectorDB;
    ///
    /// let db = VectorDB::new("./data".into())?;
    ///
    /// let point = db.get_point("embeddings", "doc-1")?;
    /// println!("ID: {}, Vector dim: {}", point.id, point.vector.len());
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    /// - Verifica se o ponto existe.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    /// - `PointNotFound` se o ponto não existir.
    pub fn get_point(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Point, FerresError> {
        let col = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        col.get(id)
            .ok_or_else(|| FerresError::PointNotFound(id.to_string()))
            .map(|p| p.clone())
    }

    /// Retorna estatísticas de uma coleção.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::VectorDB;
    ///
    /// let db = VectorDB::new("./data".into())?;
    ///
    /// let stats = db.get_collection_stats("embeddings")?;
    /// println!("Pontos: {}, Tamanho índice: {} bytes", 
    ///          stats.num_points, stats.index_size_bytes);
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Validações
    /// - Verifica se a coleção existe.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    pub fn get_collection_stats(
        &self,
        collection: &str,
    ) -> Result<CollectionStats, FerresError> {
        let col = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        let num_points = col.len();
        
        // Estima o tamanho do índice em bytes
        // Aproximação: cada ponto tem um vetor de f32 (4 bytes) + overhead do HNSW
        // HNSW tem overhead de ~M * num_points * (ponteiros + distâncias)
        let config = col.config();
        let vector_size = num_points * config.dimension * 4; // f32 = 4 bytes
        let hnsw_overhead = num_points * config.hnsw.max_nb_connection * 8; // ponteiros + distâncias
        let index_size_bytes = vector_size + hnsw_overhead;

        // Última atualização: pega o timestamp mais recente dos pontos
        let last_updated = col
            .points_owned()
            .iter()
            .map(|p| p.created_at)
            .max()
            .unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            });

        Ok(CollectionStats {
            num_points,
            index_size_bytes,
            last_updated,
        })
    }

    /// Lista todas as coleções com informações básicas.
    ///
    /// Retorna um vetor com informações resumidas de cada coleção.
    pub fn list_collections(&self) -> Vec<CollectionInfo> {
        self.collections
            .iter()
            .map(|(name, col)| {
                let config = col.config();
                let num_points = col.len();
                
                // Usa o timestamp do ponto mais antigo como created_at
                // Se a coleção estiver vazia, usa 0
                let created_at = col
                    .points_owned()
                    .iter()
                    .map(|p| p.created_at)
                    .min()
                    .unwrap_or(0);

                CollectionInfo {
                    name: name.clone(),
                    dimension: config.dimension,
                    num_points,
                    created_at,
                }
            })
            .collect()
    }

    // ─── Métodos privados ──────────────────────────────────────────────

    /// Carrega todas as coleções persistidas do disco.
    fn load_collections_from_disk(&mut self) -> Result<(), FerresError> {
        let collections_dir = self.storage_path.join("collections");
        
        // Cria o diretório collections se não existir
        if !collections_dir.exists() {
            std::fs::create_dir_all(&collections_dir).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to create collections directory {}: {e}",
                    collections_dir.display()
                ))
            })?;
        }
        
        let collection_dirs = std::fs::read_dir(&collections_dir).map_err(|e| {
            FerresError::Storage(format!(
                "failed to read collections directory {}: {e}",
                collections_dir.display()
            ))
        })?;

        for entry in collection_dirs {
            let entry = entry.map_err(|e| {
                FerresError::Storage(format!("failed to read directory entry: {e}"))
            })?;
            let path = entry.path();

            if path.is_dir() {
                match wal::recover_collection(&path) {
                    Ok(Some(collection)) => {
                        let name = collection.name().to_string();

                        // Abre WAL para esta coleção
                        let mut wal_handle = wal::Wal::open(
                            &path,
                            wal::Wal::DEFAULT_SNAPSHOT_THRESHOLD,
                        )?;

                        // Se o WAL tinha entradas, consolida com snapshot + truncate
                        if wal_handle.ops_since_snapshot() > 0 {
                            FileStorage::save_collection(&collection, &path)?;
                            wal_handle.truncate_after_snapshot()?;
                            info!(collection = %name, "post-recovery snapshot created");
                        }

                        info!(
                            collection = %name,
                            points = collection.len(),
                            "loaded collection from disk"
                        );
                        self.collections.insert(name.clone(), collection);
                        self.wals.insert(name, wal_handle);
                    }
                    Ok(None) => {
                        // Não é um diretório de coleção válido, ignora
                    }
                    Err(e) => {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to load collection, skipping"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Salva uma coleção no disco.
    fn save_collection(&self, name: &str) -> Result<(), FerresError> {
        let col = self
            .collections
            .get(name)
            .ok_or_else(|| FerresError::CollectionNotFound(name.to_string()))?;

        let collection_dir = self.storage_path.join("collections").join(name);
        FileStorage::save_collection(col, &collection_dir)?;

        Ok(())
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn create_test_db() -> (VectorDB, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db = VectorDB::new(temp_dir.path().to_path_buf()).unwrap();
        (db, temp_dir)
    }

    fn create_test_collection(db: &mut VectorDB, name: &str, dimension: usize) {
        let config = CollectionConfig {
            name: name.to_string(),
            dimension,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
        };
        db.create_collection(config).unwrap();
    }

    #[test]
    fn test_metadata_filter_from_json() {
        let filter = MetadataFilter::from_json(json!({
            "category": "tech",
            "status": "active"
        }))
        .unwrap();

        assert_eq!(filter.conditions.len(), 2);
        assert_eq!(
            filter.conditions.get("category"),
            Some(&json!("tech"))
        );
        assert_eq!(
            filter.conditions.get("status"),
            Some(&json!("active"))
        );
    }

    #[test]
    fn test_metadata_filter_empty() {
        let filter = MetadataFilter::from_json(json!(null)).unwrap();
        assert!(filter.is_empty());

        let filter2 = MetadataFilter::empty();
        assert!(filter2.is_empty());
    }

    #[test]
    fn test_metadata_filter_matches() {
        let filter = MetadataFilter::from_json(json!({
            "category": "tech"
        }))
        .unwrap();

        // Match: campo existe e valor corresponde
        let metadata1 = json!({"category": "tech", "other": "value"});
        assert!(filter.matches(&metadata1));

        // No match: campo não existe
        let metadata2 = json!({"other": "value"});
        assert!(!filter.matches(&metadata2));

        // No match: valor diferente
        let metadata3 = json!({"category": "science"});
        assert!(!filter.matches(&metadata3));

        // Match: múltiplos campos (AND lógico)
        let filter2 = MetadataFilter::from_json(json!({
            "category": "tech",
            "status": "active"
        }))
        .unwrap();
        let metadata4 = json!({"category": "tech", "status": "active"});
        assert!(filter2.matches(&metadata4));

        // No match: um campo não corresponde
        let metadata5 = json!({"category": "tech", "status": "inactive"});
        assert!(!filter2.matches(&metadata5));
    }

    #[test]
    fn test_search_with_filter_basic() {
        let (mut db, _temp_dir) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        // Insere pontos com diferentes metadados
        let points = vec![
            Point::new(
                "p1",
                vec![1.0, 0.0, 0.0],
                json!({"category": "tech", "status": "active"}),
            )
            .unwrap(),
            Point::new(
                "p2",
                vec![0.0, 1.0, 0.0],
                json!({"category": "science", "status": "active"}),
            )
            .unwrap(),
            Point::new(
                "p3",
                vec![0.0, 0.0, 1.0],
                json!({"category": "tech", "status": "inactive"}),
            )
            .unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Busca sem filtro
        let results = db.search("test", vec![1.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(results.len(), 3);

        // Busca com filtro que corresponde a 1 ponto
        let filter = MetadataFilter::from_json(json!({"category": "tech", "status": "active"})).unwrap();
        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();
        assert_eq!(filtered_results.len(), 1);
        assert_eq!(filtered_results[0].id, "p1");
    }

    #[test]
    fn test_search_with_filter_empty_filter() {
        let (mut db, _temp_dir) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        let points = vec![
            Point::new("p1", vec![1.0, 0.0, 0.0], json!({"category": "tech"})).unwrap(),
            Point::new("p2", vec![0.0, 1.0, 0.0], json!({"category": "science"})).unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Filtro vazio deve retornar todos os resultados (igual a search normal)
        let filter = MetadataFilter::empty();
        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();
        assert_eq!(filtered_results.len(), 2);

        // None também deve funcionar como filtro vazio
        let filtered_results2 = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, None)
            .unwrap();
        assert_eq!(filtered_results2.len(), 2);
    }

    #[test]
    fn test_search_with_filter_field_not_exists() {
        let (mut db, _temp_dir) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        // Ponto sem o campo "category"
        let points = vec![
            Point::new("p1", vec![1.0, 0.0, 0.0], json!({"other": "value"})).unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Filtro que requer campo inexistente
        let filter = MetadataFilter::from_json(json!({"category": "tech"})).unwrap();
        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();

        // Não deve retornar resultados porque o campo não existe
        assert_eq!(filtered_results.len(), 0);
    }

    #[test]
    fn test_search_with_filter_insufficient_results() {
        let (mut db, _temp_dir) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        // Insere apenas 2 pontos que correspondem ao filtro
        let points = vec![
            Point::new(
                "p1",
                vec![1.0, 0.0, 0.0],
                json!({"category": "tech"}),
            )
            .unwrap(),
            Point::new(
                "p2",
                vec![0.0, 1.0, 0.0],
                json!({"category": "tech"}),
            )
            .unwrap(),
            // Ponto que não corresponde ao filtro
            Point::new(
                "p3",
                vec![0.0, 0.0, 1.0],
                json!({"category": "science"}),
            )
            .unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Busca com limit maior que o número de resultados filtrados
        let filter = MetadataFilter::from_json(json!({"category": "tech"})).unwrap();
        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();

        // Deve retornar apenas os 2 pontos que correspondem ao filtro
        assert_eq!(filtered_results.len(), 2);
        assert!(filtered_results.iter().all(|r| r.id == "p1" || r.id == "p2"));
    }

    #[test]
    fn test_search_with_filter_multiple_conditions_and() {
        let (mut db, _temp_dir) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        let points = vec![
            Point::new(
                "p1",
                vec![1.0, 0.0, 0.0],
                json!({"category": "tech", "status": "active", "priority": "high"}),
            )
            .unwrap(),
            Point::new(
                "p2",
                vec![0.0, 1.0, 0.0],
                json!({"category": "tech", "status": "active", "priority": "low"}),
            )
            .unwrap(),
            Point::new(
                "p3",
                vec![0.0, 0.0, 1.0],
                json!({"category": "tech", "status": "inactive"}),
            )
            .unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Filtro com múltiplas condições (AND)
        let filter = MetadataFilter::from_json(json!({
            "category": "tech",
            "status": "active"
        }))
        .unwrap();

        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();

        // Deve retornar apenas p1 e p2 (ambos têm category=tech E status=active)
        assert_eq!(filtered_results.len(), 2);
        assert!(filtered_results.iter().all(|r| r.id == "p1" || r.id == "p2"));
        // p3 não deve aparecer porque status != "active"
    }

    #[test]
    fn test_search_with_filter_invalid_json() {
        // Filtro deve ser objeto ou null
        let result = MetadataFilter::from_json(json!("invalid"));
        assert!(result.is_err());

        let result2 = MetadataFilter::from_json(json!([]));
        assert!(result2.is_err());
    }
}
