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

use hnsw_rs::prelude::*;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::FerresError;
use crate::point::Point;

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
///     fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>, ferres_db_core::FerresError> {
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
    /// crescente.
    fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>, FerresError>;

    /// Adiciona um único ponto ao índice.
    fn add_point(&mut self, point: &Point) -> Result<(), FerresError>;

    /// Remove um ponto do índice pelo ID.
    ///
    /// Em backends que não suportam remoção nativa (como HNSW),
    /// isso marca o ponto como tombstone — ele é ignorado nas buscas
    /// mas continua no grafo até o próximo `build`.
    fn remove_point(&mut self, id: &str);
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
            reason: format!("non-finite value at index {}", pos),
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
fn validate_vector_finite(v: &[f32]) -> Result<(), FerresError> {
    if let Some(pos) = v.iter().position(|x| !x.is_finite()) {
        return Err(FerresError::InvalidVector {
            reason: format!("non-finite value at index {}", pos),
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

        Self {
            inner,
            id_map: Vec::new(),
            reverse_map: HashMap::new(),
            tombstones: HashSet::new(),
            distance,
            config,
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
                self.id_map.push(point.id.clone());
                self.reverse_map.insert(point.id.clone(), data_id);

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

    fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>, FerresError> {
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

        // Pedimos mais resultados para compensar tombstones filtrados.
        // Usa saturating_add para evitar overflow em casos extremos.
        let extra = k.saturating_add(self.tombstones.len()).min(max_points);
        let ef = self.config.ef_search.max(extra);

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
        self.id_map.push(point.id.clone());
        self.reverse_map.insert(point.id.clone(), data_id);

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
        }
    }

    #[test]
    fn insert_and_search_returns_nearest() {
        let mut index = HnswIndex::new(DistanceMetric::Euclidean, HnswConfig::default());

        index.add_point(&make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        index.add_point(&make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        index.add_point(&make_point("c", vec![0.9, 0.1, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        // O mais próximo de [1,0,0] deve ser "a" (distância 0)
        assert_eq!(results[0].0, "a");
        assert!(results[0].1 < 0.001);
    }

    #[test]
    fn cosine_distance_works() {
        let mut index = HnswIndex::new(DistanceMetric::Cosine, HnswConfig::default());
        index.add_point(&make_point("x", vec![1.0, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "x");
    }

    #[test]
    fn dot_product_distance_works() {
        let mut index = HnswIndex::new(DistanceMetric::DotProduct, HnswConfig::default());
        index.add_point(&make_point("d", vec![1.0, 0.0])).unwrap();

        let results = index.search(&[1.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d");
    }

    #[test]
    fn remove_point_excludes_from_search() {
        let mut index = HnswIndex::new(DistanceMetric::Euclidean, HnswConfig::default());

        index.add_point(&make_point("keep", vec![1.0, 0.0, 0.0])).unwrap();
        index.add_point(&make_point("remove", vec![0.9, 0.1, 0.0])).unwrap();

        index.remove_point("remove");

        let results = index.search(&[1.0, 0.0, 0.0], 2).unwrap();
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
        let results = index.search(&[1.0, 0.0], 5).unwrap();
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
                let results = index.search(&point.vector, K).unwrap();
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
}
