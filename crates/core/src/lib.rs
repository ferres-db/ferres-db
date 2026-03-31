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
//!     enable_bm25: false,
//!     bm25_text_field: "text".to_string(),
//!     quantization: Default::default(),
//!     tiered_storage: Default::default(),
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

pub mod bm25;
pub mod collection;
pub mod cost;
pub mod error;
pub mod explain;
pub mod fusion;
pub mod graph;
pub mod point;
pub mod quantization;
pub mod reindex;
pub mod rerank;
pub mod search;
pub mod storage;
pub mod tiered;
pub mod wal;

// Re-exporta os tipos mais usados na raiz do crate para ergonomia.
pub use bm25::BM25Index;
pub use collection::{BatchInsertResult, Collection, CollectionConfig};
pub use cost::{estimate_search_cost, CostBreakdown, CostEstimateParams, QueryCostEstimate};
pub use error::FerresError;
pub use explain::{
    build_search_explanation, build_search_explanation_with_resolver, evaluate_condition,
    ConditionResult, ExplainMeta, ExplainResult, FilterExplanation, IndexStats, SearchExplanation,
};
pub use fusion::{reciprocal_rank_fusion, weighted_fusion, FusionStrategy, DEFAULT_RRF_K};
pub use graph::traverse_bfs;
pub use point::Point;
pub use quantization::{QuantizationConfig, ScalarQuantizationConfig, ScalarType};
pub use reindex::{
    apply_delta, build_new_index, estimate_index_size, needs_reindex, tombstone_ratio, ReindexJob,
    ReindexStats, ReindexStatus, AUTO_REINDEX_TOMBSTONE_RATIO,
};
pub use rerank::Reranker;
pub use search::{
    create_ann_index, distance_between, simd_enabled, ANNIndex, DistanceMetric, HnswConfig,
    HnswIndex, QuantizedHnswIndex,
};
pub use storage::{
    CollectionMeta, DiskStorage, FileStorage, StorageCircuitBreaker, StorageOptions,
};
pub use tiered::{
    AccessTracker, ColdStorage, CompactionResult, StorageTier, TierDistribution, TierMetadata,
    TieredCollection, TieredStorageConfig, WarmStorage,
};
pub use wal::{
    compact_wal_entries_older_than, list_restore_points, read_last_snapshot_timestamp,
    recover_collection, recover_collection_to_timestamp, RestorePoints, Wal, WalEntry,
    WalOperation,
};

#[cfg(feature = "rerank")]
pub use rerank::CrossEncoderOrt;

// MetadataFilter e SearchResult já são públicos e definidos neste módulo

// ─── SearchResult ────────────────────────────────────────────────────

/// Resultado de uma busca vetorial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// ID lógico do ponto encontrado.
    pub id: String,
    /// Score de similaridade (menor = mais similar para distâncias).
    pub score: f32,
    /// Metadados do ponto.
    pub metadata: serde_json::Value,
    /// Vetor do ponto (opcional, pode ser omitido para economizar espaço).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
    /// Namespace do ponto, quando presente (multitenancy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

// ─── MetadataCondition (Fase 1: operadores) ────────────────────────────

/// Condição sobre um campo de metadata.
///
/// Suporta igualdade, desigualdade, pertinência em lista e comparações
/// numéricas. Múltiplas condições são combinadas com AND.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataCondition {
    /// Igualdade: campo == valor.
    Eq(String, serde_json::Value),
    /// Desigualdade: campo != valor (campo ausente também satisfaz).
    Ne(String, serde_json::Value),
    /// Pertinência: valor do campo está na lista.
    In(String, Vec<serde_json::Value>),
    /// Maior que (numérico).
    Gt(String, f64),
    /// Menor que (numérico).
    Lt(String, f64),
    /// Maior ou igual (numérico).
    Gte(String, f64),
    /// Menor ou igual (numérico).
    Lte(String, f64),
}

impl MetadataCondition {
    /// Avalia a condição contra o metadata de um ponto.
    pub fn matches(&self, metadata: &serde_json::Value) -> bool {
        match self {
            MetadataCondition::Eq(key, expected) => metadata.get(key) == Some(expected),
            MetadataCondition::Ne(key, expected) => metadata.get(key) != Some(expected),
            MetadataCondition::In(key, values) => metadata
                .get(key)
                .map(|v| values.iter().any(|x| x == v))
                .unwrap_or(false),
            MetadataCondition::Gt(key, threshold) => metadata
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|n| n > *threshold)
                .unwrap_or(false),
            MetadataCondition::Lt(key, threshold) => metadata
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|n| n < *threshold)
                .unwrap_or(false),
            MetadataCondition::Gte(key, threshold) => metadata
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|n| n >= *threshold)
                .unwrap_or(false),
            MetadataCondition::Lte(key, threshold) => metadata
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|n| n <= *threshold)
                .unwrap_or(false),
        }
    }
}

// ─── MetadataFilter ────────────────────────────────────────────────────

/// Filtro de metadata para buscas vetoriais.
///
/// Suporta operadores: `$eq`, `$ne`, `$in`, `$gt`, `$lt`, `$gte`, `$lte`.
/// Formato simples `{"field": value}` é tratado como igualdade.
/// Múltiplos campos são combinados com lógica AND.
///
/// # Exemplo
///
/// ```rust,no_run
/// use ferres_db_core::MetadataFilter;
/// use serde_json::json;
///
/// // Igualdade (formato curto)
/// let f = MetadataFilter::from_json(json!({ "category": "tech" })).unwrap();
///
/// // Operadores
/// let f = MetadataFilter::from_json(json!({
///     "price": { "$gte": 10, "$lte": 100 },
///     "status": { "$in": ["active", "pending"] }
/// })).unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFilter {
    /// Condições (AND): todas devem ser satisfeitas.
    conditions: Vec<MetadataCondition>,
    /// Namespace como condição de primeira classe (multitenancy). Quando presente, só pontos desse namespace passam.
    #[serde(default)]
    pub namespace: Option<String>,
}

impl MetadataFilter {
    /// Cria um filtro a partir de um objeto JSON.
    ///
    /// - Objeto: cada chave é um campo; valor pode ser direto (equality) ou
    ///   um objeto com operadores: `$eq`, `$ne`, `$in`, `$gt`, `$lt`, `$gte`, `$lte`.
    /// - `null`: filtro vazio (não filtra).
    pub fn from_json(value: serde_json::Value) -> Result<Self, FerresError> {
        match value {
            serde_json::Value::Object(map) => {
                let mut conditions = Vec::new();
                let mut namespace = None;
                for (key, val) in map {
                    if key == "$namespace" {
                        let s = val.as_str().ok_or_else(|| FerresError::InvalidVector {
                            reason: "filter.$namespace must be a string".to_string(),
                        })?;
                        namespace = Some(s.to_string());
                    } else {
                        Self::parse_field_conditions(&key, val, &mut conditions)?;
                    }
                }
                Ok(Self {
                    conditions,
                    namespace,
                })
            }
            serde_json::Value::Null => Ok(Self {
                conditions: Vec::new(),
                namespace: None,
            }),
            _ => Err(FerresError::InvalidVector {
                reason: "filter must be a JSON object or null".to_string(),
            }),
        }
    }

    /// Parseia o valor de um campo: ou um único valor (Eq) ou um objeto com $op.
    fn parse_field_conditions(
        field: &str,
        value: serde_json::Value,
        out: &mut Vec<MetadataCondition>,
    ) -> Result<(), FerresError> {
        if let Some(obj) = value.as_object() {
            for (op, v) in obj {
                match op.as_str() {
                    "$eq" => out.push(MetadataCondition::Eq(field.to_string(), v.clone())),
                    "$ne" => out.push(MetadataCondition::Ne(field.to_string(), v.clone())),
                    "$in" => {
                        let arr = v
                            .as_array()
                            .ok_or_else(|| FerresError::InvalidVector {
                                reason: format!("filter.{field}: $in must be an array"),
                            })?
                            .clone();
                        out.push(MetadataCondition::In(field.to_string(), arr));
                    }
                    "$gt" => {
                        let n = Self::as_f64(v).ok_or_else(|| FerresError::InvalidVector {
                            reason: format!("filter.{field}: $gt must be a number"),
                        })?;
                        out.push(MetadataCondition::Gt(field.to_string(), n));
                    }
                    "$lt" => {
                        let n = Self::as_f64(v).ok_or_else(|| FerresError::InvalidVector {
                            reason: format!("filter.{field}: $lt must be a number"),
                        })?;
                        out.push(MetadataCondition::Lt(field.to_string(), n));
                    }
                    "$gte" => {
                        let n = Self::as_f64(v).ok_or_else(|| FerresError::InvalidVector {
                            reason: format!("filter.{field}: $gte must be a number"),
                        })?;
                        out.push(MetadataCondition::Gte(field.to_string(), n));
                    }
                    "$lte" => {
                        let n = Self::as_f64(v).ok_or_else(|| FerresError::InvalidVector {
                            reason: format!("filter.{field}: $lte must be a number"),
                        })?;
                        out.push(MetadataCondition::Lte(field.to_string(), n));
                    }
                    _ => {
                        return Err(FerresError::InvalidVector {
                            reason: format!("filter.{field}: unknown operator '{op}'"),
                        });
                    }
                }
            }
        } else {
            out.push(MetadataCondition::Eq(field.to_string(), value));
        }
        Ok(())
    }

    fn as_f64(v: &serde_json::Value) -> Option<f64> {
        v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
    }

    /// Cria um filtro vazio (sem condições, não filtra nada).
    pub fn empty() -> Self {
        Self {
            conditions: Vec::new(),
            namespace: None,
        }
    }

    /// Verifica se o filtro está vazio (sem condições nem namespace).
    pub fn is_empty(&self) -> bool {
        self.conditions.is_empty() && self.namespace.is_none()
    }

    /// Retorna as condições do filtro (para inspeção em testes).
    pub fn conditions(&self) -> &[MetadataCondition] {
        &self.conditions
    }

    /// Verifica se um ponto passa no filtro (apenas condições de metadata).
    ///
    /// Retorna `true` se o ponto atende a todas as condições (AND lógico).
    /// Para incluir namespace use [`matches_point`](Self::matches_point).
    pub fn matches(&self, metadata: &serde_json::Value) -> bool {
        if self.conditions.is_empty() {
            return true;
        }
        self.conditions.iter().all(|c| c.matches(metadata))
    }

    /// Verifica se o namespace do ponto satisfaz o filtro de namespace.
    /// Se o filtro não exige namespace (`self.namespace.is_none()`), retorna `true`.
    pub fn matches_namespace(&self, ns: Option<&str>) -> bool {
        match &self.namespace {
            None => true,
            Some(filter_ns) => ns == Some(filter_ns.as_str()),
        }
    }

    /// Verifica se um ponto passa no filtro completo (namespace + metadata).
    pub fn matches_point(&self, point: &Point) -> bool {
        self.matches_namespace(point.namespace.as_deref()) && self.matches(&point.metadata)
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

// ─── AnyCollection ─────────────────────────────────────────────────────

/// Wrapper interno que abstrai coleções com e sem tiered storage.
///
/// Quando tiered storage está habilitado, wraps `TieredCollection` que
/// gerencia pontos em três camadas (Hot/Warm/Cold). Caso contrário,
/// wraps uma `Collection` simples onde todos os pontos ficam em RAM.
///
/// Fornece uma interface uniforme para resolução de pontos que funciona
/// corretamente em qualquer cenário, evitando que pontos Warm/Cold
/// sejam silenciosamente descartados durante buscas.
enum AnyCollection {
    /// Coleção sem tiered storage. Todos os pontos em RAM.
    Plain(Box<Collection>),
    /// Coleção com tiered storage (Hot/Warm/Cold).
    Tiered(Box<TieredCollection>),
}

impl AnyCollection {
    /// Retorna referência à `Collection` interna.
    ///
    /// Para `Tiered`, retorna a coleção que contém os pontos Hot e o índice HNSW.
    fn collection(&self) -> &Collection {
        match self {
            AnyCollection::Plain(c) => c,
            AnyCollection::Tiered(tc) => tc.collection(),
        }
    }

    /// Retorna referência mutável à `Collection` interna.
    #[allow(dead_code)]
    fn collection_mut(&mut self) -> &mut Collection {
        match self {
            AnyCollection::Plain(c) => c,
            AnyCollection::Tiered(tc) => tc.collection_mut(),
        }
    }

    /// Resolve um ponto pelo ID buscando em todos os tiers de armazenamento.
    ///
    /// - **Plain**: busca no HashMap da Collection (RAM). ~0 µs.
    /// - **Tiered**: busca em Hot (RAM) → Warm (mmap) → Cold (disco).
    ///   Latência varia: Hot ~0 µs, Warm ~1-10 µs, Cold ~100+ µs (disk I/O).
    ///
    /// Para tiered, registra o acesso para promoção lazy no próximo
    /// ciclo de compactação (sem promover imediatamente).
    fn resolve_point(&self, id: &str) -> Option<Point> {
        match self {
            AnyCollection::Plain(c) => c.get(id).cloned(),
            AnyCollection::Tiered(tc) => tc.get_from_any_tier(id),
        }
    }

    /// Insere um ponto, tratando tiered storage se habilitado.
    ///
    /// Pontos sempre começam em Hot (RAM). Para `Tiered`, também
    /// atualiza o tracker de acessos e tier map.
    fn insert_point(&mut self, point: Point) -> Result<(), FerresError> {
        match self {
            AnyCollection::Plain(c) => c.insert(point),
            AnyCollection::Tiered(tc) => tc.insert(point),
        }
    }

    /// Remove um ponto de todos os tiers.
    ///
    /// Para `Plain`, remove do HashMap e do índice.
    /// Para `Tiered`, remove de Hot, Warm, Cold, tier map e tracker.
    fn remove_point_from_all(&mut self, id: &str) -> Result<(), FerresError> {
        match self {
            AnyCollection::Plain(c) => c.remove(id),
            AnyCollection::Tiered(tc) => {
                // Verifica se o ponto existe em algum tier
                if tc.point_tier(id).is_none() {
                    return Err(FerresError::PointNotFound(id.to_string()));
                }
                tc.remove(id)
            }
        }
    }

    /// Número total de pontos (todos os tiers).
    fn total_len(&self) -> usize {
        match self {
            AnyCollection::Plain(c) => c.len(),
            AnyCollection::Tiered(tc) => tc.len(),
        }
    }
}

// ─── VectorDB ──────────────────────────────────────────────────────────

/// API principal de alto nível para gerenciar coleções e realizar buscas vetoriais.
///
/// Gerencia múltiplas coleções em memória e persiste automaticamente
/// modificações em disco após cada operação de escrita.
///
/// Suporta tiered storage: quando `CollectionConfig::tiered_storage.enabled`
/// é `true`, pontos são automaticamente movidos entre camadas Hot (RAM),
/// Warm (mmap) e Cold (disco) baseado na frequência de acesso. Buscas
/// com filtro resolvem pontos de todos os tiers corretamente.
pub struct VectorDB {
    collections: HashMap<String, AnyCollection>,
    wals: HashMap<String, wal::Wal>,
    storage_path: PathBuf,
    storage_circuit_breaker: StorageCircuitBreaker,
    storage_options: StorageOptions,
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
        Self::with_storage_options(storage_path, StorageOptions::default())
    }

    /// Cria uma instância com opções de armazenamento (compressão WAL, snapshot binário).
    pub fn with_storage_options(
        storage_path: PathBuf,
        storage_options: StorageOptions,
    ) -> Result<Self, FerresError> {
        info!(path = %storage_path.display(), "initializing VectorDB");
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
            storage_circuit_breaker: StorageCircuitBreaker::new(),
            storage_options,
        };

        db.load_collections_from_disk()?;
        info!(collections = db.collections.len(), "VectorDB initialized");
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
    ///     enable_bm25: false,
    ///     bm25_text_field: "text".to_string(),
    ///     quantization: Default::default(),
    ///     tiered_storage: Default::default(),
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
            tiered = config.tiered_storage.enabled,
            "creating collection"
        );

        let name = config.name.clone();
        let collection_dir = self.storage_path.join("collections").join(&name);
        let collection = Collection::new(config.clone());

        // Wraps em TieredCollection se tiered storage está habilitado
        let any_col = if config.tiered_storage.enabled {
            std::fs::create_dir_all(&collection_dir).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to create collection directory {}: {e}",
                    collection_dir.display()
                ))
            })?;
            let tc = TieredCollection::new(
                collection,
                config.tiered_storage.clone(),
                Some(&collection_dir),
            )?;
            AnyCollection::Tiered(Box::new(tc))
        } else {
            AnyCollection::Plain(Box::new(collection))
        };

        self.collections.insert(name.clone(), any_col);

        // Abre WAL para a nova coleção
        let wal_handle = wal::Wal::open(
            &collection_dir,
            wal::Wal::DEFAULT_SNAPSHOT_THRESHOLD,
            self.storage_options.wal_compression,
        )?;
        self.wals.insert(name.clone(), wal_handle);

        // Auto-save após criação (snapshot inicial)
        self.save_collection(&name)?;

        info!(collection = %name, "collection created");
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
            let ac = self
                .collections
                .get(collection)
                .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;
            let col = ac.collection();

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
                    let normalized_vectors = normalize_vectors_parallel(&vectors)?;

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

        // Fase 3: insere pontos em memória (via AnyCollection para suporte a tiers)
        let ac = self
            .collections
            .get_mut(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        for point in prepared_points {
            if let Err(e) = ac.insert_point(point) {
                error!(
                    collection = %collection,
                    error = %e,
                    "failed to insert point"
                );
                return Err(e);
            }
        }
        let total_points = ac.total_len();

        // Fase 4: snapshot se threshold atingido
        let should_snapshot = self
            .wals
            .get(collection)
            .is_some_and(|w| w.should_snapshot());
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

    /// Remove pontos de uma coleção pelos IDs (e opcionalmente namespace).
    ///
    /// Quando `namespace` é `Some`, apenas pontos com esse namespace são removidos para os ids dados.
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
        namespace: Option<&str>,
    ) -> Result<(), FerresError> {
        if !self.collections.contains_key(collection) {
            return Err(FerresError::CollectionNotFound(collection.to_string()));
        }

        info!(
            collection = %collection,
            ids = ids.len(),
            "deleting points"
        );

        // WAL: registra deletes pelo storage_id
        if let Some(wal) = self.wals.get_mut(collection) {
            for id in &ids {
                let key = Point::storage_id_from_parts(namespace, id);
                wal.append_delete(&key)?;
            }
        }

        let ac = self
            .collections
            .get_mut(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        let (deleted_count, total_points) = {
            let mut not_found = Vec::new();
            for id in &ids {
                let key = Point::storage_id_from_parts(namespace, id);
                if let Err(e) = ac.remove_point_from_all(&key) {
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

            (ids.len() - not_found.len(), ac.total_len())
        };

        // Snapshot se threshold atingido
        let should_snapshot = self
            .wals
            .get(collection)
            .is_some_and(|w| w.should_snapshot());
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
        let ac = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;
        let col = ac.collection();

        // Valida dimensão do query
        col.validate_dimension(&query)?;

        info!(
            collection = %collection,
            limit,
            "performing search"
        );

        // Realiza a busca (HNSW retorna IDs de pontos em qualquer tier)
        let results = col.search(&query, limit, None, None)?;

        // Constrói SearchResults com resolução tier-aware (id lógico + namespace).
        let search_results: Vec<SearchResult> = results
            .into_iter()
            .filter_map(|(storage_id, score)| {
                let point = ac.resolve_point(&storage_id)?;
                Some(SearchResult {
                    id: point.id,
                    score,
                    metadata: point.metadata,
                    vector: None,
                    namespace: point.namespace,
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
    /// O filtro é aplicado **durante** a exploração do grafo HNSW (pre-filtering nativo
    /// via `search_filter`): o índice retorna até `limit` resultados que já satisfazem
    /// o filtro, garantindo maior precisão e consistência no número de resultados.
    ///
    /// # Tiered Storage
    ///
    /// Quando tiered storage está habilitado, esta busca resolve pontos de
    /// **todos os tiers** (Hot/Warm/Cold), garantindo que pontos demovidos
    /// não sejam silenciosamente descartados dos resultados filtrados.
    ///
    /// **Implicação de latência**: pontos em Cold tier requerem disk I/O
    /// (deserialização JSON) durante a resolução, adicionando ~100+ µs por
    /// ponto Cold nos resultados. Para workloads com muitos pontos Cold,
    /// considere:
    /// - Ajustar `warm_threshold_hours` para manter mais pontos em Warm (mmap)
    /// - Pontos frequentemente acessados via busca são automaticamente
    ///   promovidos no próximo ciclo de compactação (promoção lazy)
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
        let ac = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;
        let col = ac.collection();

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
            "performing filtered search (native HNSW pre-filtering)"
        );

        // Predicado: passa no filtro se o ponto (resolvido tier-aware) satisfaz namespace + metadata.
        let predicate = |storage_id: &str| -> bool {
            ac.resolve_point(storage_id)
                .map(|p| filter.matches_point(&p))
                .unwrap_or(false)
        };

        // Busca com pre-filtering nativo no HNSW: o índice aplica o predicado durante
        // a exploração do grafo e retorna até `limit` resultados que já passam no filtro.
        let results = col.search(&query, limit, Some(&predicate), None)?;

        // Constrói SearchResults com id lógico e namespace.
        let search_results: Vec<SearchResult> = results
            .into_iter()
            .filter_map(|(storage_id, score)| {
                let point = ac.resolve_point(&storage_id)?;
                Some(SearchResult {
                    id: point.id,
                    score,
                    metadata: point.metadata,
                    vector: None,
                    namespace: point.namespace,
                })
            })
            .collect();

        info!(
            collection = %collection,
            requested_limit = limit,
            filtered_results = search_results.len(),
            "filtered search completed"
        );

        Ok(search_results)
    }

    /// Busca com explicação detalhada de cada resultado.
    ///
    /// Retorna uma [`SearchExplanation`] que detalha **por que** cada resultado
    /// foi retornado (ou filtrado): score breakdown, avaliação de filtros
    /// condição-a-condição, posição no ranking antes/depois de filtros e
    /// estatísticas do índice HNSW.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{VectorDB, MetadataFilter};
    /// use serde_json::json;
    ///
    /// let db = VectorDB::new("./data".into())?;
    ///
    /// let explanation = db.search_explain(
    ///     "embeddings",
    ///     vec![0.15; 384],
    ///     5,
    ///     None,
    /// )?;
    ///
    /// for result in &explanation.results {
    ///     println!("ID: {}, Score: {:.4}, Passed filter: {}",
    ///         result.id, result.score,
    ///         result.filter_evaluation.as_ref().map_or(true, |f| f.passed));
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
    pub fn search_explain(
        &self,
        collection: &str,
        query: Vec<f32>,
        limit: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<SearchExplanation, FerresError> {
        let ac = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        let col = ac.collection();
        let resolver = |id: &str| ac.resolve_point(id);
        explain::build_search_explanation_with_resolver(col, &query, limit, filter, None, &resolver)
    }

    /// Retorna uma referência à coleção, se existir.
    ///
    /// Útil para acessar a `Collection` diretamente quando se quer usar
    /// funções do core como [`build_search_explanation`] sem passar pelo
    /// VectorDB.
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    pub fn get_collection(&self, name: &str) -> Result<&Collection, FerresError> {
        self.collections
            .get(name)
            .map(|ac| ac.collection())
            .ok_or_else(|| FerresError::CollectionNotFound(name.to_string()))
    }

    /// Estima o custo de uma busca vetorial antes da execução.
    ///
    /// Calcula heurísticas baseadas no tamanho da coleção, dimensão dos vetores,
    /// configuração HNSW e presença de filtros. Não executa nenhuma busca real.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::VectorDB;
    ///
    /// let db = VectorDB::new("./data".into())?;
    ///
    /// let estimate = db.estimate_query_cost("embeddings", 10, None, 0.0, 0.0)?;
    /// println!("Estimated: {:.2}ms, Expensive: {}", estimate.estimated_ms, estimate.is_expensive);
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// # Erros
    /// - `CollectionNotFound` se a coleção não existir.
    pub fn estimate_query_cost(
        &self,
        collection: &str,
        limit: usize,
        filter: Option<&MetadataFilter>,
        historical_p50: f64,
        historical_p95: f64,
    ) -> Result<QueryCostEstimate, FerresError> {
        let ac = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;
        let col = ac.collection();

        let config = col.config();
        let has_filter = filter.is_some_and(|f| !f.is_empty());
        let filter_conditions_count = filter.map_or(0, |f| f.conditions().len());
        let is_quantized = !matches!(config.quantization, QuantizationConfig::None);

        let params = cost::CostEstimateParams {
            collection_size: ac.total_len(),
            dimension: config.dimension,
            limit,
            ef_search: config.hnsw.ef_search,
            has_filter,
            filter_conditions_count,
            historical_p50,
            historical_p95,
            is_quantized,
        };

        Ok(cost::estimate_search_cost(&params))
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
    /// let point = db.get_point("embeddings", "doc-1", None)?;
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
        namespace: Option<&str>,
    ) -> Result<Point, FerresError> {
        let ac = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;

        let key = Point::storage_id_from_parts(namespace, id);
        ac.resolve_point(&key)
            .ok_or_else(|| FerresError::PointNotFound(id.to_string()))
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
    pub fn get_collection_stats(&self, collection: &str) -> Result<CollectionStats, FerresError> {
        let ac = self
            .collections
            .get(collection)
            .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;
        let col = ac.collection();

        // Usa total_len para contar pontos em todos os tiers
        let num_points = ac.total_len();

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
            .map(|(name, ac)| {
                let col = ac.collection();
                let config = col.config();
                let num_points = ac.total_len();

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
                match self
                    .storage_circuit_breaker
                    .call(|| wal::recover_collection(&path))
                {
                    Ok(Some(collection)) => {
                        let name = collection.name().to_string();
                        let tiered_enabled = collection.config().tiered_storage.enabled;
                        let tiered_config = collection.config().tiered_storage.clone();

                        // Abre WAL para esta coleção
                        let mut wal_handle = wal::Wal::open(
                            &path,
                            wal::Wal::DEFAULT_SNAPSHOT_THRESHOLD,
                            self.storage_options.wal_compression,
                        )?;

                        // Se o WAL tinha entradas, consolida com snapshot + truncate
                        if wal_handle.ops_since_snapshot() > 0 {
                            self.storage_circuit_breaker.call(|| {
                                FileStorage::save_collection(
                                    &collection,
                                    &path,
                                    self.storage_options.binary_snapshot,
                                    self.storage_options.namespace_physical_isolation,
                                )
                            })?;
                            wal_handle.truncate_after_snapshot()?;
                            info!(collection = %name, "post-recovery snapshot created");
                        }

                        info!(
                            collection = %name,
                            points = collection.len(),
                            tiered = tiered_enabled,
                            "loaded collection from disk"
                        );

                        // Wraps em TieredCollection se tiered storage está habilitado
                        let any_col = if tiered_enabled {
                            let tc = TieredCollection::new(collection, tiered_config, Some(&path))?;
                            AnyCollection::Tiered(Box::new(tc))
                        } else {
                            AnyCollection::Plain(Box::new(collection))
                        };

                        self.collections.insert(name.clone(), any_col);
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

    /// Salva uma coleção no disco (protegido por circuit breaker).
    fn save_collection(&self, name: &str) -> Result<(), FerresError> {
        let ac = self
            .collections
            .get(name)
            .ok_or_else(|| FerresError::CollectionNotFound(name.to_string()))?;
        let col = ac.collection();

        let collection_dir = self.storage_path.join("collections").join(name);
        self.storage_circuit_breaker.call(|| {
            FileStorage::save_collection(
                col,
                &collection_dir,
                self.storage_options.binary_snapshot,
                self.storage_options.namespace_physical_isolation,
            )
        })?;

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
            hnsw: HnswConfig {
                ef_search: 64, // garante exploração suficiente em grafos pequenos (evita flakiness)
                ..HnswConfig::default()
            },
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
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

        assert_eq!(filter.conditions().len(), 2);
        assert!(filter.matches(&json!({"category": "tech", "status": "active"})));
        assert!(!filter.matches(&json!({"category": "tech", "status": "inactive"})));
    }

    #[test]
    fn test_metadata_filter_empty() {
        let filter = MetadataFilter::from_json(json!(null)).unwrap();
        assert!(filter.is_empty());

        let filter2 = MetadataFilter::empty();
        assert!(filter2.is_empty());
    }

    #[test]
    fn test_metadata_filter_namespace() {
        let filter = MetadataFilter::from_json(json!({ "$namespace": "tenant-a" })).unwrap();
        assert_eq!(filter.namespace.as_deref(), Some("tenant-a"));
        assert!(filter.conditions().is_empty());
        assert!(!filter.is_empty());

        assert!(filter.matches_namespace(Some("tenant-a")));
        assert!(!filter.matches_namespace(Some("tenant-b")));
        assert!(!filter.matches_namespace(None));

        let mut point = Point::new("p1", vec![1.0], serde_json::Value::Null).unwrap();
        point.namespace = Some("tenant-a".into());
        assert!(filter.matches_point(&point));
        point.namespace = Some("tenant-b".into());
        assert!(!filter.matches_point(&point));
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
    fn test_metadata_filter_operators_ne_in_gt_lt() {
        // $ne: campo != valor (campo ausente também satisfaz)
        let f_ne = MetadataFilter::from_json(json!({ "status": { "$ne": "inactive" } })).unwrap();
        assert!(f_ne.matches(&json!({"status": "active"})));
        assert!(f_ne.matches(&json!({"other": "x"}))); // ausente = não igual
        assert!(!f_ne.matches(&json!({"status": "inactive"})));

        // $in: valor na lista
        let f_in = MetadataFilter::from_json(json!({ "cat": { "$in": ["a", "b"] } })).unwrap();
        assert!(f_in.matches(&json!({"cat": "a"})));
        assert!(f_in.matches(&json!({"cat": "b"})));
        assert!(!f_in.matches(&json!({"cat": "c"})));
        assert!(!f_in.matches(&json!({})));

        // $gt, $lt, $gte, $lte
        let f_gt = MetadataFilter::from_json(json!({ "price": { "$gt": 10 } })).unwrap();
        assert!(f_gt.matches(&json!({"price": 11})));
        assert!(!f_gt.matches(&json!({"price": 10})));
        assert!(!f_gt.matches(&json!({"price": 9})));

        let f_lt = MetadataFilter::from_json(json!({ "price": { "$lt": 100 } })).unwrap();
        assert!(f_lt.matches(&json!({"price": 99})));
        assert!(!f_lt.matches(&json!({"price": 100})));

        let f_range = MetadataFilter::from_json(json!({
            "price": { "$gte": 10, "$lte": 20 }
        }))
        .unwrap();
        assert!(f_range.matches(&json!({"price": 10})));
        assert!(f_range.matches(&json!({"price": 15})));
        assert!(f_range.matches(&json!({"price": 20})));
        assert!(!f_range.matches(&json!({"price": 9})));
        assert!(!f_range.matches(&json!({"price": 21})));
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
        let filter =
            MetadataFilter::from_json(json!({"category": "tech", "status": "active"})).unwrap();
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

        // Usa 3+ pontos para evitar flakiness com HNSW em grafos muito pequenos
        // (com apenas 2 pontos, o grafo HNSW pode não conectar ambos dependendo
        // da atribuição aleatória de níveis).
        let points = vec![
            Point::new("p1", vec![1.0, 0.0, 0.0], json!({"category": "tech"})).unwrap(),
            Point::new("p2", vec![0.0, 1.0, 0.0], json!({"category": "science"})).unwrap(),
            Point::new("p3", vec![0.0, 0.0, 1.0], json!({"category": "math"})).unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Filtro vazio deve retornar todos os resultados (igual a search normal)
        let filter = MetadataFilter::empty();
        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();
        assert_eq!(filtered_results.len(), 3);

        // None também deve funcionar como filtro vazio
        let filtered_results2 = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, None)
            .unwrap();
        assert_eq!(filtered_results2.len(), 3);
    }

    #[test]
    fn test_search_with_filter_field_not_exists() {
        let (mut db, _temp_dir) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        // Ponto sem o campo "category"
        let points =
            vec![Point::new("p1", vec![1.0, 0.0, 0.0], json!({"other": "value"})).unwrap()];

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
            Point::new("p1", vec![1.0, 0.0, 0.0], json!({"category": "tech"})).unwrap(),
            Point::new("p2", vec![0.0, 1.0, 0.0], json!({"category": "tech"})).unwrap(),
            // Ponto que não corresponde ao filtro
            Point::new("p3", vec![0.0, 0.0, 1.0], json!({"category": "science"})).unwrap(),
        ];

        db.upsert_points("test", points).unwrap();

        // Busca com limit maior que o número de resultados filtrados
        let filter = MetadataFilter::from_json(json!({"category": "tech"})).unwrap();
        let filtered_results = db
            .search_with_filter("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();

        // Deve retornar apenas os 2 pontos que correspondem ao filtro
        assert_eq!(filtered_results.len(), 2);
        assert!(filtered_results
            .iter()
            .all(|r| r.id == "p1" || r.id == "p2"));
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

        // Deve retornar apenas p1 e p2 (ambos têm category=tech E status=active); p3 não deve aparecer
        assert!(!filtered_results.is_empty());
        assert!(filtered_results.len() <= 2);
        assert!(filtered_results
            .iter()
            .all(|r| r.id == "p1" || r.id == "p2"));
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
