//! # Explain — explicação detalhada de resultados de busca vetorial
//!
//! Este módulo fornece tipos e funções para explicar **por que** cada
//! resultado foi retornado (ou filtrado) em uma busca vetorial. É útil
//! para debug, tuning de filtros e compreensão do comportamento do índice.
//!
//! ## Estrutura
//!
//! - [`SearchExplanation`] — resposta principal, contém todos os resultados explicados.
//! - [`ExplainResult`] — explicação de um único resultado (score breakdown, filtro, ranking).
//! - [`FilterExplanation`] — avaliação condição-a-condição do filtro de metadata.
//! - [`ExplainMeta`] — metadados internos do índice coletados durante a busca.
//! - [`IndexStats`] — estatísticas do índice no momento da busca.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::collection::Collection;
use crate::error::FerresError;
use crate::point::Point;
use crate::search::DistanceMetric;
use crate::MetadataCondition;
use crate::MetadataFilter;

// ─── ExplainMeta ─────────────────────────────────────────────────────

/// Metadados internos do índice coletados durante `search_explain`.
///
/// Permite entender quantos candidatos foram visitados, quantas
/// camadas do grafo HNSW foram percorridas e quantos tombstones
/// foram ignorados.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainMeta {
    /// Número de candidatos visitados pelo algoritmo de busca.
    pub candidates_visited: usize,
    /// Número de camadas do grafo HNSW percorridas.
    pub layers_traversed: usize,
    /// Número de tombstones (pontos removidos) ignorados durante a busca.
    pub tombstones_skipped: usize,
}

// ─── ConditionResult ─────────────────────────────────────────────────

// Explain is inherently more expensive than search. These allocations
// (field, expected, actual per condition per candidate) are O(candidates × conditions),
// but explain is a debug/diagnostic endpoint, not meant for hot-path production queries.
// We accept this cost to keep the API simple (no lifetime propagation).

/// Resultado da avaliação de uma única condição de filtro contra um ponto.
///
/// Para cada condição (e.g. `category == "tech"`), mostra o valor
/// esperado, o valor real encontrado no metadata e se a condição passou.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionResult {
    /// Nome do campo de metadata (e.g. "category").
    pub field: String,
    /// Operador aplicado (e.g. "$eq", "$gt", "$in").
    pub operator: String,
    /// Valor esperado pela condição.
    pub expected: serde_json::Value,
    /// Valor real encontrado no metadata do ponto.
    pub actual: serde_json::Value,
    /// Se a condição foi satisfeita.
    pub passed: bool,
}

// ─── FilterExplanation ───────────────────────────────────────────────

/// Avaliação completa de um filtro de metadata contra um ponto.
///
/// Contém o resultado de cada condição individual e se o filtro
/// como um todo foi satisfeito (AND de todas as condições).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterExplanation {
    /// Resultado de cada condição individual.
    pub conditions: Vec<ConditionResult>,
    /// Se o filtro como um todo foi satisfeito.
    pub passed: bool,
}

// ─── ExplainResult ───────────────────────────────────────────────────

/// Explicação detalhada de um resultado de busca vetorial.
///
/// Contém o score, breakdown dos componentes, avaliação de filtros
/// e posição no ranking antes e depois da filtragem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResult {
    /// ID do ponto.
    pub id: String,
    /// Score final na métrica original da coleção.
    ///
    /// - **Cosine**: `1.0 - cosine_similarity` (derivado de `raw_distance`).
    /// - **Euclidean**: `sqrt(raw_distance)` (HNSW retorna L2²).
    /// - **DotProduct**: `raw_distance` (já é `1 - dot`).
    pub score: f32,
    /// Métrica de distância utilizada (e.g. "Cosine", "Euclidean").
    pub distance_metric: String,
    /// Distância bruta retornada pelo índice HNSW, sem transformação.
    pub raw_distance: f32,
    /// Similaridade no intervalo \[0, 1\] (quanto maior, mais similar).
    ///
    /// - **Cosine**: `Some(1.0 - score)` = cosine similarity.
    /// - **DotProduct**: `Some(1.0 - score)`.
    /// - **Euclidean**: `None` (similaridade não faz sentido para distância euclidiana).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,
    /// Componentes do score (para busca híbrida: vector_score, bm25_score, combined).
    pub score_breakdown: HashMap<String, f32>,
    /// Avaliação do filtro de metadata (presente apenas se filtro foi aplicado).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_evaluation: Option<FilterExplanation>,
    /// Posição no ranking antes da aplicação de filtros (1-indexed).
    pub rank_before_filter: usize,
    /// Posição no ranking após aplicação de filtros (1-indexed, 0 se não passou).
    pub rank_after_filter: usize,
}

// ─── IndexStats ──────────────────────────────────────────────────────

/// Estatísticas do índice HNSW no momento da busca.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    /// Total de pontos no índice (excluindo tombstones).
    pub total_points: usize,
    /// Número máximo de camadas do grafo HNSW.
    pub hnsw_layers: usize,
    /// Valor de ef_search utilizado na busca.
    pub ef_search_used: usize,
    /// Número de tombstones ignorados durante a busca.
    pub tombstones_skipped: usize,
}

// ─── SearchExplanation ───────────────────────────────────────────────

/// Explicação completa de uma busca vetorial.
///
/// Contém informações sobre o vetor de consulta, candidatos escaneados,
/// resultados explicados individualmente, estatísticas do índice e
/// metadados do percurso da busca (camadas HNSW, comparações de distância).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchExplanation {
    /// Norma L2 do vetor de consulta.
    pub query_vector_norm: f32,
    /// Métrica de distância utilizada.
    pub distance_metric: String,
    /// Total de candidatos escaneados pelo índice.
    pub candidates_scanned: usize,
    /// Candidatos que passaram no filtro de metadata.
    pub candidates_after_filter: usize,
    /// Resultados explicados individualmente.
    pub results: Vec<ExplainResult>,
    /// Estatísticas do índice no momento da busca.
    pub index_stats: IndexStats,
    /// Metadados do percurso da busca (camadas percorridas, comparações).
    /// Presente quando o índice retorna esses dados (ex.: HNSW).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain_meta: Option<ExplainMeta>,
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Avalia uma condição de metadata contra os metadados de um ponto,
/// retornando um [`ConditionResult`] detalhado para uso em explain.
pub fn evaluate_condition(
    cond: &MetadataCondition,
    metadata: &serde_json::Value,
) -> ConditionResult {
    match cond {
        MetadataCondition::Eq(field, expected) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$eq".to_string(),
                expected: expected.clone(),
                actual,
                passed: cond.matches(metadata),
            }
        }
        MetadataCondition::Ne(field, expected) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$ne".to_string(),
                expected: expected.clone(),
                actual,
                passed: cond.matches(metadata),
            }
        }
        MetadataCondition::In(field, values) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$in".to_string(),
                expected: serde_json::Value::Array(values.clone()),
                actual,
                passed: cond.matches(metadata),
            }
        }
        MetadataCondition::Gt(field, threshold) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$gt".to_string(),
                expected: serde_json::json!(*threshold),
                actual,
                passed: cond.matches(metadata),
            }
        }
        MetadataCondition::Lt(field, threshold) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$lt".to_string(),
                expected: serde_json::json!(*threshold),
                actual,
                passed: cond.matches(metadata),
            }
        }
        MetadataCondition::Gte(field, threshold) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$gte".to_string(),
                expected: serde_json::json!(*threshold),
                actual,
                passed: cond.matches(metadata),
            }
        }
        MetadataCondition::Lte(field, threshold) => {
            let actual = metadata
                .get(field)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ConditionResult {
                field: field.clone(),
                operator: "$lte".to_string(),
                expected: serde_json::json!(*threshold),
                actual,
                passed: cond.matches(metadata),
            }
        }
    }
}

// ─── build_search_explanation ────────────────────────────────────

/// Constrói uma [`SearchExplanation`] completa a partir de uma coleção.
///
/// Esta é a **única** implementação da lógica de explain. Tanto
/// `VectorDB::search_explain` quanto os handlers REST e gRPC devem
/// delegar a esta função para evitar duplicação.
///
/// # Parâmetros
///
/// - `collection` — referência à coleção onde a busca será realizada.
/// - `query` — vetor de consulta.
/// - `limit` — número máximo de resultados desejados.
/// - `filter` — filtro de metadata opcional (AND de condições).
///
/// # Erros
///
/// - `DimensionMismatch` se o vetor de consulta tiver dimensão incorreta.
/// - Erros propagados do índice HNSW durante a busca.
pub fn build_search_explanation(
    collection: &Collection,
    query: &[f32],
    limit: usize,
    filter: Option<MetadataFilter>,
    vector_field: Option<&str>,
) -> Result<SearchExplanation, FerresError> {
    let resolver = |id: &str| collection.get(id).cloned();
    build_search_explanation_with_resolver(collection, query, limit, filter, vector_field, &resolver)
}

/// Variante de [`build_search_explanation`] que aceita um resolver customizado
/// para resolução de pontos.
///
/// Quando tiered storage está habilitado, pontos podem estar em Warm (mmap)
/// ou Cold (disco) e não são encontrados por `collection.get(id)`. O
/// `point_resolver` permite buscar pontos em qualquer tier.
///
/// # Parâmetros
///
/// - `collection` — referência à coleção (para busca HNSW, validação e config).
/// - `query` — vetor de consulta.
/// - `limit` — número máximo de resultados desejados.
/// - `filter` — filtro de metadata opcional (AND de condições).
/// - `point_resolver` — closure que resolve um ponto pelo ID, buscando em
///   qualquer tier de armazenamento.
pub fn build_search_explanation_with_resolver(
    collection: &Collection,
    query: &[f32],
    limit: usize,
    filter: Option<MetadataFilter>,
    vector_field: Option<&str>,
    point_resolver: &dyn Fn(&str) -> Option<Point>,
) -> Result<SearchExplanation, FerresError> {
    collection.validate_dimension(query)?;

    // Calcula norma L2 do vetor de consulta
    let query_vector_norm = query
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt() as f32;

    let distance_metric = format!("{:?}", collection.config().distance);
    let filter = filter.unwrap_or_else(MetadataFilter::empty);

    // Busca mais candidatos quando há filtro para compensar filtragem
    let search_limit = if filter.is_empty() {
        limit
    } else {
        let expanded = limit.saturating_mul(10);
        let max_points = collection.len().max(limit);
        expanded.min(max_points)
    };

    info!(
        collection = %collection.name(),
        limit,
        search_limit,
        has_filter = !filter.is_empty(),
        "performing search_explain"
    );

    // Busca com metadata de explain (sem predicado: buscamos mais candidatos
    // para avaliar filtro condição-a-condição no explain).
    let raw_results = collection.search_explain(query, search_limit, None, vector_field)?;
    let candidates_scanned = raw_results.len();

    // Constrói ExplainResult para cada candidato
    let mut explain_results = Vec::with_capacity(raw_results.len());
    let mut rank_after_counter = 0usize;

    // Captura ExplainMeta do primeiro resultado (igual para todos quando vindo do HNSW).
    let explain_meta = raw_results
        .first()
        .map(|(_, _, meta)| meta.clone());
    let total_tombstones_skipped = explain_meta
        .as_ref()
        .map(|m| m.tombstones_skipped)
        .unwrap_or(0);

    let metric = collection.config().distance;

    for (rank_before_idx, (id, hnsw_score, _meta)) in raw_results.iter().enumerate() {
        let point = match point_resolver(id) {
            Some(p) => p,
            None => continue,
        };

        // Avalia cada condição do filtro individualmente; namespace é parte do filtro
        let filter_eval = if !filter.is_empty() {
            let condition_results: Vec<ConditionResult> = filter
                .conditions()
                .iter()
                .map(|cond| evaluate_condition(cond, &point.metadata))
                .collect();
            let passed = filter.matches_point(&point);
            Some(FilterExplanation {
                conditions: condition_results,
                passed,
            })
        } else {
            None
        };

        let passed_filter = filter_eval.as_ref().is_none_or(|f| f.passed);
        if passed_filter {
            rank_after_counter += 1;
        }

        // raw_distance = valor bruto do HNSW, sem transformação
        let raw_distance = *hnsw_score;

        // score = distância na métrica original
        let score = match metric {
            // Cosine: HNSW com normalização L2 retorna L2².
            // cosine_distance = L2² / 2. Para mostrar como "cosine distance":
            // score = 1.0 - (1.0 - raw_distance / 2.0).clamp(0.0, 1.0)
            DistanceMetric::Cosine => 1.0 - (1.0 - raw_distance / 2.0).clamp(0.0, 1.0),
            // Euclidean: HNSW retorna L2² (distância ao quadrado)
            DistanceMetric::Euclidean => raw_distance.sqrt(),
            // DotProduct: já é 1 - dot
            DistanceMetric::DotProduct => raw_distance,
        };

        // similarity: faz sentido apenas para Cosine e DotProduct
        let similarity = match metric {
            DistanceMetric::Cosine => Some(1.0 - score),
            DistanceMetric::DotProduct => Some(1.0 - score),
            DistanceMetric::Euclidean => None,
        };

        let mut score_breakdown = HashMap::new();
        score_breakdown.insert("raw_distance".to_string(), raw_distance);
        score_breakdown.insert("vector_score".to_string(), score);
        if let Some(sim) = similarity {
            score_breakdown.insert("cosine_similarity".to_string(), sim);
        }

        explain_results.push(ExplainResult {
            id: point.id.clone(),
            score,
            distance_metric: distance_metric.clone(),
            raw_distance,
            similarity,
            score_breakdown,
            filter_evaluation: filter_eval,
            rank_before_filter: rank_before_idx + 1,
            rank_after_filter: if passed_filter { rank_after_counter } else { 0 },
        });
    }

    let candidates_after_filter = explain_results
        .iter()
        .filter(|r| r.filter_evaluation.as_ref().is_none_or(|f| f.passed))
        .count();

    let index_stats = IndexStats {
        total_points: collection.len(),
        hnsw_layers: collection.config().hnsw.max_layer,
        ef_search_used: collection.current_hnsw_ef_search(),
        tombstones_skipped: total_tombstones_skipped,
    };

    info!(
        collection = %collection.name(),
        candidates_scanned,
        candidates_after_filter,
        results = explain_results.len(),
        "search_explain completed"
    );

    Ok(SearchExplanation {
        query_vector_norm,
        distance_metric,
        candidates_scanned,
        candidates_after_filter,
        results: explain_results,
        index_stats,
        explain_meta,
    })
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CollectionConfig, DistanceMetric, HnswConfig, MetadataFilter, Point, VectorDB,
    };
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
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: Default::default(),
            tiered_storage: Default::default(),
            retention_days: None,
        };
        db.create_collection(config).unwrap();
    }

    #[test]
    fn test_explain_basic() {
        let (mut db, _temp) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        let points = vec![
            Point::new("p1", vec![1.0, 0.0, 0.0], json!({"category": "tech"})).unwrap(),
            Point::new("p2", vec![0.0, 1.0, 0.0], json!({"category": "science"})).unwrap(),
            Point::new("p3", vec![0.9, 0.1, 0.0], json!({"category": "tech"})).unwrap(),
        ];
        db.upsert_points("test", points).unwrap();

        let explanation = db
            .search_explain("test", vec![1.0, 0.0, 0.0], 3, None)
            .unwrap();

        // Verifica campos globais
        assert_eq!(explanation.distance_metric, "Euclidean");
        assert!(explanation.query_vector_norm > 0.0);
        assert!(explanation.candidates_scanned > 0);
        assert!(!explanation.results.is_empty());

        // Verifica que cada resultado tem score_breakdown com vector_score e raw_distance
        for result in &explanation.results {
            assert!(
                result.score_breakdown.contains_key("vector_score"),
                "score_breakdown deve conter 'vector_score'"
            );
            assert!(
                result.score_breakdown.contains_key("raw_distance"),
                "score_breakdown deve conter 'raw_distance'"
            );
            assert_eq!(result.distance_metric, "Euclidean");
            assert!(result.rank_before_filter > 0);

            // Para Euclidean: score = sqrt(raw_distance), logo score² ≈ raw_distance
            let expected_score = result.raw_distance.sqrt();
            assert!(
                (result.score - expected_score).abs() < 1e-6,
                "Euclidean: score ({}) deve ser sqrt(raw_distance ({}))",
                result.score,
                result.raw_distance,
            );

            // Para Euclidean: similarity deve ser None
            assert!(
                result.similarity.is_none(),
                "Euclidean: similarity deve ser None"
            );
        }

        // Verifica IndexStats
        assert_eq!(explanation.index_stats.total_points, 3);
        assert!(explanation.index_stats.ef_search_used > 0);
    }

    #[test]
    fn test_explain_cosine() {
        let (mut db, _temp) = create_test_db();

        // Cria coleção com métrica Cosine
        let config = CollectionConfig {
            name: "cosine_test".to_string(),
            dimension: 3,
            distance: DistanceMetric::Cosine,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: Default::default(),
            tiered_storage: Default::default(),
            retention_days: None,
        };
        db.create_collection(config).unwrap();

        let points = vec![
            Point::new("p1", vec![1.0, 0.0, 0.0], json!({"category": "tech"})).unwrap(),
            Point::new("p2", vec![0.0, 1.0, 0.0], json!({"category": "science"})).unwrap(),
            Point::new("p3", vec![0.9, 0.1, 0.0], json!({"category": "tech"})).unwrap(),
        ];
        db.upsert_points("cosine_test", points).unwrap();

        let explanation = db
            .search_explain("cosine_test", vec![1.0, 0.0, 0.0], 3, None)
            .unwrap();

        assert_eq!(explanation.distance_metric, "Cosine");
        assert!(!explanation.results.is_empty());

        for result in &explanation.results {
            assert_eq!(result.distance_metric, "Cosine");

            // Para Cosine: score e raw_distance podem diferir
            // raw_distance é o valor bruto L2² do HNSW
            // score = 1.0 - (1.0 - raw_distance / 2.0).clamp(0.0, 1.0)
            let expected_score =
                1.0 - (1.0 - result.raw_distance / 2.0).clamp(0.0, 1.0);
            assert!(
                (result.score - expected_score).abs() < 1e-6,
                "Cosine: score ({}) deve ser derivado de raw_distance ({})",
                result.score,
                result.raw_distance,
            );

            // similarity deve estar presente para Cosine
            assert!(
                result.similarity.is_some(),
                "Cosine: similarity deve ser Some"
            );

            let sim = result.similarity.unwrap();
            assert!(
                (sim - (1.0 - result.score)).abs() < 1e-6,
                "Cosine: similarity ({}) deve ser 1.0 - score ({})",
                sim,
                result.score,
            );
            assert!(sim >= 0.0 && sim <= 1.0, "similarity deve estar em [0, 1]");

            // score_breakdown deve conter cosine_similarity
            assert!(
                result.score_breakdown.contains_key("cosine_similarity"),
                "score_breakdown deve conter 'cosine_similarity'"
            );
            assert!(
                result.score_breakdown.contains_key("raw_distance"),
                "score_breakdown deve conter 'raw_distance'"
            );
            assert!(
                result.score_breakdown.contains_key("vector_score"),
                "score_breakdown deve conter 'vector_score'"
            );
        }

        // O ponto mais próximo de [1,0,0] com Cosine deve ser p1 (coseno = 1)
        let best = &explanation.results[0];
        assert_eq!(best.id, "p1");
        // similarity próxima de 1.0 para vetor idêntico
        assert!(
            best.similarity.unwrap() > 0.9,
            "p1 deve ter similarity alta, got {}",
            best.similarity.unwrap()
        );
    }

    #[test]
    fn test_explain_with_filter() {
        let (mut db, _temp) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        let points = vec![
            Point::new(
                "p1",
                vec![1.0, 0.0, 0.0],
                json!({"category": "tech", "price": 50}),
            )
            .unwrap(),
            Point::new(
                "p2",
                vec![0.0, 1.0, 0.0],
                json!({"category": "science", "price": 200}),
            )
            .unwrap(),
            Point::new(
                "p3",
                vec![0.9, 0.1, 0.0],
                json!({"category": "tech", "price": 30}),
            )
            .unwrap(),
        ];
        db.upsert_points("test", points).unwrap();

        let filter = MetadataFilter::from_json(json!({
            "category": "tech",
            "price": { "$lte": 100 }
        }))
        .unwrap();

        let explanation = db
            .search_explain("test", vec![1.0, 0.0, 0.0], 10, Some(filter))
            .unwrap();

        // Deve ter candidatos escaneados >= candidatos após filtro
        assert!(explanation.candidates_scanned >= explanation.candidates_after_filter);

        // Verifica que resultados têm filter_evaluation
        for result in &explanation.results {
            let eval = result
                .filter_evaluation
                .as_ref()
                .expect("filter_evaluation deve estar presente");

            // Deve ter 2 condições (category + price)
            assert_eq!(eval.conditions.len(), 2);

            // Verifica que cada condição tem os campos preenchidos
            for cond in &eval.conditions {
                assert!(!cond.field.is_empty());
                assert!(!cond.operator.is_empty());
            }
        }

        // Verifica que p2 (science, price=200) NÃO passou no filtro
        let p2_result = explanation.results.iter().find(|r| r.id == "p2");
        if let Some(p2) = p2_result {
            let eval = p2.filter_evaluation.as_ref().unwrap();
            assert!(!eval.passed, "p2 não deveria passar no filtro");
            assert_eq!(p2.rank_after_filter, 0, "rank_after_filter deve ser 0 para pontos que não passaram");
        }

        // Verifica que p1 e p3 (tech, price <= 100) PASSARAM no filtro
        let passed_results: Vec<_> = explanation
            .results
            .iter()
            .filter(|r| {
                r.filter_evaluation
                    .as_ref()
                    .is_none_or(|f| f.passed)
            })
            .collect();
        assert_eq!(passed_results.len(), 2);
    }

    #[test]
    fn test_explain_hybrid() {
        let (mut db, _temp) = create_test_db();

        // Cria coleção com BM25 habilitado
        let config = CollectionConfig {
            name: "hybrid_test".to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: true,
            bm25_text_field: "text".to_string(),
            quantization: Default::default(),
            tiered_storage: Default::default(),
            retention_days: None,
        };
        db.create_collection(config).unwrap();

        let points = vec![
            Point::new(
                "p1",
                vec![1.0, 0.0, 0.0],
                json!({"text": "hello world", "category": "tech"}),
            )
            .unwrap(),
            Point::new(
                "p2",
                vec![0.0, 1.0, 0.0],
                json!({"text": "foo bar", "category": "science"}),
            )
            .unwrap(),
        ];
        db.upsert_points("hybrid_test", points).unwrap();

        // Busca explain (vetorial apenas) em coleção com BM25 habilitado
        let explanation = db
            .search_explain("hybrid_test", vec![1.0, 0.0, 0.0], 5, None)
            .unwrap();

        // Deve funcionar normalmente mesmo com BM25 habilitado
        assert!(!explanation.results.is_empty());

        // score_breakdown deve ter vector_score
        for result in &explanation.results {
            assert!(
                result.score_breakdown.contains_key("vector_score"),
                "score_breakdown deve conter 'vector_score'"
            );
        }

        // Verifica que index_stats está preenchido
        assert_eq!(explanation.index_stats.total_points, 2);
    }

    #[test]
    fn test_evaluate_condition_eq() {
        let cond = MetadataCondition::Eq("status".to_string(), json!("active"));
        let metadata = json!({"status": "active", "other": 123});

        let result = evaluate_condition(&cond, &metadata);
        assert_eq!(result.field, "status");
        assert_eq!(result.operator, "$eq");
        assert_eq!(result.expected, json!("active"));
        assert_eq!(result.actual, json!("active"));
        assert!(result.passed);
    }

    #[test]
    fn test_evaluate_condition_gt_fails() {
        let cond = MetadataCondition::Gt("price".to_string(), 100.0);
        let metadata = json!({"price": 50});

        let result = evaluate_condition(&cond, &metadata);
        assert_eq!(result.field, "price");
        assert_eq!(result.operator, "$gt");
        assert!(!result.passed);
        assert_eq!(result.actual, json!(50));
    }

    #[test]
    fn test_evaluate_condition_missing_field() {
        let cond = MetadataCondition::Eq("missing".to_string(), json!("value"));
        let metadata = json!({"other": "data"});

        let result = evaluate_condition(&cond, &metadata);
        assert_eq!(result.field, "missing");
        assert_eq!(result.actual, serde_json::Value::Null);
        assert!(!result.passed);
    }

    /// Verifica que `build_search_explanation` (usado pelo REST/gRPC)
    /// produz resultado **idêntico** a `VectorDB::search_explain`.
    ///
    /// Isso garante que os endpoints nunca divergem do core.
    #[test]
    fn test_build_search_explanation_matches_vectordb() {
        let (mut db, _temp) = create_test_db();
        create_test_collection(&mut db, "test", 3);

        let points = vec![
            Point::new(
                "p1",
                vec![1.0, 0.0, 0.0],
                json!({"category": "tech", "price": 50}),
            )
            .unwrap(),
            Point::new(
                "p2",
                vec![0.0, 1.0, 0.0],
                json!({"category": "science", "price": 200}),
            )
            .unwrap(),
            Point::new(
                "p3",
                vec![0.9, 0.1, 0.0],
                json!({"category": "tech", "price": 30}),
            )
            .unwrap(),
        ];
        db.upsert_points("test", points).unwrap();

        let query = vec![1.0, 0.0, 0.0];
        let limit = 10;
        let filter = MetadataFilter::from_json(json!({
            "category": "tech"
        }))
        .unwrap();

        // Resultado via VectorDB (API de alto nível)
        let via_db = db
            .search_explain("test", query.clone(), limit, Some(filter.clone()))
            .unwrap();

        // Resultado via build_search_explanation (função direta usada por REST/gRPC)
        let collection = db.get_collection("test").unwrap();
        let via_build = build_search_explanation(
            collection,
            &query,
            limit,
            Some(filter),
            None,
        )
        .unwrap();

        // Campos globais idênticos
        assert_eq!(via_db.query_vector_norm, via_build.query_vector_norm);
        assert_eq!(via_db.distance_metric, via_build.distance_metric);
        assert_eq!(via_db.candidates_scanned, via_build.candidates_scanned);
        assert_eq!(via_db.candidates_after_filter, via_build.candidates_after_filter);
        assert_eq!(via_db.results.len(), via_build.results.len());

        // Cada resultado individual idêntico
        for (a, b) in via_db.results.iter().zip(via_build.results.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.score, b.score);
            assert_eq!(a.distance_metric, b.distance_metric);
            assert_eq!(a.raw_distance, b.raw_distance);
            assert_eq!(a.similarity, b.similarity);
            assert_eq!(a.score_breakdown, b.score_breakdown);
            assert_eq!(a.rank_before_filter, b.rank_before_filter);
            assert_eq!(a.rank_after_filter, b.rank_after_filter);

            // filter_evaluation idêntico
            match (&a.filter_evaluation, &b.filter_evaluation) {
                (Some(fa), Some(fb)) => {
                    assert_eq!(fa.passed, fb.passed);
                    assert_eq!(fa.conditions.len(), fb.conditions.len());
                    for (ca, cb) in fa.conditions.iter().zip(fb.conditions.iter()) {
                        assert_eq!(ca.field, cb.field);
                        assert_eq!(ca.operator, cb.operator);
                        assert_eq!(ca.expected, cb.expected);
                        assert_eq!(ca.actual, cb.actual);
                        assert_eq!(ca.passed, cb.passed);
                    }
                }
                (None, None) => {}
                _ => panic!("filter_evaluation divergiu entre VectorDB e build_search_explanation"),
            }
        }

        // IndexStats idêntico
        assert_eq!(
            via_db.index_stats.total_points,
            via_build.index_stats.total_points
        );
        assert_eq!(
            via_db.index_stats.hnsw_layers,
            via_build.index_stats.hnsw_layers
        );
        assert_eq!(
            via_db.index_stats.ef_search_used,
            via_build.index_stats.ef_search_used
        );
        assert_eq!(
            via_db.index_stats.tombstones_skipped,
            via_build.index_stats.tombstones_skipped
        );
    }
}
