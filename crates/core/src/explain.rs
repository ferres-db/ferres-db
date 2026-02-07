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

use crate::MetadataCondition;

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
    /// Score final (distância/similaridade).
    pub score: f32,
    /// Métrica de distância utilizada (e.g. "Cosine", "Euclidean").
    pub distance_metric: String,
    /// Distância bruta retornada pelo índice.
    pub raw_distance: f32,
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
/// resultados explicados individualmente e estatísticas do índice.
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

        // Verifica que cada resultado tem score_breakdown com vector_score
        for result in &explanation.results {
            assert!(
                result.score_breakdown.contains_key("vector_score"),
                "score_breakdown deve conter 'vector_score'"
            );
            assert_eq!(result.distance_metric, "Euclidean");
            assert!(result.rank_before_filter > 0);
        }

        // Verifica IndexStats
        assert_eq!(explanation.index_stats.total_points, 3);
        assert!(explanation.index_stats.ef_search_used > 0);
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
                    .map_or(true, |f| f.passed)
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
}
