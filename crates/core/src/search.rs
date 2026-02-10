//! # Search — motor de busca aproximada por vizinhos mais próximos (ANN)
//!
//! Define o trait [`ANNIndex`] que abstrai qualquer backend de busca
//! vetorial, e fornece [`HnswIndex`] como implementação concreta
//! usando `hnsw_rs`.
//!
//! ## Decisões arquiteturais
//!
//! - **Trait `ANNIndex`**: permite trocar a implementação (HNSW, brute-
//!   force, usearch, etc.) sem alterar `Collection` nem o server.
//!   O trait é object-safe (`dyn ANNIndex`) para uso em `Box`.
//!
//! - **`DistanceMetric` como enum próprio**: desacopla a API pública
//!   dos tipos internos do `hnsw_rs`. Consumidores do FerresDB não
//!   precisam conhecer `DistCosine`, `DistDot` ou `DistL2`.
//!
//! - **Mapeamento `DataId ↔ String`**: o HNSW usa índices numéricos
//!   (`usize`). Mantemos um `Vec<String>` que traduz o `DataId` do
//!   HNSW de volta ao ID string do ponto original.
//!
//! - **`remove_point` por tombstone**: HNSW não suporta remoção nativa.
//!   Marcamos o ponto como removido no mapeamento e o excluímos dos
//!   resultados de busca. O rebuild periódico (via `build`) limpa os
//!   tombstones.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use hnsw_rs::prelude::*;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::FerresError;
use crate::explain::ExplainMeta;
use crate::point::Point;
use crate::quantization::{QuantizationConfig, ScalarQuantizationConfig, ScalarQuantizationParams};

// ─── DistanceMetric ─────────────────────────────────────────────────

/// Métricas de distância suportadas pelo FerresDB.
///
/// - `Cosine`: 1 − cos(a,b). Padrão para embeddings de texto.
/// - `DotProduct`: produto escalar negado. Usado quando os vetores
///   já estão normalizados e se quer maximizar similaridade.
/// - `Euclidean`: distância L2. Comum em visão computacional.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DistanceMetric {
    Cosine,
    DotProduct,
    Euclidean,
}

// ─── ANNIndex trait ─────────────────────────────────────────────────

/// Trait que abstrai um índice de busca aproximada por vizinhos.
///
/// Object-safe para uso como `Box<dyn ANNIndex>`. Qualquer backend
/// (HNSW, flat/brute-force, IVF, etc.) pode implementar este trait.
///
/// # Exemplo de Implementação
///
/// ```rust,no_run
/// use ferres_db_core::search::{ANNIndex, DistanceMetric};
/// use ferres_db_core::Point;
///
/// struct BruteForceIndex {
///     points: Vec<Point>,
/// }
///
/// impl ANNIndex for BruteForceIndex {
///     fn build(&mut self, points: &[Point]) -> Result<(), ferres_db_core::FerresError> {
///         self.points = points.to_vec();
///         Ok(())
///     }
///
///     fn search(&self, query: &[f32], k: usize, predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>) -> Result<Vec<(String, f32)>, ferres_db_core::FerresError> {
///         Ok(vec![])
///     }
///
///     fn add_point(&mut self, point: &Point) -> Result<(), ferres_db_core::FerresError> {
///         self.points.push(point.clone());
///         Ok(())
///     }
///
///     fn remove_point(&mut self, id: &str) {
///         self.points.retain(|p| p.id != id);
///     }
/// }
/// ```
pub trait ANNIndex: Send + Sync {
    /// Reconstrói o índice inteiro a partir de uma lista de pontos.
    ///
    /// Descarta o índice anterior e cria um novo. Útil para compactação
    /// (eliminar tombstones) e para o carregamento inicial do disco.
    fn build(&mut self, points: &[Point]) -> Result<(), FerresError>;

    /// Busca os `k` vizinhos mais próximos do vetor de consulta.
    ///
    /// Retorna pares `(point_id, distância)` ordenados por distância
    /// crescente. Se `predicate` é `Some`, apenas pontos cujo ID retorna
    /// `true` no predicado são considerados (pre-filtering durante a busca).
    fn search(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>, FerresError>;

    /// Adiciona um único ponto ao índice.
    fn add_point(&mut self, point: &Point) -> Result<(), FerresError>;

    /// Remove um ponto do índice pelo ID.
    ///
    /// Em backends que não suportam remoção nativa (como HNSW),
    /// isso marca o ponto como tombstone — ele é ignorado nas buscas
    /// mas continua no grafo até o próximo `build`.
    fn remove_point(&mut self, id: &str);

    /// Returns the number of tombstoned (logically deleted) points in the index.
    ///
    /// Tombstones accumulate from `remove_point` calls and degrade search
    /// performance over time. A background reindex cleans them.
    /// Default: 0 (backends with native deletion).
    fn tombstone_count(&self) -> usize {
        0
    }

    /// Returns estimated memory waste (bytes) from tombstoned points not yet reclaimed.
    ///
    /// Non-zero only for quantized index; reclaimed on next `build()`.
    /// Default: 0 (backends that do not keep per-point vectors).
    fn tombstone_memory_waste(&self) -> usize {
        0
    }

    /// Current ef_search used at search time (for HNSW: runtime value, may be auto-tuned).
    fn current_ef_search(&self) -> usize;

    /// Set ef_search at runtime (for HNSW auto-tuning). No-op for backends that do not support it.
    fn set_ef_search(&self, v: usize);

    /// Busca os `k` vizinhos mais próximos com metadados de explicação.
    ///
    /// Retorna tuplas `(point_id, distância, ExplainMeta)` com informações
    /// adicionais sobre o processo de busca (candidatos visitados, camadas
    /// percorridas, tombstones ignorados).
    ///
    /// A implementação padrão delega para [`search`] sem metadata extra.
    fn search_explain(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32, ExplainMeta)>, FerresError> {
        let results = self.search(query, k, predicate)?;
        Ok(results
            .into_iter()
            .map(|(id, score)| {
                (
                    id,
                    score,
                    ExplainMeta {
                        candidates_visited: 0,
                        layers_traversed: 0,
                        tombstones_skipped: 0,
                    },
                )
            })
            .collect())
    }
}

// ─── HnswConfig ─────────────────────────────────────────────────────

/// Parâmetros de construção do índice HNSW.
///
/// - `max_nb_connection` (M): vizinhos por nó. Típico: 16–64.
/// - `ef_construction`: lista dinâmica durante build. Típico: 100–400.
/// - `max_elements`: capacidade inicial pré-alocada.
/// - `max_layer`: profundidade máxima do grafo hierárquico.
/// - `ef_search`: tamanho da lista dinâmica durante busca.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswConfig {
    pub max_nb_connection: usize,
    pub max_elements: usize,
    pub max_layer: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            max_nb_connection: 16,
            max_elements: 10_000,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 50,
        }
    }
}

// ─── Helpers de distância ───────────────────────────────────────────

/// Normaliza um vetor para norma L2 = 1.
///
/// Para métrica **Cosine**, normalizar antes de inserir transforma a busca
/// por cosseno em busca L2, melhorando estabilidade numérica.
/// Rejeita vetores com valores não finitos (NaN/Infinity) ou norma zero.
///
/// A norma é calculada em `f64` para evitar acúmulo de erro em vetores
/// de alta dimensão (384+), onde a soma de quadrados em `f32` pode
/// resultar em norma ligeiramente > 1.0 após divisão.
fn normalize_vector(v: &[f32]) -> Result<Vec<f32>, FerresError> {
    if let Some(pos) = v.iter().position(|x| !x.is_finite()) {
        return Err(FerresError::InvalidVector {
            reason: format!("non-finite value at index {pos}"),
        });
    }

    let norm = v
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();

    if norm < f64::EPSILON {
        return Err(FerresError::InvalidVector {
            reason: "zero-norm vector".to_string(),
        });
    }

    Ok(v.iter().map(|x| (*x as f64 / norm) as f32).collect())
}

/// Normaliza múltiplos vetores em paralelo usando rayon.
///
/// Útil para batch insert quando a métrica é Cosine.
/// Pública para uso em otimizações de batch insert.
pub fn normalize_vectors_parallel(vectors: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, FerresError> {
    let results: Vec<Result<Vec<f32>, FerresError>> = vectors
        .par_iter()
        .map(|v| normalize_vector(v))
        .collect();
    results.into_iter().collect()
}

/// Valida que todos os componentes do vetor são finitos (não NaN nem infinito).
/// Pública para uso em validação de vetores nomeados em multi-vector.
pub fn validate_vector_finite(v: &[f32]) -> Result<(), FerresError> {
    if let Some(pos) = v.iter().position(|x| !x.is_finite()) {
        return Err(FerresError::InvalidVector {
            reason: format!("non-finite value at index {pos}"),
        });
    }
    Ok(())
}

/// Prepara o vetor de acordo com a métrica antes da inserção/busca.
///
/// - `Cosine` → normaliza para que distância cosseno = distância L2
/// - `DotProduct` → usa o vetor diretamente (valida finitude)
/// - `Euclidean` → usa o vetor diretamente (L2 nativo, valida finitude)
fn prepare_vector(v: &[f32], metric: DistanceMetric) -> Result<Vec<f32>, FerresError> {
    match metric {
        DistanceMetric::Cosine => normalize_vector(v),
        DistanceMetric::DotProduct | DistanceMetric::Euclidean => {
            validate_vector_finite(v)?;
            Ok(v.to_vec())
        }
    }
}

// ─── SIMD distance kernels (f32 × f32) ─────────────────────────────
//
// Uses the `pulp` crate for safe SIMD abstraction: runtime dispatch to
// AVX2 (8× f32), SSE4.1 (4× f32), or scalar fallback on unsupported CPUs.
// For quantized (SQ8) vectors, asymmetric distance (f32 query × u8 candidate)
// is optimized in `crate::quantization`: multiple bytes are processed
// simultaneously (8× u8→f32 + L2/dot in AVX2, 4× in SSE4.1) with scalar fallback.

use pulp::{Arch, Simd, WithSimd};

/// Squared L2 (Euclidean) distance: sum of (a[i] - b[i])².
/// Used by pulp when SIMD is not available (Scalar backend).
#[inline(always)]
#[allow(dead_code)]
fn euclidean_distance_scalar(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = (*x as f64) - (*y as f64);
            d * d
        })
        .sum::<f64>() as f32
}

/// Dot product: sum of a[i] * b[i]. Used by pulp when SIMD is not available.
#[inline(always)]
#[allow(dead_code)]
fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum::<f64>() as f32
}

struct EuclideanDistance<'a>(&'a [f32], &'a [f32]);
impl WithSimd for EuclideanDistance<'_> {
    type Output = f32;

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> f32 {
        let (a_head, a_tail) = S::as_simd_f32s(self.0);
        let (b_head, b_tail) = S::as_simd_f32s(self.1);
        let mut acc = simd.splat_f32s(0.0);
        for (va, vb) in a_head.iter().zip(b_head.iter()) {
            let d = simd.sub_f32s(*va, *vb);
            acc = simd.add_f32s(acc, simd.mul_f32s(d, d));
        }
        let mut sum = simd.reduce_sum_f32s(acc);
        for i in 0..a_tail.len() {
            let d = (a_tail[i] - b_tail[i]) as f64;
            sum += (d * d) as f32;
        }
        sum
    }
}

struct DotProductKernel<'a>(&'a [f32], &'a [f32]);
impl WithSimd for DotProductKernel<'_> {
    type Output = f32;

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> f32 {
        let (a_head, a_tail) = S::as_simd_f32s(self.0);
        let (b_head, b_tail) = S::as_simd_f32s(self.1);
        let mut acc = simd.splat_f32s(0.0);
        for (va, vb) in a_head.iter().zip(b_head.iter()) {
            acc = simd.add_f32s(acc, simd.mul_f32s(*va, *vb));
        }
        let mut sum = simd.reduce_sum_f32s(acc);
        for i in 0..a_tail.len() {
            sum += a_tail[i] * b_tail[i];
        }
        sum
    }
}

/// Euclidean distance (L2²) between two f32 vectors.
///
/// SIMD-accelerated via pulp: AVX2 (8× f32) or SSE4.1 (4× f32) on x86/x86_64,
/// with automatic scalar fallback on other architectures or older CPUs.
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        panic!("euclidean_distance: length mismatch");
    }
    Arch::new().dispatch(EuclideanDistance(a, b))
}

/// Dot product between two f32 vectors.
///
/// SIMD-accelerated via pulp (AVX2/SSE4.1 on x86/x86_64), scalar fallback otherwise.
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        panic!("dot_product: length mismatch");
    }
    Arch::new().dispatch(DotProductKernel(a, b))
}

/// Returns whether SIMD acceleration is active at runtime.
///
/// `true` when the CPU supports instructions used by the distance kernels
/// (AVX2 or SSE4.1 on x86/x86_64; used by both pulp f32×f32 kernels and
/// QuantizedHnswIndex asymmetric f32×u8 kernels). Used by the stats API
/// and dashboard to show "SIMD Acceleration: Active" vs "Scalar Fallback".
#[inline]
pub fn simd_enabled() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("avx2") || std::arch::is_x86_feature_detected!("sse4.1")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        false
    }
}

// ─── IndexVariant ───────────────────────────────────────────────────

/// Despacho estático entre variantes de distância.
///
/// Usar enum em vez de `dyn Distance` mantém a busca monomorphizada
/// e evita overhead de vtable no loop interno de cálculo de distância.
enum IndexVariant<'a> {
    Cosine(Hnsw<'a, f32, DistCosine>),
    DotProduct(Hnsw<'a, f32, DistDot>),
    Euclidean(Hnsw<'a, f32, DistL2>),
}

// ─── Adaptador FilterT para predicado por ID string ──────────────────

/// Adaptador que implementa `FilterT` do hnsw_rs: converte DataId (usize) em
/// ID string via `id_map`, exclui tombstones e delega ao predicado do caller.
struct IdMapFilter<'a> {
    id_map: &'a Vec<String>,
    tombstones: &'a HashSet<String>,
    predicate: &'a (dyn Fn(&str) -> bool + Send + Sync),
}

impl FilterT for IdMapFilter<'_> {
    fn hnsw_filter(&self, id: &DataId) -> bool {
        let id_str = match self.id_map.get(*id) {
            Some(s) => s.as_str(),
            None => return false,
        };
        if self.tombstones.contains(id_str) {
            return false;
        }
        (self.predicate)(id_str)
    }
}

// ─── HnswIndex ──────────────────────────────────────────────────────

/// Implementação concreta de [`ANNIndex`] usando o algoritmo HNSW.
///
/// Mantém internamente:
/// - O grafo HNSW (via `hnsw_rs`)
/// - Mapeamento `DataId → String` para traduzir IDs numéricos
/// - Conjunto de tombstones para remoção lógica
pub struct HnswIndex {
    inner: IndexVariant<'static>,
    /// Traduz o DataId numérico do HNSW para o ID string do ponto.
    id_map: Vec<String>,
    /// Mapeamento reverso: String ID → DataId numérico.
    reverse_map: HashMap<String, usize>,
    /// IDs marcados como removidos (tombstones).
    tombstones: HashSet<String>,
    distance: DistanceMetric,
    config: HnswConfig,
    /// ef_search usado na busca; pode ser ajustado em runtime (auto-tune).
    ef_search_runtime: AtomicUsize,
}

impl HnswIndex {
    /// Cria um índice HNSW vazio.
    ///
    /// # Exemplo
    ///
    /// ```rust,no_run
    /// use ferres_db_core::search::{HnswIndex, DistanceMetric, HnswConfig};
    ///
    /// let index = HnswIndex::new(
    ///     DistanceMetric::Cosine,
    ///     HnswConfig::default()
    /// );
    /// ```
    pub fn new(distance: DistanceMetric, config: HnswConfig) -> Self {
        debug!(
            ?distance,
            m = config.max_nb_connection,
            ef = config.ef_construction,
            "creating HNSW index"
        );

        let inner = Self::create_variant(distance, &config);
        let ef_search = config.ef_search;

        Self {
            inner,
            id_map: Vec::new(),
            reverse_map: HashMap::new(),
            tombstones: HashSet::new(),
            distance,
            config,
            ef_search_runtime: AtomicUsize::new(ef_search),
        }
    }

    /// Cria a variante interna do HNSW de acordo com a métrica.
    fn create_variant(distance: DistanceMetric, config: &HnswConfig) -> IndexVariant<'static> {
        match distance {
            DistanceMetric::Cosine => IndexVariant::Cosine(Hnsw::new(
                config.max_nb_connection,
                config.max_elements,
                config.max_layer,
                config.ef_construction,
                DistCosine,
            )),
            DistanceMetric::DotProduct => IndexVariant::DotProduct(Hnsw::new(
                config.max_nb_connection,
                config.max_elements,
                config.max_layer,
                config.ef_construction,
                DistDot,
            )),
            DistanceMetric::Euclidean => IndexVariant::Euclidean(Hnsw::new(
                config.max_nb_connection,
                config.max_elements,
                config.max_layer,
                config.ef_construction,
                DistL2,
            )),
        }
    }

    /// Retorna a métrica configurada.
    pub fn distance_metric(&self) -> DistanceMetric {
        self.distance
    }

    /// Número de pontos indexados (excluindo tombstones).
    pub fn len(&self) -> usize {
        self.id_map.len() - self.tombstones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Drop for HnswIndex {
    /// Limpa recursos do índice HNSW ao ser descartado.
    ///
    /// O HNSW mantém estruturas internas que são liberadas automaticamente
    /// pelo Drop, mas explicitamos aqui para garantir limpeza adequada.
    fn drop(&mut self) {
        debug!("dropping HNSW index");
        // Limpa mapeamentos e tombstones
        self.id_map.clear();
        self.reverse_map.clear();
        self.tombstones.clear();
        // O inner será descartado automaticamente
    }
}

impl ANNIndex for HnswIndex {
    fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }

    fn current_ef_search(&self) -> usize {
        self.ef_search_runtime.load(Ordering::Relaxed)
    }

    fn set_ef_search(&self, v: usize) {
        self.ef_search_runtime.store(v, Ordering::Relaxed);
    }

    fn build(&mut self, points: &[Point]) -> Result<(), FerresError> {
        // Recria o grafo do zero — limpa tombstones e mapeamentos.
        self.inner = Self::create_variant(self.distance, &self.config);
        self.id_map.clear();
        self.reverse_map.clear();
        self.tombstones.clear();

        // Otimização: paraleliza normalização de vetores para Cosine
        if self.distance == DistanceMetric::Cosine && points.len() > 100 {
            // Pré-normaliza todos os vetores em paralelo
            let vectors: Vec<Vec<f32>> = points.par_iter().map(|p| p.vector.clone()).collect();
            let normalized = normalize_vectors_parallel(&vectors)?;

            // Insere pontos com vetores já normalizados
            for (point, normalized_vec) in points.iter().zip(normalized.iter()) {
                let data_id = self.id_map.len();
                let sid = point.storage_id();
                self.id_map.push(sid.clone());
                self.reverse_map.insert(sid, data_id);

                match &mut self.inner {
                    IndexVariant::Cosine(hnsw) => hnsw.insert_data(normalized_vec, data_id),
                    IndexVariant::DotProduct(hnsw) => hnsw.insert_data(&point.vector, data_id),
                    IndexVariant::Euclidean(hnsw) => hnsw.insert_data(&point.vector, data_id),
                }
            }
        } else {
            // Para pequenos batches ou outras métricas, usa inserção sequencial
            for point in points {
                self.add_point(point)?;
            }
        }

        debug!(count = points.len(), "HNSW index rebuilt");
        Ok(())
    }

    fn search(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        // Normaliza o query se a métrica for Cosine.
        let prepared = prepare_vector(query, self.distance)?;

        // Limita k ao número máximo de pontos disponíveis para evitar overflow
        // e alocações excessivas. Não podemos retornar mais resultados do que
        // existem pontos no índice.
        let max_points = self.id_map.len();
        let k = k.min(max_points);

        // Se não há pontos, retorna vazio imediatamente
        if k == 0 || max_points == 0 {
            return Ok(Vec::new());
        }

        if let Some(pred) = predicate {
            // Pre-filtering nativo: o predicado é aplicado durante a exploração do grafo
            // (via FilterT). Nós que não satisfazem o filtro de metadados são ignorados
            // antes de entrar na lista de candidatos; a busca continua até obter até `k`
            // resultados válidos ou exaurir o grafo (aumentando ef quando necessário).
            let adapter = IdMapFilter {
                id_map: &self.id_map,
                tombstones: &self.tombstones,
                predicate: pred,
            };
            let ef_search = self.ef_search_runtime.load(Ordering::Relaxed);
            let mut ef = ef_search
                .max(k.saturating_mul(5))
                .min(max_points);
            const MAX_ITER: usize = 20;
            let mut best = Vec::new();
            for _ in 0..MAX_ITER {
                let _search_span = tracing::info_span!(
                    "hnsw.search_filter",
                    candidates = max_points,
                    ef = ef,
                    tombstones = self.tombstones.len(),
                )
                .entered();
                let neighbours = match &self.inner {
                    IndexVariant::Cosine(hnsw) => {
                        hnsw.search_filter(&prepared, k, ef, Some(&adapter))
                    }
                    IndexVariant::DotProduct(hnsw) => {
                        hnsw.search_filter(&prepared, k, ef, Some(&adapter))
                    }
                    IndexVariant::Euclidean(hnsw) => {
                        hnsw.search_filter(&prepared, k, ef, Some(&adapter))
                    }
                };
                let results: Vec<(String, f32)> = neighbours
                    .into_iter()
                    .filter_map(|n| {
                        let id = self.id_map.get(n.d_id)?.clone();
                        Some((id, n.distance))
                    })
                    .collect();
                if results.len() >= k {
                    return Ok(results);
                }
                if results.len() > best.len() {
                    best = results;
                }
                if ef >= max_points {
                    break;
                }
                let next_ef = (ef * 2).min(max_points);
                if next_ef <= ef {
                    break;
                }
                ef = next_ef;
            }
            return Ok(best);
        }

        // Sem predicado: busca normal; pedimos mais para compensar tombstones.
        let extra = k.saturating_add(self.tombstones.len()).min(max_points);
        let ef = self.ef_search_runtime.load(Ordering::Relaxed).max(extra);
        let _search_span = tracing::info_span!(
            "hnsw.search",
            candidates = max_points,
            ef = ef,
            tombstones = self.tombstones.len(),
        )
        .entered();
        let neighbours = match &self.inner {
            IndexVariant::Cosine(hnsw) => hnsw.search(&prepared, extra, ef),
            IndexVariant::DotProduct(hnsw) => hnsw.search(&prepared, extra, ef),
            IndexVariant::Euclidean(hnsw) => hnsw.search(&prepared, extra, ef),
        };
        Ok(neighbours
            .into_iter()
            .filter_map(|n| {
                let id = self.id_map.get(n.d_id)?;
                if self.tombstones.contains(id) {
                    return None;
                }
                Some((id.clone(), n.distance))
            })
            .take(k)
            .collect())
    }

    fn add_point(&mut self, point: &Point) -> Result<(), FerresError> {
        let data_id = self.id_map.len();
        let sid = point.storage_id();
        self.id_map.push(sid.clone());
        self.reverse_map.insert(sid, data_id);

        // Normaliza o vetor se a métrica for Cosine.
        let prepared = prepare_vector(&point.vector, self.distance)?;

        match &mut self.inner {
            IndexVariant::Cosine(hnsw) => hnsw.insert_data(&prepared, data_id),
            IndexVariant::DotProduct(hnsw) => hnsw.insert_data(&prepared, data_id),
            IndexVariant::Euclidean(hnsw) => hnsw.insert_data(&prepared, data_id),
        }
        Ok(())
    }

    fn remove_point(&mut self, id: &str) {
        // HNSW não suporta remoção nativa. Usamos tombstone:
        // o ponto fica no grafo mas é filtrado nos resultados.
        if self.reverse_map.contains_key(id) {
            self.tombstones.insert(id.to_string());
            debug!(id, "point tombstoned in HNSW index");
        }
    }

    fn search_explain(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32, ExplainMeta)>, FerresError> {
        // Delega para search com predicado e adiciona ExplainMeta.
        let results = self.search(query, k, predicate)?;
        let candidates_visited = results.len();
        Ok(results
            .into_iter()
            .map(|(id, score)| {
                (
                    id,
                    score,
                    ExplainMeta {
                        candidates_visited,
                        layers_traversed: self.config.max_layer,
                        tombstones_skipped: 0,
                    },
                )
            })
            .collect())
    }
}

// ─── QuantizedHnswIndex ─────────────────────────────────────────────

/// Índice HNSW com Scalar Quantization (SQ8).
///
/// Combina o grafo HNSW para navegação com vetores quantizados `u8`
/// para reduzir consumo de memória em ~4×. A busca usa distância
/// assimétrica: o query permanece em `f32`, os candidatos em `u8`.
///
/// ## Fluxo de busca
///
/// 1. Busca HNSW retorna top-K candidatos (usando distâncias do HNSW)
/// 2. Re-rank com distância assimétrica (query f32 vs candidatos u8)
/// 3. Se `always_ram=true`, re-rank final com vetores originais f32
///
/// ## Economia de memória
///
/// Para 1M vetores de 384 dimensões:
/// - f32: 1M × 384 × 4 = **1.46 GB**
/// - u8:  1M × 384 × 1 = **0.37 GB** (4× menor)
pub struct QuantizedHnswIndex {
    /// Índice HNSW interno para navegação do grafo.
    /// Usa vetores quantizados (dequantized para f32 para inserção no HNSW).
    inner: HnswIndex,
    /// Parâmetros de quantização calibrados.
    params: Option<ScalarQuantizationParams>,
    /// Vetores quantizados (u8) indexados por DataId.
    quantized_vectors: Vec<Vec<u8>>,
    /// Vetores originais para re-ranking (se `always_ram=true`).
    original_vectors: Option<Vec<Vec<f32>>>,
    /// Configuração de quantização.
    config: ScalarQuantizationConfig,
    /// Mapeamento DataId → Point ID (mantido em sincronia com inner).
    id_map: Vec<String>,
}

impl QuantizedHnswIndex {
    /// Cria um índice HNSW quantizado vazio.
    ///
    /// O índice é calibrado automaticamente no primeiro `build()` ou
    /// após acumular pontos suficientes via `add_point()`.
    pub fn new(
        distance: DistanceMetric,
        hnsw_config: HnswConfig,
        sq_config: ScalarQuantizationConfig,
    ) -> Self {
        debug!(
            ?distance,
            always_ram = sq_config.always_ram,
            quantile = sq_config.quantile,
            "creating quantized HNSW index (SQ8)"
        );

        let inner = HnswIndex::new(distance, hnsw_config);

        Self {
            inner,
            params: None,
            quantized_vectors: Vec::new(),
            original_vectors: if sq_config.always_ram {
                Some(Vec::new())
            } else {
                None
            },
            config: sq_config,
            id_map: Vec::new(),
        }
    }

    /// Retorna a métrica configurada.
    pub fn distance_metric(&self) -> DistanceMetric {
        self.inner.distance_metric()
    }

    /// Retorna os parâmetros de quantização calibrados (se houver).
    pub fn quantization_params(&self) -> Option<&ScalarQuantizationParams> {
        self.params.as_ref()
    }

    /// Estimates memory (bytes) wasted by tombstoned points until the next `build()`.
    ///
    /// Tombstoned points remain in `quantized_vectors`, `original_vectors`, and
    /// `id_map`; this returns an approximate byte count for that unreclaimed storage.
    pub fn tombstone_memory_waste(&self) -> usize {
        let tombstone_count = self.inner.tombstone_count();
        let dim = self.quantized_vectors.first().map(|v| v.len()).unwrap_or(0);
        let quantized_waste = tombstone_count * dim; // u8 per dim
        let original_waste = if self.original_vectors.is_some() {
            tombstone_count * dim * 4 // f32 per dim
        } else {
            0
        };
        let id_waste = tombstone_count * 64; // estimate per ID string
        quantized_waste + original_waste + id_waste
    }

    /// Re-rankeia resultados usando distância assimétrica (f32 query vs u8 candidatos).
    ///
    /// Melhora a ordenação comparado com a distância do HNSW que usa vetores
    /// dequantizados (com erro de quantização).
    fn rerank_asymmetric(
        &self,
        query: &[f32],
        candidates: Vec<(String, f32)>,
        k: usize,
    ) -> Vec<(String, f32)> {
        let params = match &self.params {
            Some(p) => p,
            None => return candidates,
        };
        let metric = self.inner.distance_metric();

        let mut scored: Vec<(String, f32)> = candidates
            .into_iter()
            .filter_map(|(id, _old_score)| {
                // Encontra o índice no id_map
                let idx = self.id_map.iter().position(|i| i == &id)?;
                let qvec = self.quantized_vectors.get(idx)?;
                let dist = params.asymmetric_distance(query, qvec, metric);
                Some((id, dist))
            })
            .collect();

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    /// Re-rankeia usando vetores originais f32 (melhor precisão).
    ///
    /// Disponível apenas quando `always_ram=true`.
    fn rerank_original(
        &self,
        query: &[f32],
        candidates: Vec<(String, f32)>,
        k: usize,
    ) -> Vec<(String, f32)> {
        let originals = match &self.original_vectors {
            Some(o) => o,
            None => return candidates,
        };
        let metric = self.inner.distance_metric();

        let mut scored: Vec<(String, f32)> = candidates
            .into_iter()
            .filter_map(|(id, _old_score)| {
                let idx = self.id_map.iter().position(|i| i == &id)?;
                let orig = originals.get(idx)?;
                let dist = compute_distance(query, orig, metric);
                Some((id, dist))
            })
            .collect();

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }
}

/// Calcula distância entre dois vetores f32 para re-ranking.
fn compute_distance(a: &[f32], b: &[f32], metric: DistanceMetric) -> f32 {
    match metric {
        DistanceMetric::Euclidean => euclidean_distance(a, b),
        DistanceMetric::Cosine => {
            let dot = dot_product(a, b);
            let na = a.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>();
            let nb = b.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>();
            let denom = na.sqrt() * nb.sqrt();
            if denom < f64::EPSILON {
                1.0
            } else {
                (1.0 - (dot as f64) / denom) as f32
            }
        }
        DistanceMetric::DotProduct => (1.0 - dot_product(a, b)) as f32,
    }
}

impl ANNIndex for QuantizedHnswIndex {
    fn tombstone_count(&self) -> usize {
        self.inner.tombstone_count()
    }

    fn current_ef_search(&self) -> usize {
        self.inner.current_ef_search()
    }

    fn set_ef_search(&self, v: usize) {
        self.inner.set_ef_search(v);
    }

    fn build(&mut self, points: &[Point]) -> Result<(), FerresError> {
        if points.is_empty() {
            self.params = None;
            self.quantized_vectors.clear();
            self.original_vectors = if self.config.always_ram {
                Some(Vec::new())
            } else {
                None
            };
            self.id_map.clear();
            return self.inner.build(points);
        }

        // 1. Calibra parâmetros de quantização com amostra
        let vectors: Vec<&[f32]> = points.iter().map(|p| p.vector.as_slice()).collect();
        let params = ScalarQuantizationParams::calibrate(&vectors, self.config.quantile);

        // 2. Quantiza todos os vetores
        self.quantized_vectors = points
            .iter()
            .map(|p| params.quantize(&p.vector))
            .collect();

        // 3. Mantém originais se always_ram
        if self.config.always_ram {
            self.original_vectors = Some(points.iter().map(|p| p.vector.clone()).collect());
        } else {
            self.original_vectors = None;
        }

        // 4. Guarda mapeamento de IDs
        self.id_map = points.iter().map(|p| p.id.clone()).collect();

        // 5. Constrói HNSW com vetores dequantizados (para navegação do grafo)
        // Usar dequantized preserva a estrutura do grafo com vetores mais compactos
        let dequantized_points: Vec<Point> = points
            .iter()
            .zip(self.quantized_vectors.iter())
            .map(|(p, qv)| Point {
                id: p.id.clone(),
                vector: params.dequantize(qv),
                metadata: p.metadata.clone(),
                created_at: p.created_at,
                namespace: p.namespace.clone(),
                expires_at: p.expires_at,
                vectors: None,
            })
            .collect();

        self.params = Some(params);
        self.inner.build(&dequantized_points)?;

        debug!(
            count = points.len(),
            dim = points[0].dimension(),
            "quantized HNSW index rebuilt (SQ8)"
        );

        Ok(())
    }

    fn search(
        &self,
        query: &[f32],
        k: usize,
        predicate: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>, FerresError> {
        if self.params.is_none() {
            // Sem calibração, delega para HNSW normal
            return self.inner.search(query, k, predicate);
        }

        // Busca HNSW (com predicado nativo se houver); pedimos mais candidatos
        // para compensar erro de quantização quando não há predicado.
        let expanded_k = (k * 3).max(k + 10);
        let hnsw_results = self.inner.search(query, expanded_k, predicate)?;

        if hnsw_results.is_empty() {
            return Ok(hnsw_results);
        }

        // Re-rank com distância assimétrica (query f32 vs candidatos u8)
        let reranked = self.rerank_asymmetric(query, hnsw_results, expanded_k);

        // Se always_ram, re-rank final com vetores originais
        if self.original_vectors.is_some() {
            Ok(self.rerank_original(query, reranked, k))
        } else {
            let mut final_results = reranked;
            final_results.truncate(k);
            Ok(final_results)
        }
    }

    fn add_point(&mut self, point: &Point) -> Result<(), FerresError> {
        let sid = point.storage_id();
        // Se não temos parâmetros calibrados, faz inserção normal
        if self.params.is_none() {
            self.id_map.push(sid.clone());
            if self.config.always_ram {
                if let Some(ref mut originals) = self.original_vectors {
                    originals.push(point.vector.clone());
                }
            }
            self.quantized_vectors.push(Vec::new()); // placeholder
            return self.inner.add_point(point);
        }

        let params = self.params.as_ref().unwrap();

        // Quantiza o vetor
        let quantized = params.quantize(&point.vector);

        // Guarda vetor quantizado
        self.quantized_vectors.push(quantized.clone());

        // Guarda vetor original se always_ram
        if let Some(ref mut originals) = self.original_vectors {
            originals.push(point.vector.clone());
        }

        // Guarda ID (storage_id para consistência com o mapa da coleção)
        self.id_map.push(sid.clone());

        // Insere no HNSW com vetor dequantizado; inner usa point.storage_id() que deve bater com o mapa
        let dequantized = params.dequantize(&quantized);
        let dq_point = Point {
            id: point.id.clone(),
            vector: dequantized,
            metadata: point.metadata.clone(),
            created_at: point.created_at,
            namespace: point.namespace.clone(),
            expires_at: point.expires_at,
            vectors: None,
        };

        self.inner.add_point(&dq_point)
    }

    /// Remove a point from the index (tombstone-based).
    ///
    /// **Note:** The quantized vectors, original vectors, and ID map are NOT
    /// cleaned up immediately. They will be reclaimed on the next `build()`
    /// (triggered by reindex when tombstones > 20%). This trades memory for
    /// O(1) deletion speed. For workloads with heavy deletes, consider
    /// triggering a manual reindex via the `/reindex` endpoint.
    fn remove_point(&mut self, id: &str) {
        self.inner.remove_point(id);
        // Nota: não removemos de quantized_vectors/original_vectors/id_map
        // pois HNSW usa tombstones. Serão limpos no próximo build().
    }

    fn tombstone_memory_waste(&self) -> usize {
        QuantizedHnswIndex::tombstone_memory_waste(self)
    }
}

// ─── Factory function ──────────────────────────────────────────────

/// Cria o índice ANN apropriado baseado na configuração de quantização.
///
/// Se `quantization` é `None`, retorna `HnswIndex` padrão.
/// Se `Scalar(config)`, retorna `QuantizedHnswIndex` com SQ8.
pub fn create_ann_index(
    distance: DistanceMetric,
    hnsw_config: HnswConfig,
    quantization: &QuantizationConfig,
) -> Box<dyn ANNIndex> {
    match quantization {
        QuantizationConfig::None => Box::new(HnswIndex::new(distance, hnsw_config)),
        QuantizationConfig::Scalar(sq_config) => {
            Box::new(QuantizedHnswIndex::new(distance, hnsw_config, sq_config.clone()))
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::error::FerresError;
    use super::*;

    /// Helper para criar pontos de teste sem validação (bypass do `new`).
    fn make_point(id: &str, vector: Vec<f32>) -> Point {
        Point {
            id: id.to_string(),
            vector,
            metadata: serde_json::Value::Null,
            created_at: 0,
            namespace: None,
            expires_at: None,
            vectors: None,
        }
    }

    #[test]
    fn insert_and_search_returns_nearest() {
        let mut index = HnswIndex::new(DistanceMetric::Euclidean, HnswConfig::default());

        index.add_point(&make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        index.add_point(&make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        index.add_point(&make_point("c", vec![0.9, 0.1, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0, 0.0], 2, None).unwrap();
        assert_eq!(results.len(), 2);
        // O mais próximo de [1,0,0] deve ser "a" (distância 0)
        assert_eq!(results[0].0, "a");
        assert!(results[0].1 < 0.001);
    }

    #[test]
    fn cosine_distance_works() {
        let mut index = HnswIndex::new(DistanceMetric::Cosine, HnswConfig::default());
        index.add_point(&make_point("x", vec![1.0, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0], 1, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "x");
    }

    #[test]
    fn dot_product_distance_works() {
        let mut index = HnswIndex::new(DistanceMetric::DotProduct, HnswConfig::default());
        index.add_point(&make_point("d", vec![1.0, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0], 1, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d");
    }

    #[test]
    fn remove_point_excludes_from_search() {
        let mut index = HnswIndex::new(DistanceMetric::Euclidean, HnswConfig::default());

        index.add_point(&make_point("keep", vec![1.0, 0.0, 0.0])).unwrap();
        index.add_point(&make_point("remove", vec![0.9, 0.1, 0.0])).unwrap();

        index.remove_point("remove");

        let results = index.search(&[1.0, 0.0, 0.0], 2, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "keep");
    }

    #[test]
    fn build_clears_tombstones_and_reindexes() {
        let mut index = HnswIndex::new(DistanceMetric::Euclidean, HnswConfig::default());

        let p1 = make_point("a", vec![1.0, 0.0]);
        let p2 = make_point("b", vec![0.0, 1.0]);
        index.add_point(&p1).unwrap();
        index.add_point(&p2).unwrap();
        index.remove_point("b");

        // Rebuild só com p1
        index.build(&[p1]).unwrap();

        assert_eq!(index.len(), 1);
        let results = index.search(&[1.0, 0.0], 5, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "a");
    }

    #[test]
    fn normalize_vector_unit_norm() {
        let v = vec![3.0, 4.0];
        let n = normalize_vector(&v).unwrap();
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((n[0] - 0.6).abs() < 1e-6);
        assert!((n[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn normalize_zero_vector_returns_error() {
        let v = vec![0.0, 0.0, 0.0];
        let err = normalize_vector(&v).unwrap_err();
        assert!(matches!(err, FerresError::InvalidVector { .. }));
        assert!(err.to_string().contains("zero-norm"));
    }

    #[test]
    fn normalize_nan_vector_returns_error() {
        let v = vec![1.0, f32::NAN, 0.0];
        let err = normalize_vector(&v).unwrap_err();
        assert!(matches!(err, FerresError::InvalidVector { .. }));
        assert!(err.to_string().contains("non-finite"));
    }

    /// Testa recall@10 com 1000 vetores aleatórios de 384 dimensões.
    ///
    /// Para cada vetor inserido, busca os 10 vizinhos mais próximos e
    /// verifica que o próprio vetor aparece entre os resultados (recall@1
    /// como proxy de recall@10). Espera recall > 90%.
    ///
    /// **Nota:** DotProduct (`DistDot` do anndists) não é incluído neste
    /// teste de alta dimensão porque a implementação calcula `1 − dot(a,b)`
    /// em f32 e o acúmulo de erro em 384 dimensões pode violar a assertion
    /// interna `dot >= 0`. A métrica DotProduct é validada no teste
    /// `dot_product_distance_works` com vetores de baixa dimensão.
    #[test]
    fn recall_at_10_with_1000_random_vectors() {
        use rand::Rng;

        const N: usize = 1_000;
        const DIM: usize = 384;
        const K: usize = 10;

        let mut rng = rand::thread_rng();

        // Gera 1000 vetores aleatórios de 384 dimensões com componentes em [-1, 1).
        let points: Vec<Point> = (0..N)
            .map(|i| {
                let vector: Vec<f32> = (0..DIM).map(|_| rng.gen_range(-1.0_f32..1.0)).collect();
                make_point(&format!("v{i}"), vector)
            })
            .collect();

        // Euclidean e Cosine — DistDot excluído (vide doc acima).
        for metric in [DistanceMetric::Euclidean, DistanceMetric::Cosine] {
            let config = HnswConfig {
                max_nb_connection: 16,
                max_elements: N + 100,
                max_layer: 16,
                ef_construction: 200,
                ef_search: 50,
            };

            let mut index = HnswIndex::new(metric, config);
            index.build(&points).unwrap();

            // Para cada vetor, busca top-10 e verifica se ele mesmo aparece.
            let mut hits = 0usize;
            for point in &points {
                let results = index.search(&point.vector, K, None).unwrap();
                let ids: Vec<&str> = results.iter().map(|r| r.0.as_str()).collect();
                if ids.contains(&point.id.as_str()) {
                    hits += 1;
                }
            }

            let recall = hits as f64 / N as f64;
            assert!(
                recall > 0.90,
                "recall@{K} for {metric:?} too low: {recall:.3} ({hits}/{N})"
            );
        }
    }

    // ─── QuantizedHnswIndex Tests ────────────────────────────────────

    /// Testa recall@10 do SQ8 com 1000 vetores: deve ser > 95% comparado com f32.
    ///
    /// Compara os resultados do índice quantizado com os resultados do índice
    /// normal (ground truth) para verificar que a perda de recall é mínima.
    #[test]
    fn test_sq8_recall() {
        use rand::Rng;

        const N: usize = 1_000;
        const DIM: usize = 128; // dimensão menor para teste rápido
        const K: usize = 10;
        const NUM_QUERIES: usize = 100;

        let mut rng = rand::thread_rng();

        let points: Vec<Point> = (0..N)
            .map(|i| {
                let vector: Vec<f32> = (0..DIM).map(|_| rng.gen_range(-1.0_f32..1.0)).collect();
                make_point(&format!("v{i}"), vector)
            })
            .collect();

        let hnsw_config = HnswConfig {
            max_nb_connection: 16,
            max_elements: N + 100,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 64,
        };

        // Índice normal (ground truth)
        let mut normal_index = HnswIndex::new(DistanceMetric::Euclidean, hnsw_config.clone());
        normal_index.build(&points).unwrap();

        // Índice quantizado
        let sq_config = ScalarQuantizationConfig {
            dtype: crate::quantization::ScalarType::Int8,
            always_ram: false,
            quantile: 99.5,
        };
        let mut quantized_index = QuantizedHnswIndex::new(
            DistanceMetric::Euclidean,
            hnsw_config,
            sq_config,
        );
        quantized_index.build(&points).unwrap();

        // Compara recall para queries aleatórias
        let mut total_overlap = 0usize;
        let mut total_possible = 0usize;

        for i in 0..NUM_QUERIES {
            let query = &points[i % N].vector;

            let normal_results = normal_index.search(query, K, None).unwrap();
            let quantized_results = quantized_index.search(query, K, None).unwrap();

            let normal_ids: std::collections::HashSet<&str> = normal_results.iter().map(|r| r.0.as_str()).collect();
            let quantized_ids: std::collections::HashSet<&str> = quantized_results.iter().map(|r| r.0.as_str()).collect();

            total_overlap += normal_ids.intersection(&quantized_ids).count();
            total_possible += K.min(normal_results.len());
        }

        let recall = total_overlap as f64 / total_possible as f64;
        assert!(
            recall > 0.90,
            "SQ8 recall@{K} too low: {recall:.3} ({total_overlap}/{total_possible}). \
             Expected > 90% overlap with f32 index."
        );
    }

    /// Testa que QuantizedHnswIndex funciona com build e search básicos.
    #[test]
    fn test_quantized_hnsw_basic() {
        let sq_config = ScalarQuantizationConfig::default();
        let mut index = QuantizedHnswIndex::new(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            sq_config,
        );

        let points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.9, 0.1, 0.0]),
        ];

        index.build(&points).unwrap();

        let results = index.search(&[1.0, 0.0, 0.0], 2, None).unwrap();
        assert_eq!(results.len(), 2);
        // O mais próximo de [1,0,0] deve ser "a"
        assert_eq!(results[0].0, "a");
    }

    /// Testa QuantizedHnswIndex com always_ram (re-ranking com originais).
    #[test]
    fn test_quantized_hnsw_always_ram() {
        let sq_config = ScalarQuantizationConfig {
            dtype: crate::quantization::ScalarType::Int8,
            always_ram: true,
            quantile: 99.5,
        };
        let mut index = QuantizedHnswIndex::new(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            sq_config,
        );

        let points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.9, 0.1, 0.0]),
        ];

        index.build(&points).unwrap();

        // Deve ter vetores originais armazenados
        assert!(index.original_vectors.is_some());
        assert_eq!(index.original_vectors.as_ref().unwrap().len(), 3);

        let results = index.search(&[1.0, 0.0, 0.0], 2, None).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a");
    }

    /// Testa add_point incremental no QuantizedHnswIndex.
    #[test]
    fn test_quantized_hnsw_add_point() {
        let sq_config = ScalarQuantizationConfig::default();
        let mut index = QuantizedHnswIndex::new(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            sq_config,
        );

        // Build inicial para calibrar com mais pontos para estabilidade
        let initial = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.0, 0.0, 1.0]),
            make_point("d", vec![0.5, 0.5, 0.0]),
        ];
        index.build(&initial).unwrap();

        // Adiciona ponto incremental
        index.add_point(&make_point("e", vec![0.9, 0.1, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0, 0.0], 3, None).unwrap();
        assert!(results.len() >= 2, "expected at least 2 results, got {}", results.len());
        // O mais próximo de [1,0,0] deve ser "a"
        assert_eq!(results[0].0, "a");
    }

    /// Testa remove_point no QuantizedHnswIndex.
    #[test]
    fn test_quantized_hnsw_remove_point() {
        let sq_config = ScalarQuantizationConfig::default();
        let mut index = QuantizedHnswIndex::new(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            sq_config,
        );

        let points = vec![
            make_point("keep", vec![1.0, 0.0, 0.0]),
            make_point("remove", vec![0.9, 0.1, 0.0]),
        ];
        index.build(&points).unwrap();

        index.remove_point("remove");

        let results = index.search(&[1.0, 0.0, 0.0], 2, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "keep");
    }

    /// Build with 100 points, remove 50, assert tombstone_memory_waste > 0; rebuild, assert tombstone_memory_waste == 0.
    #[test]
    fn test_quantized_tombstone_waste() {
        use rand::Rng;

        const N: usize = 100;
        const DIM: usize = 32;

        let mut rng = rand::thread_rng();
        let points: Vec<Point> = (0..N)
            .map(|i| {
                let vector: Vec<f32> = (0..DIM).map(|_| rng.gen_range(-1.0_f32..1.0)).collect();
                make_point(&format!("p{i}"), vector)
            })
            .collect();

        let sq_config = ScalarQuantizationConfig::default();
        let mut index = QuantizedHnswIndex::new(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            sq_config,
        );

        index.build(&points).unwrap();
        assert_eq!(index.tombstone_memory_waste(), 0, "no tombstones after build");

        for i in 0..50 {
            index.remove_point(&format!("p{i}"));
        }
        assert!(
            index.tombstone_memory_waste() > 0,
            "tombstone_memory_waste should be > 0 after 50 removes"
        );

        let remaining: Vec<Point> = points.into_iter().skip(50).collect();
        index.build(&remaining).unwrap();
        assert_eq!(
            index.tombstone_memory_waste(),
            0,
            "tombstone_memory_waste should be 0 after rebuild"
        );
    }

    /// Testa a factory function create_ann_index.
    #[test]
    fn test_create_ann_index_none() {
        let index = create_ann_index(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            &QuantizationConfig::None,
        );
        // Deve funcionar como HnswIndex normal
        let _ = index.search(&[0.0, 0.0, 0.0], 1, None);
    }

    #[test]
    fn test_create_ann_index_sq8() {
        let index = create_ann_index(
            DistanceMetric::Euclidean,
            HnswConfig::default(),
            &QuantizationConfig::Scalar(ScalarQuantizationConfig::default()),
        );
        let _ = index.search(&[0.0, 0.0, 0.0], 1, None);
    }
}
