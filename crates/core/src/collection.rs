//! # Collection — container lógico de pontos vetoriais
//!
//! Uma `Collection` é a unidade principal de organização no FerresDB.
//! Ela combina:
//! - Um `HashMap<String, Point>` para acesso O(1) por ID
//! - Um `Box<dyn ANNIndex>` para busca ANN desacoplada do backend
//! - Validação de dimensionalidade consistente
//!
//! ## Decisões arquiteturais
//!
//! - **`Box<dyn ANNIndex>`**: o índice é um trait object. Isso permite
//!   que o server injete implementações diferentes (HNSW para produção,
//!   brute-force para testes) sem alterar a Collection.
//!
//! - **`validate_dimension`** como método público: permite validação
//!   antecipada antes de criar o Point (útil em endpoints HTTP que
//!   recebem vetores brutos).
//!
//! - **`DistanceMetric` na config**: define a métrica no nível da
//!   coleção. Todos os pontos da mesma coleção usam a mesma métrica.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use lru::LruCache;
use serde::{Deserialize, Serialize};
use tracing::debug;

use rayon::prelude::*;

use crate::bm25::BM25Index;
use crate::error::FerresError;
use crate::explain::ExplainMeta;
use crate::graph;
use crate::point::Point;
use crate::quantization::QuantizationConfig;
use crate::search::{
    create_ann_index, distance_between, normalize_vectors_parallel, ANNIndex, DistanceMetric,
    HnswConfig,
};
use crate::tiered::TieredStorageConfig;

// ─── CollectionConfig ───────────────────────────────────────────────

/// Configuração para criar uma nova coleção.
///
/// A `distance` determina como a similaridade entre vetores é medida.
/// Uma vez criada, `dimension` e `distance` são imutáveis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    pub name: String,
    pub dimension: usize,
    pub distance: DistanceMetric,
    #[serde(default)]
    pub hnsw: HnswConfig,
    /// Tamanho do cache LRU para resultados de busca (0 = desabilitado).
    /// Padrão: 100 queries.
    #[serde(default = "default_cache_size")]
    pub search_cache_size: usize,
    /// Habilita índice BM25 para busca híbrida (vetorial + keyword).
    #[serde(default)]
    pub enable_bm25: bool,
    /// Chave em `metadata` usada como texto para BM25. Padrão: "text".
    #[serde(default = "default_bm25_text_field")]
    pub bm25_text_field: String,
    /// Configuração de quantização de vetores (default: None = sem quantização).
    /// SQ8 comprime vetores f32 para u8 com ~4× economia de memória.
    #[serde(default)]
    pub quantization: QuantizationConfig,
    /// Configuração de tiered storage (default: desabilitado).
    /// Quando habilitado, pontos são movidos automaticamente entre camadas
    /// Hot (RAM), Warm (mmap) e Cold (disco) baseado na frequência de acesso.
    #[serde(default)]
    pub tiered_storage: TieredStorageConfig,
    /// Período de retenção em dias para snapshots e WAL. None = manter indefinidamente.
    /// O worker de retenção remove entradas do WAL mais antigas que este período.
    #[serde(default)]
    pub retention_days: Option<u32>,
}

fn default_cache_size() -> usize {
    100
}

fn default_bm25_text_field() -> String {
    "text".to_string()
}

// ─── Collection ─────────────────────────────────────────────────────

/// Chave de cache para resultados de busca.
/// Usa hash do vetor de query, k e campo vetorial para identificar queries únicas.
#[derive(Debug, Clone)]
struct CacheKey {
    query_hash: u64,
    k: usize,
    vector_field: Option<String>,
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.query_hash.hash(state);
        self.k.hash(state);
        self.vector_field.hash(state);
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.query_hash == other.query_hash
            && self.k == other.k
            && self.vector_field == other.vector_field
    }
}

impl Eq for CacheKey {}

/// Extrai texto de `metadata` pela chave configurada (ex. "text" ou "content").
fn metadata_text(metadata: &serde_json::Value, field: &str) -> String {
    metadata
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

use crate::fusion::{self, FusionStrategy, DEFAULT_RRF_K};
use crate::search::validate_vector_finite;

/// Threshold para usar rebuild completo do índice vs inserção incremental.
const BATCH_REBUILD_THRESHOLD: usize = 100;

/// Constrói um ponto "sintético" com o mesmo id/metadata/namespace que `p`, mas com `vector` substituído.
/// Usado para indexar vetores nomeados em índices separados.
fn synthetic_point_with_vector(p: &Point, vector: Vec<f32>) -> Point {
    Point {
        id: p.id.clone(),
        vector,
        metadata: p.metadata.clone(),
        created_at: p.created_at,
        namespace: p.namespace.clone(),
        expires_at: p.expires_at.clone(),
        vectors: None,
        relations: p.relations.clone(),
    }
}

// ─── BatchInsertResult ──────────────────────────────────────────────

/// Resultado de uma operação de insert em batch.
///
/// Fornece estatísticas sobre a operação, incluindo número de pontos
/// inseridos com sucesso.
#[derive(Debug, Clone)]
pub struct BatchInsertResult {
    /// Número de pontos inseridos com sucesso.
    pub inserted: usize,
}

/// Uma coleção de pontos vetoriais com índice de busca ANN.
///
/// Suporta múltiplos vetores por ponto: o vetor principal (`point.vector`)
/// é indexado no `index`; vetores nomeados (`point.vectors`) são indexados
/// em `vector_indices` por nome de campo (ex.: "title_vector", "content_vector").
pub struct Collection {
    config: CollectionConfig,
    points: HashMap<String, Point>,
    index: Box<dyn ANNIndex>,
    /// Índices ANN por campo vetorial nomeado (ex.: "title_vector").
    /// O vetor principal usa `index`; buscas com `vector_field` usam estes.
    vector_indices: HashMap<String, Box<dyn ANNIndex>>,
    /// Índice BM25 opcional para busca híbrida.
    bm25_index: Option<BM25Index>,
    /// Cache LRU opcional para resultados de busca.
    /// Mutex é necessário porque search() é &self mas precisa mutar o cache.
    #[allow(dead_code, clippy::type_complexity)]
    search_cache: Option<Mutex<LruCache<CacheKey, Vec<(String, f32)>>>>,
    /// Contadores para Cache Hit Rate (hits e misses do search_cache).
    search_cache_hits: AtomicU64,
    search_cache_misses: AtomicU64,
    /// Flag indicando se a coleção foi modificada e precisa ser salva.
    dirty: AtomicBool,
}

impl Collection {
    /// Cria uma coleção vazia usando o backend de índice apropriado.
    ///
    /// Se `quantization` é `None`, usa `HnswIndex` padrão.
    /// Se `Scalar(...)`, usa `QuantizedHnswIndex` com SQ8.
    pub fn new(config: CollectionConfig) -> Self {
        let index = create_ann_index(config.distance, config.hnsw.clone(), &config.quantization);
        let search_cache = if config.search_cache_size > 0 {
            Some(Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(config.search_cache_size).unwrap(),
            )))
        } else {
            None
        };
        let bm25_index = if config.enable_bm25 {
            Some(BM25Index::new())
        } else {
            None
        };
        debug!(
            name = %config.name,
            dim = config.dimension,
            distance = ?config.distance,
            cache_size = config.search_cache_size,
            "collection created"
        );
        Self {
            config,
            points: HashMap::new(),
            index,
            vector_indices: HashMap::new(),
            bm25_index,
            search_cache,
            search_cache_hits: AtomicU64::new(0),
            search_cache_misses: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    /// Cria uma coleção com um índice ANN customizado.
    ///
    /// Útil para injetar implementações alternativas (brute-force
    /// para testes, mocks, etc).
    pub fn with_index(config: CollectionConfig, index: Box<dyn ANNIndex>) -> Self {
        let search_cache = if config.search_cache_size > 0 {
            Some(Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(config.search_cache_size).unwrap(),
            )))
        } else {
            None
        };
        let bm25_index = if config.enable_bm25 {
            Some(BM25Index::new())
        } else {
            None
        };
        debug!(
            name = %config.name,
            dim = config.dimension,
            "collection created with custom index"
        );
        Self {
            config,
            points: HashMap::new(),
            index,
            vector_indices: HashMap::new(),
            bm25_index,
            search_cache,
            search_cache_hits: AtomicU64::new(0),
            search_cache_misses: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    /// Reconstrói uma coleção a partir de pontos carregados do disco.
    ///
    /// Re-indexa todos os pontos via `build`. Usado no startup do
    /// server ao carregar coleções persistidas.
    /// Índices nomeados (`vector_indices`) são construídos para cada
    /// campo presente em `point.vectors`.
    pub fn from_points(config: CollectionConfig, points: Vec<Point>) -> Result<Self, FerresError> {
        let mut collection = Self::new(config);
        collection.index.build(&points)?;
        for point in &points {
            collection.points.insert(point.storage_id(), point.clone());
        }
        // Build indices for named vector fields
        let field_names: std::collections::HashSet<String> = points
            .iter()
            .filter_map(|p| p.vectors.as_ref())
            .flat_map(|m| m.keys().cloned())
            .collect();
        for field_name in field_names {
            let synthetic: Vec<Point> = points
                .iter()
                .filter_map(|p| {
                    p.vectors
                        .as_ref()
                        .and_then(|m| m.get(&field_name))
                        .cloned()
                        .map(|vec| synthetic_point_with_vector(p, vec))
                })
                .collect();
            if synthetic.is_empty() {
                continue;
            }
            let mut idx = create_ann_index(
                collection.config.distance,
                collection.config.hnsw.clone(),
                &collection.config.quantization,
            );
            idx.build(&synthetic)?;
            collection.vector_indices.insert(field_name, idx);
        }
        if let Some(ref mut bm25) = collection.bm25_index {
            let field = &collection.config.bm25_text_field;
            for point in collection.points.values() {
                let text = metadata_text(&point.metadata, field);
                bm25.index_document(&point.storage_id(), &text);
            }
        }
        Ok(collection)
    }

    /// Valida se um vetor tem a dimensão correta para esta coleção.
    ///
    /// Retorna `Ok(())` se a dimensão bate, ou `Err(DimensionMismatch)`
    /// caso contrário. Útil para validar input antes de criar o Point.
    pub fn validate_dimension(&self, vector: &[f32]) -> Result<(), FerresError> {
        if vector.len() != self.config.dimension {
            return Err(FerresError::DimensionMismatch {
                expected: self.config.dimension,
                got: vector.len(),
            });
        }
        Ok(())
    }

    /// Valida que todos os vetores nomeados do ponto têm dimensão correta e valores finitos.
    fn validate_point_named_vectors(&self, point: &Point) -> Result<(), FerresError> {
        if let Some(ref map) = point.vectors {
            for (name, vec) in map {
                self.validate_dimension(vec)?;
                validate_vector_finite(vec)?;
                if vec.is_empty() {
                    return Err(FerresError::InvalidVector {
                        reason: format!("named vector '{name}' cannot be empty"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Obtém ou cria o índice ANN para um campo vetorial nomeado.
    fn get_or_create_vector_index(
        &mut self,
        field: &str,
    ) -> Result<&mut Box<dyn ANNIndex>, FerresError> {
        if !self.vector_indices.contains_key(field) {
            let idx = create_ann_index(
                self.config.distance,
                self.config.hnsw.clone(),
                &self.config.quantization,
            );
            self.vector_indices.insert(field.to_string(), idx);
        }
        Ok(self.vector_indices.get_mut(field).unwrap())
    }

    /// Invalida o cache de busca após mutação (insert/remove).
    /// Evita resultados desatualizados e crescimento indefinido do cache em workloads write-heavy.
    fn invalidate_search_cache(&self) {
        if let Some(cache) = &self.search_cache {
            if let Ok(mut guard) = cache.lock() {
                guard.clear();
            }
        }
    }

    /// Insere um ponto na coleção e no índice ANN.
    ///
    /// Valida a dimensão antes de inserir. Se já existir um ponto
    /// com o mesmo ID, ele é substituído (upsert).
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, Point};
    ///
    /// let mut collection = Collection::new(CollectionConfig {
    ///     name: "test".into(),
    ///     dimension: 3,
    ///     distance: DistanceMetric::Euclidean,
    ///     hnsw: Default::default(),
    ///     search_cache_size: 0,
    ///     enable_bm25: false,
    ///     bm25_text_field: "text".to_string(),
    ///     quantization: Default::default(),
    ///     tiered_storage: Default::default(),
    /// });
    ///
    /// let point = Point::new("p1", vec![1.0, 2.0, 3.0], serde_json::json!(null))?;
    /// collection.insert(point)?;
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    pub fn insert(&mut self, point: Point) -> Result<(), FerresError> {
        self.validate_dimension(&point.vector)?;
        self.validate_point_named_vectors(&point)?;
        self.index.add_point(&point)?;
        if let Some(ref vectors) = point.vectors {
            for (name, vec) in vectors {
                let syn = synthetic_point_with_vector(&point, vec.clone());
                let idx = self.get_or_create_vector_index(name)?;
                idx.add_point(&syn)?;
            }
        }
        if let Some(ref mut bm25) = self.bm25_index {
            let text = metadata_text(&point.metadata, &self.config.bm25_text_field);
            bm25.index_document(&point.storage_id(), &text);
        }
        self.points.insert(point.storage_id(), point);
        self.invalidate_search_cache();
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    /// Insere múltiplos pontos em batch de forma otimizada.
    ///
    /// Esta operação é significativamente mais eficiente que chamar `insert`
    /// repetidamente para grandes volumes de pontos (50-70% mais rápido para
    /// batches > 100 pontos).
    ///
    /// ## Otimizações aplicadas
    ///
    /// 1. **Validação paralela**: valida dimensões de todos os vetores em paralelo
    /// 2. **Normalização paralela**: para métrica Cosine, normaliza vetores em paralelo
    /// 3. **Rebuild do índice**: para batches grandes, reconstrói o índice HNSW
    ///    com todos os pontos de uma vez (mais eficiente que inserções incrementais)
    ///
    /// ## Trade-offs
    ///
    /// - Maior uso de memória durante rebuild (mantém pontos antigos + novos)
    /// - Para batches pequenos (< 100 pontos), usa inserção incremental
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, Point};
    ///
    /// let mut collection = Collection::new(CollectionConfig {
    ///     name: "test".into(),
    ///     dimension: 3,
    ///     distance: DistanceMetric::Euclidean,
    ///     hnsw: Default::default(),
    ///     search_cache_size: 0,
    ///     enable_bm25: false,
    ///     bm25_text_field: "text".to_string(),
    ///     quantization: Default::default(),
    ///     tiered_storage: Default::default(),
    /// });
    ///
    /// let points = vec![
    ///     Point::new("p1", vec![1.0, 2.0, 3.0], serde_json::json!(null))?,
    ///     Point::new("p2", vec![4.0, 5.0, 6.0], serde_json::json!(null))?,
    /// ];
    ///
    /// let result = collection.insert_batch(points)?;
    /// assert_eq!(result.inserted, 2);
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    pub fn insert_batch(&mut self, points: Vec<Point>) -> Result<BatchInsertResult, FerresError> {
        if points.is_empty() {
            return Ok(BatchInsertResult { inserted: 0 });
        }

        // Fase 1: Validação de dimensões e vetores nomeados
        if points.len() > BATCH_REBUILD_THRESHOLD {
            let validation_errors: Vec<_> = points
                .par_iter()
                .filter_map(|point| self.validate_dimension(&point.vector).err())
                .collect();

            if !validation_errors.is_empty() {
                return Err(validation_errors.into_iter().next().unwrap());
            }
            for point in &points {
                self.validate_point_named_vectors(point)?;
            }
        } else {
            for point in &points {
                self.validate_dimension(&point.vector)?;
                self.validate_point_named_vectors(point)?;
            }
        }

        // Fase 2: Preparação dos vetores (normalização para Cosine)
        let prepared_points = if self.config.distance == DistanceMetric::Cosine
            && points.len() > BATCH_REBUILD_THRESHOLD
        {
            // Normaliza vetores em paralelo
            let vectors: Vec<Vec<f32>> = points.iter().map(|p| p.vector.clone()).collect();
            let normalized = normalize_vectors_parallel(&vectors)?;

            points
                .into_iter()
                .zip(normalized)
                .map(|(mut p, nv)| {
                    p.vector = nv;
                    p
                })
                .collect::<Vec<_>>()
        } else {
            points
        };

        // Fase 3: Inserção no índice HNSW (e índices nomeados)
        if prepared_points.len() > BATCH_REBUILD_THRESHOLD {
            // Para batches grandes, rebuild completo é mais eficiente
            let all_points: Vec<Point> = self
                .points
                .values()
                .cloned()
                .chain(prepared_points.iter().cloned())
                .collect();

            self.index.build(&all_points)?;

            let field_names: std::collections::HashSet<String> = all_points
                .iter()
                .filter_map(|p| p.vectors.as_ref())
                .flat_map(|m| m.keys().cloned())
                .collect();
            for field_name in field_names {
                let synthetic: Vec<Point> = all_points
                    .iter()
                    .filter_map(|p| {
                        p.vectors
                            .as_ref()
                            .and_then(|m| m.get(&field_name))
                            .cloned()
                            .map(|vec| synthetic_point_with_vector(p, vec))
                    })
                    .collect();
                if synthetic.is_empty() {
                    continue;
                }
                let mut idx = create_ann_index(
                    self.config.distance,
                    self.config.hnsw.clone(),
                    &self.config.quantization,
                );
                idx.build(&synthetic)?;
                self.vector_indices.insert(field_name, idx);
            }

            debug!(
                collection = %self.config.name,
                existing = self.points.len(),
                new = prepared_points.len(),
                "index rebuilt for batch insert"
            );
        } else {
            for point in &prepared_points {
                self.index.add_point(point)?;
                if let Some(ref vectors) = point.vectors {
                    for (name, vec) in vectors {
                        let syn = synthetic_point_with_vector(point, vec.clone());
                        let idx = self.get_or_create_vector_index(name)?;
                        idx.add_point(&syn)?;
                    }
                }
            }
        }

        // Fase 4: Atualiza HashMap e BM25
        let inserted = prepared_points.len();
        for point in prepared_points {
            let key = point.storage_id();
            if let Some(ref mut bm25) = self.bm25_index {
                let text = metadata_text(&point.metadata, &self.config.bm25_text_field);
                bm25.index_document(&key, &text);
            }
            self.points.insert(key, point);
        }

        self.invalidate_search_cache();
        self.dirty.store(true, Ordering::Release);

        Ok(BatchInsertResult { inserted })
    }

    /// Busca os `k` vizinhos mais próximos do vetor de consulta.
    ///
    /// Valida a dimensão do query antes da busca.
    /// Usa cache LRU se habilitado na configuração.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, Point};
    ///
    /// let mut collection = Collection::new(CollectionConfig {
    ///     name: "test".into(),
    ///     dimension: 3,
    ///     distance: DistanceMetric::Euclidean,
    ///     hnsw: Default::default(),
    ///     search_cache_size: 0,
    ///     enable_bm25: false,
    ///     bm25_text_field: "text".to_string(),
    ///     quantization: Default::default(),
    ///     tiered_storage: Default::default(),
    /// });
    ///
    /// collection.insert(Point::new("p1", vec![1.0, 0.0, 0.0], serde_json::json!(null))?)?;
    /// collection.insert(Point::new("p2", vec![0.0, 1.0, 0.0], serde_json::json!(null))?)?;
    ///
    /// let results = collection.search(&[1.0, 0.0, 0.0], 2, None, None)?;
    /// assert_eq!(results.len(), 2);
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    ///
    /// Quando `predicate` é `Some`, a busca aplica pre-filtering no índice (HNSW)
    /// e o cache LRU não é usado.
    ///
    /// O parâmetro `vector_field` indica contra qual vetor buscar: `None` ou
    /// `"default"` usa o vetor principal; outro nome (ex.: `"title_vector"`) usa
    /// o índice do campo nomeado, se existir.
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
        vector_field: Option<&str>,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        self.validate_dimension(query)?;

        let index_to_use = match vector_field {
            None | Some("default") => None,
            Some(f) => {
                if !self.vector_indices.contains_key(f) {
                    return Err(FerresError::UnknownVectorField(f.to_string()));
                }
                Some(f)
            }
        };

        // Com predicado, não usar cache (resultado depende do filtro).
        if predicate.is_none() {
            use std::collections::hash_map::DefaultHasher;
            let mut hasher = DefaultHasher::new();
            query.iter().for_each(|x| {
                x.to_bits().hash(&mut hasher);
            });
            let query_hash = hasher.finish();
            let cache_key = CacheKey {
                query_hash,
                k,
                vector_field: index_to_use.map(String::from),
            };

            if let Some(cache) = &self.search_cache {
                if let Ok(mut cache_guard) = cache.lock() {
                    if let Some(cached_results) = cache_guard.get(&cache_key) {
                        self.search_cache_hits.fetch_add(1, Ordering::Relaxed);
                        return Ok(cached_results.clone());
                    }
                }
                self.search_cache_misses.fetch_add(1, Ordering::Relaxed);
            }

            let results = {
                let _span = tracing::info_span!("collection.search",
                    points = self.points.len(),
                    dimension = self.config.dimension,
                    vector_field = ?index_to_use,
                )
                .entered();
                match index_to_use {
                    None => self.index.search(query, k, None)?,
                    Some(f) => self.vector_indices.get(f).unwrap().search(query, k, None)?,
                }
            };

            if let Some(cache) = &self.search_cache {
                if let Ok(mut cache_guard) = cache.lock() {
                    cache_guard.put(cache_key, results.clone());
                }
            }
            return Ok(results);
        }

        let _span = tracing::info_span!("collection.search",
            points = self.points.len(),
            dimension = self.config.dimension,
            vector_field = ?index_to_use,
        )
        .entered();
        match index_to_use {
            None => self.index.search(query, k, predicate),
            Some(f) => self
                .vector_indices
                .get(f)
                .unwrap()
                .search(query, k, predicate),
        }
    }

    /// Busca com re-ranking opcional via Cross-Encoder (ex.: BGE-Reranker).
    ///
    /// Quando `reranker` é `Some`, recupera `limit * 5` candidatos via HNSW, re-pontua cada um
    /// com o modelo Cross-Encoder (query + vetor do documento) e retorna os top `limit` ordenados
    /// pelo score do reranker (maior = mais relevante). Quando `reranker` é `None`, equivale a
    /// [`search`](Self::search).
    pub fn search_with_rerank(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
        vector_field: Option<&str>,
        reranker: Option<&dyn crate::rerank::Reranker>,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        self.validate_dimension(query)?;
        let r = match reranker {
            Some(r) => r,
            None => return self.search(query, k, predicate, vector_field),
        };
        if r.dimension() != self.config.dimension {
            return Err(FerresError::InvalidVector {
                reason: format!(
                    "reranker dimension {} does not match collection dimension {}",
                    r.dimension(),
                    self.config.dimension
                ),
            });
        }
        let index_to_use = match vector_field {
            None | Some("default") => None,
            Some(f) => {
                if !self.vector_indices.contains_key(f) {
                    return Err(FerresError::UnknownVectorField(f.to_string()));
                }
                Some(f)
            }
        };
        let k_candidates = (k * 5).min(self.points.len().max(1));
        let candidates = match index_to_use {
            None => self.index.search(query, k_candidates, predicate)?,
            Some(f) => {
                self.vector_indices
                    .get(f)
                    .unwrap()
                    .search(query, k_candidates, predicate)?
            }
        };
        if candidates.is_empty() {
            return Ok(candidates);
        }
        let mut scored: Vec<(String, f32)> = Vec::with_capacity(candidates.len());
        for (storage_id, _) in candidates {
            let doc_vec = match index_to_use {
                None => self.points.get(&storage_id).map(|p| p.vector.as_slice()),
                Some(f) => self
                    .points
                    .get(&storage_id)
                    .and_then(|p| p.vectors.as_ref())
                    .and_then(|m| m.get(f))
                    .map(|v| v.as_slice()),
            };
            let doc_vec = match doc_vec {
                Some(v) => v,
                None => continue,
            };
            match r.score(query, doc_vec) {
                Ok(score) => scored.push((storage_id, score)),
                Err(_) => continue,
            }
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        Ok(scored)
    }

    /// Busca os `k` vizinhos mais próximos com metadados de explicação.
    ///
    /// Semelhante a [`search`], mas retorna [`ExplainMeta`] adicional para
    /// cada resultado com informações internas do índice (candidatos visitados,
    /// camadas percorridas, tombstones ignorados). Não usa cache LRU.
    pub fn search_explain(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
        vector_field: Option<&str>,
    ) -> Result<Vec<(String, f32, ExplainMeta)>, FerresError> {
        self.validate_dimension(query)?;
        let index_to_use = match vector_field {
            None | Some("default") => None,
            Some(f) => {
                if !self.vector_indices.contains_key(f) {
                    return Err(FerresError::UnknownVectorField(f.to_string()));
                }
                Some(f)
            }
        };
        match index_to_use {
            None => self.index.search_explain(query, k, predicate),
            Some(f) => self
                .vector_indices
                .get(f)
                .unwrap()
                .search_explain(query, k, predicate),
        }
    }

    /// Busca conectada (graph + vetor): restringe candidatos ao subgrafo e ordena por similaridade.
    ///
    /// 1. Executa BFS a partir de `center_point_id` com `hops` saltos (subconjunto conectado).
    /// 2. Dentro desse subconjunto, calcula a distância vetorial (Cosine/Euclidean/DotProduct)
    ///    contra `query_vector`.
    /// 3. Retorna os top `k` mais similares semanticamente e estruturalmente conectados.
    pub fn search_connected(
        &self,
        query_vector: &[f32],
        center_point_id: &str,
        hops: u32,
        k: usize,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        self.validate_dimension(query_vector)?;
        let get_point = |id: &str| self.points.get(id).cloned();
        let candidates = graph::traverse_bfs(get_point, center_point_id, hops)?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let metric = self.config.distance;
        let mut scored: Vec<(String, f32)> = candidates
            .iter()
            .map(|p| {
                let dist = distance_between(query_vector, &p.vector, metric);
                (p.storage_id(), dist)
            })
            .collect();
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        Ok(scored)
    }

    /// Busca híbrida: combina resultados vetoriais e BM25 via estratégia de fusão.
    ///
    /// Requer que a coleção tenha BM25 habilitado (`enable_bm25: true`).
    ///
    /// # Estratégias de fusão
    ///
    /// - `WeightedScore { alpha }`: pondera rankings por alpha (comportamento original).
    /// - `RRF { k }`: Reciprocal Rank Fusion pura (sem ponderação).
    pub fn hybrid_search(
        &self,
        query_vector: &[f32],
        query_text: &str,
        limit: usize,
        strategy: &FusionStrategy,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        self.validate_dimension(query_vector)?;
        let bm25 = self.bm25_index.as_ref().ok_or_else(|| {
            FerresError::Storage(
                "hybrid search requires BM25 index enabled for this collection".to_string(),
            )
        })?;

        let k_expanded = (limit * 3).max(50).min(self.points.len().max(1));
        let vec_results = self.search(query_vector, k_expanded, None, None)?;
        let bm25_results = bm25.search(query_text, k_expanded);

        let combined = match strategy {
            FusionStrategy::WeightedScore { alpha } => {
                fusion::weighted_fusion(&vec_results, &bm25_results, *alpha, DEFAULT_RRF_K, limit)
            }
            FusionStrategy::RRF { k } => {
                fusion::reciprocal_rank_fusion(&[vec_results, bm25_results], *k, limit)
            }
        };

        Ok(combined)
    }

    /// Remove os dados de um ponto do HashMap sem tombstoning no índice HNSW.
    ///
    /// Usado pelo tiered storage para liberar RAM de pontos demovidos
    /// enquanto mantém o nó no grafo HNSW. O HNSW continua retornando
    /// o ID do ponto nos resultados de busca, e a hidratação resolve
    /// os dados de Warm (mmap) ou Cold (disco).
    ///
    /// **Não use para exclusão real** — use [`remove`] para isso, que
    /// também tombstona no HNSW e impede que o ponto apareça em buscas.
    pub fn remove_data_only(&mut self, id: &str) -> Option<Point> {
        let point = self.points.remove(id);
        if point.is_some() {
            // Remove do BM25 mas NÃO do HNSW
            if let Some(ref mut bm25) = self.bm25_index {
                bm25.remove_document(id);
            }
            self.invalidate_search_cache();
            self.dirty.store(true, Ordering::Release);
        }
        point
    }

    /// Tombstona um ponto no índice HNSW sem remover do HashMap.
    ///
    /// Usado pelo tiered storage ao promover um ponto de volta para Hot:
    /// o nó antigo no HNSW é tombstonado antes de inserir o nó atualizado,
    /// evitando duplicatas no grafo.
    pub fn tombstone_in_index(&mut self, id: &str) {
        self.index.remove_point(id);
    }

    /// Remove um ponto pelo ID (da coleção e do índice).
    pub fn remove(&mut self, id: &str) -> Result<(), FerresError> {
        if self.points.remove(id).is_none() {
            return Err(FerresError::PointNotFound(id.to_string()));
        }
        self.index.remove_point(id);
        for idx in self.vector_indices.values_mut() {
            idx.remove_point(id);
        }
        if let Some(ref mut bm25) = self.bm25_index {
            bm25.remove_document(id);
        }
        self.invalidate_search_cache();
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    /// Adiciona uma relação entre dois pontos (grafo não direcionado).
    ///
    /// Atualiza a lista `relations` em ambos os pontos: `from_id` ganha `to_id`
    /// e `to_id` ganha `from_id`. Os IDs devem ser as chaves de armazenamento
    /// (storage_id), ou seja, o mesmo usado em `get(id)`.
    pub fn add_relation(&mut self, from_id: &str, to_id: &str) -> Result<(), FerresError> {
        if from_id == to_id {
            return Err(FerresError::InvalidPointId(
                "from and to must be different points".into(),
            ));
        }
        let mut from_point = self
            .points
            .get(from_id)
            .ok_or_else(|| FerresError::PointNotFound(from_id.to_string()))?
            .clone();
        let mut to_point = self
            .points
            .get(to_id)
            .ok_or_else(|| FerresError::PointNotFound(to_id.to_string()))?
            .clone();

        fn ensure_contains(relations: &mut Option<Vec<String>>, id: &str) {
            let list = relations.get_or_insert_with(Vec::new);
            if !list.contains(&id.to_string()) {
                list.push(id.to_string());
            }
        }
        ensure_contains(&mut from_point.relations, to_id);
        ensure_contains(&mut to_point.relations, from_id);

        self.points.insert(from_id.to_string(), from_point);
        self.points.insert(to_id.to_string(), to_point);
        self.invalidate_search_cache();
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    /// Remove múltiplos pontos por ID em batch.
    ///
    /// IDs são ordenados para melhor cache locality. Retorna o número de
    /// pontos efetivamente removidos (IDs inexistentes são ignorados).
    pub fn delete_points_batch(&mut self, ids: &[String]) -> Result<usize, FerresError> {
        let mut sorted_ids = ids.to_vec();
        sorted_ids.sort_unstable();

        let mut deleted = 0;
        for id in &sorted_ids {
            if self.points.remove(id).is_some() {
                self.index.remove_point(id);
                for idx in self.vector_indices.values_mut() {
                    idx.remove_point(id);
                }
                if let Some(ref mut bm25) = self.bm25_index {
                    bm25.remove_document(id);
                }
                deleted += 1;
            }
        }

        if deleted > 0 {
            self.invalidate_search_cache();
            self.dirty.store(true, Ordering::Release);
        }
        Ok(deleted)
    }

    /// Remove pontos cujo TTL expirou (`expires_at < now`).
    ///
    /// Retorna o número de pontos removidos. Pontos sem `expires_at` (None) nunca expiram.
    pub fn vacuum_expired_points(&mut self) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs();
        let expired: Vec<String> = self
            .points
            .iter()
            .filter(|(_, p)| p.expires_at.map(|e| e < now).unwrap_or(false))
            .map(|(id, _)| id.clone())
            .collect();
        let count = expired.len();
        for id in &expired {
            self.points.remove(id);
            self.index.remove_point(id);
            for idx in self.vector_indices.values_mut() {
                idx.remove_point(id);
            }
            if let Some(ref mut bm25) = self.bm25_index {
                bm25.remove_document(id);
            }
        }
        if count > 0 {
            self.invalidate_search_cache();
            self.dirty.store(true, Ordering::Release);
        }
        count
    }

    /// Recupera um ponto pelo ID.
    pub fn get(&self, id: &str) -> Option<&Point> {
        self.points.get(id)
    }

    /// Retorna todos os pontos como owned (para serialização/storage).
    pub fn points_owned(&self) -> Vec<Point> {
        self.points.values().cloned().collect()
    }

    /// Nome da coleção.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Configuração da coleção.
    pub fn config(&self) -> &CollectionConfig {
        &self.config
    }

    /// Define o período de retenção em dias (None = manter indefinidamente).
    /// Persistência: chamar `FileStorage::save_collection` após alterar para gravar config.json.
    pub fn set_retention_days(&mut self, days: Option<u32>) {
        self.config.retention_days = days;
    }

    /// Valor atual de ef_search usado nas buscas (pode estar auto-ajustado).
    pub fn current_hnsw_ef_search(&self) -> usize {
        self.index.current_ef_search()
    }

    /// Define ef_search em runtime (para auto-tune). Aplica ao índice principal e aos índices de vetores nomeados.
    pub fn set_hnsw_ef_search(&self, v: usize) {
        self.index.set_ef_search(v);
        for idx in self.vector_indices.values() {
            idx.set_ef_search(v);
        }
    }

    /// Ajusta ef_search dinamicamente com base na latência P95 observada (Auto-Tune FerresEngine).
    ///
    /// - Latência muito baixa e recall prioridade → aumenta ef_search (melhor recall).
    /// - Latência alta (proxy para CPU sob estresse) → diminui ef_search (menor carga).
    pub fn apply_hnsw_auto_tune(&self, p95_latency_ms: f64, recall_priority: bool) {
        const EF_MIN: usize = 10;
        const EF_MAX: usize = 200;
        const STEP: usize = 10;
        const P95_LOW_MS: f64 = 10.0;
        const P95_HIGH_MS: f64 = 50.0;

        let current = self.index.current_ef_search();
        let base = self.config.hnsw.ef_search;
        let (min_ef, max_ef) = ((base / 2).max(EF_MIN), (base * 2).min(EF_MAX));

        let new_ef = if p95_latency_ms < P95_LOW_MS && recall_priority && current < max_ef {
            (current + STEP).min(max_ef)
        } else if p95_latency_ms > P95_HIGH_MS && current > min_ef {
            current.saturating_sub(STEP).max(min_ef)
        } else {
            current
        };

        if new_ef != current {
            self.set_hnsw_ef_search(new_ef);
            tracing::debug!(
                collection = %self.config.name,
                p95_ms = p95_latency_ms,
                previous_ef = current,
                new_ef,
                "hnsw auto-tune applied"
            );
        }
    }

    /// Número de pontos na coleção.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Estatísticas do search_cache para cálculo de Cache Hit Rate.
    /// Retorna (hits, misses). Hit rate % = hits / (hits + misses) * 100 quando total > 0.
    pub fn search_cache_stats(&self) -> (u64, u64) {
        (
            self.search_cache_hits.load(Ordering::Relaxed),
            self.search_cache_misses.load(Ordering::Relaxed),
        )
    }

    /// Returns the number of tombstoned points in the underlying ANN index.
    ///
    /// Tombstones accumulate when points are deleted and degrade search
    /// performance. Use [`crate::reindex::needs_reindex`] to check if a
    /// background reindex is recommended.
    pub fn tombstone_count(&self) -> usize {
        self.index.tombstone_count()
    }

    /// Total number of entries in the index (live points + tombstones).
    ///
    /// Use with [`crate::reindex::needs_reindex`] as the `total_indexed`
    /// argument: `needs_reindex(coll.tombstone_count(), coll.total_indexed_len())`.
    pub fn total_indexed_len(&self) -> usize {
        self.len() + self.tombstone_count()
    }

    /// Returns estimated memory (bytes) wasted by tombstoned points until the next reindex.
    ///
    /// Non-zero only for quantized index; non-quantized backends return 0.
    pub fn tombstone_memory_waste(&self) -> usize {
        self.index.tombstone_memory_waste()
    }

    /// Takes a snapshot of the current points for background reindexing.
    ///
    /// Returns `(owned_points, set_of_ids)`. The caller builds a new index
    /// from the owned points and later uses the ID set to compute the delta.
    ///
    /// **Memory note:** Reindex temporarily requires approximately 2× the
    /// collection's memory: one copy for the existing index (serving queries)
    /// and one for the new index being built from the snapshot.
    /// For a 1M × 384 collection (~1.5 GB), expect ~3 GB peak usage.
    pub fn points_snapshot(&self) -> (Vec<Point>, std::collections::HashSet<String>) {
        let points: Vec<Point> = self.points.values().cloned().collect();
        let ids: std::collections::HashSet<String> = self.points.keys().cloned().collect();
        (points, ids)
    }

    /// Swaps the current ANN index with a new one.
    ///
    /// Used during background reindex: the new index was built from a
    /// snapshot and has no tombstones. The old index is dropped.
    pub fn swap_index(&mut self, new_index: Box<dyn ANNIndex>) {
        self.index = new_index;
        self.invalidate_search_cache();
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Marca a coleção como dirty (modificada).
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Verifica se a coleção está dirty (precisa ser salva).
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    /// Marca a coleção como limpa (salva).
    pub fn mark_clean(&self) {
        self.dirty.store(false, Ordering::Release);
    }
}

impl Drop for Collection {
    /// Limpa recursos da coleção ao ser descartada.
    ///
    /// Garante que o índice HNSW seja limpo adequadamente.
    fn drop(&mut self) {
        debug!(name = %self.config.name, "dropping collection");
        // Limpa cache
        if let Some(cache) = &self.search_cache {
            if let Ok(mut cache_guard) = cache.lock() {
                cache_guard.clear();
            }
        }
        // Limpa pontos
        self.points.clear();
        // O índice será descartado automaticamente via Drop
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CollectionConfig {
        CollectionConfig {
            name: "test".to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        }
    }

    fn make_point(id: &str, vector: Vec<f32>) -> Point {
        Point::new(id, vector, serde_json::Value::Null).unwrap()
    }

    #[test]
    fn insert_and_get() {
        let mut col = Collection::new(test_config());
        let point = make_point("p1", vec![1.0, 2.0, 3.0]);

        col.insert(point).unwrap();
        assert_eq!(col.len(), 1);

        let retrieved = col.get("p1").unwrap();
        assert_eq!(retrieved.vector, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn validate_dimension_correct() {
        let col = Collection::new(test_config());
        assert!(col.validate_dimension(&[1.0, 2.0, 3.0]).is_ok());
    }

    #[test]
    fn validate_dimension_mismatch() {
        let col = Collection::new(test_config());
        let result = col.validate_dimension(&[1.0, 2.0]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected 3"));
        assert!(err.to_string().contains("got 2"));
    }

    #[test]
    fn dimension_mismatch_rejected_on_insert() {
        let mut col = Collection::new(test_config());
        let point = make_point("p1", vec![1.0, 2.0]); // dimensão 2, esperado 3
        let result = col.insert(point);
        assert!(result.is_err());
    }

    #[test]
    fn search_returns_results() {
        let mut col = Collection::new(test_config());
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        col.insert(make_point("c", vec![0.9, 0.1, 0.0])).unwrap();

        let results = col.search(&[1.0, 0.0, 0.0], 2, None, None).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn insert_batch_small() {
        let mut col = Collection::new(test_config());
        let points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.0, 0.0, 1.0]),
        ];

        let result = col.insert_batch(points).unwrap();
        assert_eq!(result.inserted, 3);
        assert_eq!(col.len(), 3);

        // Verifica que os pontos foram inseridos corretamente
        assert!(col.get("a").is_some());
        assert!(col.get("b").is_some());
        assert!(col.get("c").is_some());
    }

    #[test]
    fn insert_batch_empty() {
        let mut col = Collection::new(test_config());
        let result = col.insert_batch(vec![]).unwrap();
        assert_eq!(result.inserted, 0);
        assert_eq!(col.len(), 0);
    }

    #[test]
    fn insert_batch_dimension_mismatch() {
        let mut col = Collection::new(test_config());
        let points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0]), // dimensão incorreta
        ];

        let result = col.insert_batch(points);
        assert!(result.is_err());
    }

    #[test]
    fn insert_batch_searchable() {
        let mut col = Collection::new(test_config());
        let points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.9, 0.1, 0.0]),
        ];

        col.insert_batch(points).unwrap();

        // Verifica que os pontos são buscáveis
        let results = col.search(&[1.0, 0.0, 0.0], 2, None, None).unwrap();
        assert_eq!(results.len(), 2);
        // O mais próximo de [1,0,0] deve ser "a"
        assert_eq!(results[0].0, "a");
    }

    #[test]
    fn insert_batch_large_rebuild() {
        // Testa o caminho de rebuild para batches grandes (>100)
        let config = CollectionConfig {
            name: "large_batch_test".to_string(),
            dimension: 8,
            distance: DistanceMetric::Cosine,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        };
        let mut col = Collection::new(config);

        // Cria 150 pontos para forçar o caminho de rebuild
        let points: Vec<Point> = (0..150)
            .map(|i| {
                let mut vector = vec![0.0; 8];
                vector[i % 8] = 1.0;
                make_point(&format!("p{i}"), vector)
            })
            .collect();

        let result = col.insert_batch(points).unwrap();
        assert_eq!(result.inserted, 150);
        assert_eq!(col.len(), 150);

        // Verifica que os pontos são buscáveis após rebuild
        let results = col
            .search(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 5, None, None)
            .unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn insert_batch_incremental_then_large() {
        // Testa inserção incremental seguida de batch grande
        let mut col = Collection::new(test_config());

        // Primeiro, insere alguns pontos individuais
        col.insert(make_point("existing1", vec![1.0, 0.0, 0.0]))
            .unwrap();
        col.insert(make_point("existing2", vec![0.0, 1.0, 0.0]))
            .unwrap();
        assert_eq!(col.len(), 2);

        // Depois, insere um batch pequeno
        let small_batch = vec![
            make_point("batch1", vec![0.0, 0.0, 1.0]),
            make_point("batch2", vec![0.5, 0.5, 0.0]),
        ];
        let result = col.insert_batch(small_batch).unwrap();
        assert_eq!(result.inserted, 2);
        assert_eq!(col.len(), 4);

        // Verifica que todos os pontos são acessíveis
        assert!(col.get("existing1").is_some());
        assert!(col.get("existing2").is_some());
        assert!(col.get("batch1").is_some());
        assert!(col.get("batch2").is_some());
    }

    #[test]
    fn search_validates_query_dimension() {
        let col = Collection::new(test_config());
        let result = col.search(&[1.0, 2.0], 5, None, None); // dimensão 2, esperado 3
        assert!(result.is_err());
    }

    #[test]
    fn remove_point_works() {
        let mut col = Collection::new(test_config());
        col.insert(make_point("rm", vec![1.0, 0.0, 0.0])).unwrap();
        assert_eq!(col.len(), 1);

        col.remove("rm").unwrap();
        assert_eq!(col.len(), 0);
        assert!(col.get("rm").is_none());
    }

    #[test]
    fn remove_nonexistent_returns_error() {
        let mut col = Collection::new(test_config());
        let result = col.remove("ghost");
        assert!(result.is_err());
    }

    #[test]
    fn add_relation_updates_both_points() {
        let mut col = Collection::new(test_config());
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();

        col.add_relation("a", "b").unwrap();

        let pa = col.get("a").unwrap();
        let pb = col.get("b").unwrap();
        assert_eq!(pa.relations.as_deref(), Some(&["b".to_string()][..]));
        assert_eq!(pb.relations.as_deref(), Some(&["a".to_string()][..]));
    }

    #[test]
    fn add_relation_same_id_returns_error() {
        let mut col = Collection::new(test_config());
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        let result = col.add_relation("a", "a");
        assert!(result.is_err());
    }

    #[test]
    fn add_relation_nonexistent_returns_error() {
        let mut col = Collection::new(test_config());
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        let result = col.add_relation("a", "ghost");
        assert!(result.is_err());
    }

    #[test]
    fn search_connected_returns_similar_within_hops() {
        let mut col = Collection::new(test_config());
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.9, 0.1, 0.0])).unwrap();
        col.insert(make_point("c", vec![0.0, 0.0, 1.0])).unwrap();
        col.add_relation("a", "b").unwrap();
        col.add_relation("a", "c").unwrap();
        let results = col.search_connected(&[1.0, 0.0, 0.0], "a", 1, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a");
        assert_eq!(results[1].0, "b");
    }

    #[test]
    fn vacuum_expired_points_removes_only_expired() {
        let mut col = Collection::new(test_config());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let mut p_expired = make_point("exp1", vec![1.0, 0.0, 0.0]);
        p_expired.expires_at = Some(1); // past
        let mut p_future = make_point("exp2", vec![0.0, 1.0, 0.0]);
        p_future.expires_at = Some(now + 3600);
        let p_no_ttl = make_point("exp3", vec![0.0, 0.0, 1.0]);
        col.insert(p_expired).unwrap();
        col.insert(p_future).unwrap();
        col.insert(p_no_ttl).unwrap();
        assert_eq!(col.len(), 3);

        let removed = col.vacuum_expired_points();
        assert_eq!(removed, 1);
        assert_eq!(col.len(), 2);
        assert!(col.get("exp1").is_none());
        assert!(col.get("exp2").is_some());
        assert!(col.get("exp3").is_some());
    }

    #[test]
    fn collection_config_serialization() {
        let config = test_config();
        let json = serde_json::to_string(&config).unwrap();
        let restored: CollectionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.dimension, 3);
        assert_eq!(restored.distance, DistanceMetric::Euclidean);
    }

    #[test]
    fn hybrid_search_requires_bm25() {
        let col = Collection::new(test_config());
        let result = col.hybrid_search(&[1.0, 2.0, 3.0], "query", 5, &FusionStrategy::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("BM25"));
    }

    #[test]
    fn hybrid_search_returns_fused_results() {
        let config = CollectionConfig {
            name: "hybrid_test".to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: true,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        };
        let mut col = Collection::new(config);
        col.insert(
            Point::new(
                "a",
                vec![1.0, 0.0, 0.0],
                serde_json::json!({"text": "hello world"}),
            )
            .unwrap(),
        )
        .unwrap();
        col.insert(
            Point::new(
                "b",
                vec![0.0, 1.0, 0.0],
                serde_json::json!({"text": "foo bar"}),
            )
            .unwrap(),
        )
        .unwrap();
        col.insert(
            Point::new(
                "c",
                vec![0.9, 0.1, 0.0],
                serde_json::json!({"text": "hello foo"}),
            )
            .unwrap(),
        )
        .unwrap();
        let results = col
            .hybrid_search(&[1.0, 0.0, 0.0], "hello", 3, &FusionStrategy::default())
            .unwrap();
        assert!(!results.is_empty());
        assert!(results.len() <= 3);
    }

    // ─── Property Tests com QuickCheck ─────────────────────────────────

    #[cfg(test)]
    mod prop_tests {
        use super::*;
        use quickcheck::TestResult;
        use quickcheck_macros::quickcheck;

        /// Propriedade: inserir um ponto e depois buscá-lo deve retornar o ponto.
        #[quickcheck]
        fn prop_insert_then_search_returns_point(
            id: String,
            vector: Vec<f32>,
            k: usize,
        ) -> TestResult {
            if id.is_empty() || vector.is_empty() || vector.len() > 1000 || k == 0 {
                return TestResult::discard();
            }

            let dimension = vector.len();
            let config = CollectionConfig {
                name: "prop_test".to_string(),
                dimension,
                distance: DistanceMetric::Euclidean,
                hnsw: HnswConfig::default(),
                search_cache_size: 0, // Desabilita cache para testes determinísticos
                enable_bm25: false,
                bm25_text_field: "text".to_string(),
                quantization: QuantizationConfig::default(),
                tiered_storage: TieredStorageConfig::default(),
                retention_days: None,
            };

            let mut col = Collection::new(config);
            let point = match Point::new(id.clone(), vector.clone(), serde_json::Value::Null) {
                Ok(p) => p,
                Err(_) => return TestResult::discard(),
            };

            if col.insert(point).is_err() {
                return TestResult::discard();
            }

            let results = match col.search(&vector, k.min(col.len()), None, None) {
                Ok(r) => r,
                Err(_) => return TestResult::discard(),
            };

            // O ponto inserido deve aparecer nos resultados
            let found = results.iter().any(|(result_id, _)| result_id == &id);
            TestResult::from_bool(found || k == 0 || col.is_empty())
        }

        /// Propriedade: o número de pontos na coleção deve ser igual ao número de inserções.
        #[quickcheck]
        fn prop_collection_length_matches_insertions(
            points: Vec<(String, Vec<f32>)>,
        ) -> TestResult {
            if points.is_empty() || points.len() > 100 {
                return TestResult::discard();
            }

            // Todos os pontos devem ter a mesma dimensão
            let dimension = match points.first() {
                Some((_, v)) => v.len(),
                None => return TestResult::discard(),
            };

            if dimension == 0 || dimension > 1000 {
                return TestResult::discard();
            }

            if !points.iter().all(|(_, v)| v.len() == dimension) {
                return TestResult::discard();
            }

            let config = CollectionConfig {
                name: "prop_test".to_string(),
                dimension,
                distance: DistanceMetric::Euclidean,
                hnsw: HnswConfig::default(),
                search_cache_size: 0,
                enable_bm25: false,
                bm25_text_field: "text".to_string(),
                quantization: QuantizationConfig::default(),
                tiered_storage: TieredStorageConfig::default(),
                retention_days: None,
            };

            let mut col = Collection::new(config);
            let mut inserted_count = 0;

            for (id, vector) in points {
                if id.is_empty() {
                    continue;
                }
                let point = match Point::new(id, vector, serde_json::Value::Null) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if col.insert(point).is_ok() {
                    inserted_count += 1;
                }
            }

            TestResult::from_bool(col.len() == inserted_count)
        }

        /// Propriedade: remover um ponto deve diminuir o tamanho da coleção.
        #[quickcheck]
        fn prop_remove_decreases_length(
            points: Vec<(String, Vec<f32>)>,
            remove_id: String,
        ) -> TestResult {
            if points.is_empty() || points.len() > 50 || remove_id.is_empty() {
                return TestResult::discard();
            }

            let dimension = match points.first() {
                Some((_, v)) => v.len(),
                None => return TestResult::discard(),
            };

            if dimension == 0 || dimension > 100 {
                return TestResult::discard();
            }

            if !points.iter().all(|(_, v)| v.len() == dimension) {
                return TestResult::discard();
            }

            let config = CollectionConfig {
                name: "prop_test".to_string(),
                dimension,
                distance: DistanceMetric::Euclidean,
                hnsw: HnswConfig::default(),
                search_cache_size: 0,
                enable_bm25: false,
                bm25_text_field: "text".to_string(),
                quantization: QuantizationConfig::default(),
                tiered_storage: TieredStorageConfig::default(),
                retention_days: None,
            };

            let mut col = Collection::new(config);

            // Insere pontos
            for (id, vector) in points {
                if id.is_empty() {
                    continue;
                }
                if let Ok(point) = Point::new(id, vector, serde_json::Value::Null) {
                    let _ = col.insert(point);
                }
            }

            let len_before = col.len();
            if len_before == 0 {
                return TestResult::discard();
            }

            // Tenta remover
            let removed = col.remove(&remove_id).is_ok();
            let len_after = col.len();

            if removed {
                TestResult::from_bool(len_after == len_before - 1)
            } else {
                // Se não removeu, o tamanho deve ser o mesmo
                TestResult::from_bool(len_after == len_before)
            }
        }

        /// Propriedade: buscar com k maior que o número de pontos deve retornar no máximo o número de pontos.
        #[quickcheck]
        fn prop_search_k_limited_by_collection_size(
            points: Vec<(String, Vec<f32>)>,
            k: usize,
        ) -> TestResult {
            if points.is_empty() || points.len() > 50 {
                return TestResult::discard();
            }

            let dimension = match points.first() {
                Some((_, v)) => v.len(),
                None => return TestResult::discard(),
            };

            if dimension == 0 || dimension > 100 {
                return TestResult::discard();
            }

            if !points.iter().all(|(_, v)| v.len() == dimension) {
                return TestResult::discard();
            }

            let config = CollectionConfig {
                name: "prop_test".to_string(),
                dimension,
                distance: DistanceMetric::Euclidean,
                hnsw: HnswConfig::default(),
                search_cache_size: 0,
                enable_bm25: false,
                bm25_text_field: "text".to_string(),
                quantization: QuantizationConfig::default(),
                tiered_storage: TieredStorageConfig::default(),
                retention_days: None,
            };

            let mut col = Collection::new(config);

            // Insere pontos
            for (id, vector) in points {
                if id.is_empty() {
                    continue;
                }
                if let Ok(point) = Point::new(id, vector, serde_json::Value::Null) {
                    let _ = col.insert(point);
                }
            }

            if col.is_empty() {
                return TestResult::discard();
            }

            let query = vec![0.0; dimension];
            let results = match col.search(&query, k, None, None) {
                Ok(r) => r,
                Err(_) => return TestResult::discard(),
            };

            TestResult::from_bool(results.len() <= col.len())
        }
    }
}
