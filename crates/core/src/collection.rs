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

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use lru::LruCache;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::bm25::BM25Index;
use crate::error::FerresError;
use crate::point::Point;
use crate::search::{ANNIndex, DistanceMetric, HnswConfig, HnswIndex};

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
}

fn default_cache_size() -> usize {
    100
}

fn default_bm25_text_field() -> String {
    "text".to_string()
}

// ─── Collection ─────────────────────────────────────────────────────

/// Chave de cache para resultados de busca.
/// Usa hash do vetor de query e k para identificar queries únicas.
#[derive(Debug, Clone)]
struct CacheKey {
    query_hash: u64,
    k: usize,
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.query_hash.hash(state);
        self.k.hash(state);
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.query_hash == other.query_hash && self.k == other.k
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

/// Constante RRF (Reciprocal Rank Fusion). Típico: 60.
const RRF_K: u32 = 60;

/// Uma coleção de pontos vetoriais com índice de busca ANN.
pub struct Collection {
    config: CollectionConfig,
    points: HashMap<String, Point>,
    index: Box<dyn ANNIndex>,
    /// Índice BM25 opcional para busca híbrida.
    bm25_index: Option<BM25Index>,
    /// Cache LRU opcional para resultados de busca.
    /// Mutex é necessário porque search() é &self mas precisa mutar o cache.
    #[allow(dead_code)]
    search_cache: Option<Mutex<LruCache<CacheKey, Vec<(String, f32)>>>>,
    /// Flag indicando se a coleção foi modificada e precisa ser salva.
    dirty: AtomicBool,
}

impl Collection {
    /// Cria uma coleção vazia usando `HnswIndex` como backend padrão.
    pub fn new(config: CollectionConfig) -> Self {
        let index = Box::new(HnswIndex::new(config.distance, config.hnsw.clone()));
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
            bm25_index,
            search_cache,
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
            bm25_index,
            search_cache,
            dirty: AtomicBool::new(false),
        }
    }

    /// Reconstrói uma coleção a partir de pontos carregados do disco.
    ///
    /// Re-indexa todos os pontos via `build`. Usado no startup do
    /// server ao carregar coleções persistidas.
    pub fn from_points(config: CollectionConfig, points: Vec<Point>) -> Result<Self, FerresError> {
        let mut collection = Self::new(config);
        collection.index.build(&points)?;
        for point in &points {
            collection.points.insert(point.id.clone(), point.clone());
        }
        if let Some(ref mut bm25) = collection.bm25_index {
            let field = &collection.config.bm25_text_field;
            for point in collection.points.values() {
                let text = metadata_text(&point.metadata, field);
                bm25.index_document(&point.id, &text);
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
    /// });
    ///
    /// let point = Point::new("p1", vec![1.0, 2.0, 3.0], serde_json::json!(null))?;
    /// collection.insert(point)?;
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    pub fn insert(&mut self, point: Point) -> Result<(), FerresError> {
        self.validate_dimension(&point.vector)?;
        self.index.add_point(&point)?;
        if let Some(ref mut bm25) = self.bm25_index {
            let text = metadata_text(&point.metadata, &self.config.bm25_text_field);
            bm25.index_document(&point.id, &text);
        }
        self.points.insert(point.id.clone(), point);
        self.invalidate_search_cache();
        self.dirty.store(true, Ordering::Release);
        Ok(())
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
    /// });
    ///
    /// collection.insert(Point::new("p1", vec![1.0, 0.0, 0.0], serde_json::json!(null))?)?;
    /// collection.insert(Point::new("p2", vec![0.0, 1.0, 0.0], serde_json::json!(null))?)?;
    ///
    /// let results = collection.search(&[1.0, 0.0, 0.0], 2)?;
    /// assert_eq!(results.len(), 2);
    /// # Ok::<(), ferres_db_core::FerresError>(())
    /// ```
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        self.validate_dimension(query)?;

        // Calcula hash do query para usar como chave de cache
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        query.iter().for_each(|x| {
            x.to_bits().hash(&mut hasher);
        });
        let query_hash = hasher.finish();
        let cache_key = CacheKey { query_hash, k };

        // Verifica cache se habilitado
        if let Some(cache) = &self.search_cache {
            if let Ok(mut cache_guard) = cache.lock() {
                if let Some(cached_results) = cache_guard.get(&cache_key) {
                    return Ok(cached_results.clone());
                }
            }
        }

        // Executa busca
        let results = self.index.search(query, k)?;

        // Armazena no cache se habilitado
        if let Some(cache) = &self.search_cache {
            if let Ok(mut cache_guard) = cache.lock() {
                cache_guard.put(cache_key, results.clone());
            }
        }

        Ok(results)
    }

    /// Busca híbrida: combina resultados vetoriais e BM25 via RRF ponderado por `alpha`.
    ///
    /// Requer que a coleção tenha BM25 habilitado (`enable_bm25: true`).
    /// `alpha` em [0, 1]: peso da busca vetorial; (1 - alpha) é o peso da busca keyword.
    pub fn hybrid_search(
        &self,
        query_vector: &[f32],
        query_text: &str,
        k: usize,
        alpha: f32,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        self.validate_dimension(query_vector)?;
        let bm25 = self
            .bm25_index
            .as_ref()
            .ok_or_else(|| FerresError::Storage("hybrid search requires BM25 index enabled for this collection".to_string()))?;

        let k_expanded = (k * 3).max(50).min(self.points.len().max(1));
        let vec_results = self.search(query_vector, k_expanded)?;
        let bm25_results = bm25.search(query_text, k_expanded);

        let k_rrf = RRF_K as f32;
        let mut rank_vec: HashMap<String, u32> = HashMap::new();
        for (rank, (id, _)) in vec_results.iter().enumerate() {
            rank_vec.insert(id.clone(), rank as u32 + 1);
        }
        let mut rank_bm25: HashMap<String, u32> = HashMap::new();
        for (rank, (id, _)) in bm25_results.iter().enumerate() {
            rank_bm25.insert(id.clone(), rank as u32 + 1);
        }

        let all_ids: HashSet<_> = rank_vec
            .keys()
            .chain(rank_bm25.keys())
            .cloned()
            .collect();
        let mut combined: Vec<(String, f32)> = all_ids
            .into_iter()
            .map(|id| {
                let rv = rank_vec.get(&id).copied().unwrap_or(u32::MAX);
                let rb = rank_bm25.get(&id).copied().unwrap_or(u32::MAX);
                let score = alpha * (1.0 / (k_rrf + rv as f32))
                    + (1.0 - alpha) * (1.0 / (k_rrf + rb as f32));
                (id, score)
            })
            .collect();
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(k);
        Ok(combined)
    }

    /// Remove um ponto pelo ID (da coleção e do índice).
    pub fn remove(&mut self, id: &str) -> Result<(), FerresError> {
        if self.points.remove(id).is_none() {
            return Err(FerresError::PointNotFound(id.to_string()));
        }
        self.index.remove_point(id);
        if let Some(ref mut bm25) = self.bm25_index {
            bm25.remove_document(id);
        }
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

    /// Número de pontos na coleção.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
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

        let results = col.search(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_validates_query_dimension() {
        let col = Collection::new(test_config());
        let result = col.search(&[1.0, 2.0], 5); // dimensão 2, esperado 3
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
        let result = col.hybrid_search(&[1.0, 2.0, 3.0], "query", 5, 0.5);
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
            .hybrid_search(&[1.0, 0.0, 0.0], "hello", 3, 0.5)
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
            };

            let mut col = Collection::new(config);
            let point = match Point::new(id.clone(), vector.clone(), serde_json::Value::Null) {
                Ok(p) => p,
                Err(_) => return TestResult::discard(),
            };

            if col.insert(point).is_err() {
                return TestResult::discard();
            }

            let results = match col.search(&vector, k.min(col.len())) {
                Ok(r) => r,
                Err(_) => return TestResult::discard(),
            };

            // O ponto inserido deve aparecer nos resultados
            let found = results.iter().any(|(result_id, _)| result_id == &id);
            TestResult::from_bool(found || k == 0 || col.len() == 0)
        }

        /// Propriedade: o número de pontos na coleção deve ser igual ao número de inserções.
        #[quickcheck]
        fn prop_collection_length_matches_insertions(points: Vec<(String, Vec<f32>)>) -> TestResult {
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
            let results = match col.search(&query, k) {
                Ok(r) => r,
                Err(_) => return TestResult::discard(),
            };

            TestResult::from_bool(results.len() <= col.len())
        }
    }
}
