//! # Fusion Strategies for Hybrid Search
//!
//! Este módulo contém algoritmos de fusão para combinar rankings de
//! diferentes fontes (e.g. busca vetorial + BM25 keyword search).
//!
//! ## Estratégias disponíveis
//!
//! - **Weighted Score**: combina rankings ponderados por `alpha`.
//!   `score = alpha * 1/(k + rank_vec) + (1-alpha) * 1/(k + rank_bm25)`.
//!   Compatível com o comportamento original do hybrid search.
//!
//! - **RRF (Reciprocal Rank Fusion)**: soma pura de reciprocais de rank
//!   sem ponderação. `score = Σ 1/(k + rank_i)` para cada ranker.
//!   Produz resultados mais estáveis por tratar todos os rankers igualmente.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Constante RRF padrão (Reciprocal Rank Fusion). Típico: 60.
pub const DEFAULT_RRF_K: usize = 60;

/// Estratégia de fusão para combinar múltiplos rankings em hybrid search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FusionStrategy {
    /// Fusão ponderada por alpha: `alpha * 1/(k + rank_vec) + (1-alpha) * 1/(k + rank_bm25)`.
    /// Compatível com o comportamento original.
    WeightedScore {
        /// Peso da busca vetorial (0..=1). (1 - alpha) = peso keyword.
        alpha: f32,
    },
    /// Reciprocal Rank Fusion pura: `score = Σ 1/(k + rank_i)` para cada ranker.
    /// Todos os rankers têm peso igual.
    RRF {
        /// Constante RRF (default: 60). Valores maiores suavizam diferenças de rank.
        k: usize,
    },
}

impl Default for FusionStrategy {
    fn default() -> Self {
        FusionStrategy::WeightedScore { alpha: 0.5 }
    }
}

/// Reciprocal Rank Fusion (RRF) pura.
///
/// Combina múltiplos rankings em um único ranking fusionado.
/// Cada item recebe score `Σ 1/(k + rank_i)` onde `rank_i` é a posição
/// (1-based) do item no i-ésimo ranking. Itens ausentes em um ranking
/// recebem rank = `u32::MAX` (contribuição desprezível).
///
/// # Parâmetros
///
/// - `rankings`: slice de rankings, cada um sendo `Vec<(id, score)>` ordenado
///   por relevância (melhor primeiro). O score original é ignorado; apenas
///   a posição importa.
/// - `k`: constante RRF (default recomendado: 60). Valores maiores suavizam
///   as diferenças entre posições próximas no ranking.
/// - `limit`: número máximo de resultados a retornar.
///
/// # Exemplo
///
/// ```rust
/// use ferres_db_core::fusion::reciprocal_rank_fusion;
///
/// let vector_ranking = vec![
///     ("doc-1".to_string(), 0.95),
///     ("doc-2".to_string(), 0.80),
///     ("doc-3".to_string(), 0.70),
/// ];
/// let bm25_ranking = vec![
///     ("doc-2".to_string(), 5.2),
///     ("doc-4".to_string(), 4.1),
///     ("doc-1".to_string(), 3.0),
/// ];
///
/// let fused = reciprocal_rank_fusion(&[vector_ranking, bm25_ranking], 60, 10);
/// // doc-2 aparece bem nos dois rankings → score mais alto
/// assert_eq!(fused[0].0, "doc-2");
/// ```
pub fn reciprocal_rank_fusion(
    rankings: &[Vec<(String, f32)>],
    k: usize,
    limit: usize,
) -> Vec<(String, f32)> {
    if rankings.is_empty() {
        return Vec::new();
    }

    let k_f = k as f32;

    // Coleta todos os IDs únicos
    let all_ids: HashSet<String> = rankings
        .iter()
        .flat_map(|r| r.iter().map(|(id, _)| id.clone()))
        .collect();

    // Para cada ranking, mapeia id → rank (1-based)
    let rank_maps: Vec<HashMap<&str, u32>> = rankings
        .iter()
        .map(|ranking| {
            ranking
                .iter()
                .enumerate()
                .map(|(rank, (id, _))| (id.as_str(), rank as u32 + 1))
                .collect()
        })
        .collect();

    // Calcula score RRF para cada ID
    let mut combined: Vec<(String, f32)> = all_ids
        .into_iter()
        .map(|id| {
            let score: f32 = rank_maps
                .iter()
                .map(|rm| {
                    let rank = rm.get(id.as_str()).copied().unwrap_or(u32::MAX);
                    1.0 / (k_f + rank as f32)
                })
                .sum();
            (id, score)
        })
        .collect();

    combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    combined.truncate(limit);
    combined
}

/// Fusão ponderada (weighted score) usando Reciprocal Rank Fusion.
///
/// Combina dois rankings (vetorial e keyword) usando pesos alpha e (1-alpha).
/// Score = `alpha * 1/(k + rank_vec) + (1-alpha) * 1/(k + rank_bm25)`.
///
/// Este é o algoritmo original do FerresDB, mantido para backward compatibility.
///
/// # Parâmetros
///
/// - `vector_results`: ranking da busca vetorial `(id, score)`, melhor primeiro.
/// - `keyword_results`: ranking da busca BM25 `(id, score)`, melhor primeiro.
/// - `alpha`: peso da busca vetorial (0..=1). `(1-alpha)` = peso keyword.
/// - `k_rrf`: constante RRF (default: 60).
/// - `limit`: número máximo de resultados a retornar.
///
/// # Exemplo
///
/// ```rust
/// use ferres_db_core::fusion::weighted_fusion;
///
/// let vec_results = vec![
///     ("doc-1".to_string(), 0.95),
///     ("doc-2".to_string(), 0.80),
/// ];
/// let bm25_results = vec![
///     ("doc-2".to_string(), 5.2),
///     ("doc-3".to_string(), 4.1),
/// ];
///
/// let fused = weighted_fusion(&vec_results, &bm25_results, 0.5, 60, 10);
/// assert!(!fused.is_empty());
/// ```
pub fn weighted_fusion(
    vector_results: &[(String, f32)],
    keyword_results: &[(String, f32)],
    alpha: f32,
    k_rrf: usize,
    limit: usize,
) -> Vec<(String, f32)> {
    let k_f = k_rrf as f32;

    // Mapeia id → rank (1-based) para cada ranking
    let mut rank_vec: HashMap<&str, u32> = HashMap::new();
    for (rank, (id, _)) in vector_results.iter().enumerate() {
        rank_vec.insert(id.as_str(), rank as u32 + 1);
    }

    let mut rank_bm25: HashMap<&str, u32> = HashMap::new();
    for (rank, (id, _)) in keyword_results.iter().enumerate() {
        rank_bm25.insert(id.as_str(), rank as u32 + 1);
    }

    // Coleta todos os IDs únicos
    let all_ids: HashSet<&str> = rank_vec
        .keys()
        .chain(rank_bm25.keys())
        .copied()
        .collect();

    // Calcula score ponderado
    let mut combined: Vec<(String, f32)> = all_ids
        .into_iter()
        .map(|id| {
            let rv = rank_vec.get(id).copied().unwrap_or(u32::MAX);
            let rb = rank_bm25.get(id).copied().unwrap_or(u32::MAX);
            let score =
                alpha * (1.0 / (k_f + rv as f32)) + (1.0 - alpha) * (1.0 / (k_f + rb as f32));
            (id.to_string(), score)
        })
        .collect();

    combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    combined.truncate(limit);
    combined
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: cria um ranking a partir de IDs (score decrescente fictício).
    fn make_ranking(ids: &[&str]) -> Vec<(String, f32)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| (id.to_string(), 100.0 - i as f32))
            .collect()
    }

    #[test]
    fn test_rrf_basic() {
        // Dois rankings com overlap parcial
        let r1 = vec![
            ("a".to_string(), 10.0),
            ("b".to_string(), 8.0),
            ("c".to_string(), 6.0),
        ];
        let r2 = vec![
            ("b".to_string(), 5.0),
            ("d".to_string(), 4.0),
            ("a".to_string(), 3.0),
        ];

        let fused = reciprocal_rank_fusion(&[r1, r2], 60, 10);

        // "b" está em rank 2 no r1 e rank 1 no r2 → melhor score combinado
        // "a" está em rank 1 no r1 e rank 3 no r2
        // "b" deveria ter score mais alto porque soma das contribuições é maior
        assert!(!fused.is_empty());

        // Verifica que "b" está no topo (rank 2 + rank 1 = melhor soma)
        // b: 1/(60+2) + 1/(60+1) = 1/62 + 1/61 ≈ 0.01613 + 0.01639 = 0.03252
        // a: 1/(60+1) + 1/(60+3) = 1/61 + 1/63 ≈ 0.01639 + 0.01587 = 0.03226
        assert_eq!(fused[0].0, "b");
        assert_eq!(fused[1].0, "a");

        // Todos os 4 IDs devem estar presentes
        let ids: HashSet<_> = fused.iter().map(|x| x.0.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));
        assert!(ids.contains("d"));
    }

    #[test]
    fn test_rrf_no_overlap() {
        // Rankings completamente disjuntos
        let r1 = make_ranking(&["a", "b", "c"]);
        let r2 = make_ranking(&["x", "y", "z"]);

        let fused = reciprocal_rank_fusion(&[r1, r2], 60, 10);

        // Todos os 6 IDs devem aparecer
        assert_eq!(fused.len(), 6);
        let ids: HashSet<_> = fused.iter().map(|x| x.0.as_str()).collect();
        for expected in &["a", "b", "c", "x", "y", "z"] {
            assert!(ids.contains(expected), "missing {}", expected);
        }

        // Os primeiros de cada ranking devem ter score igual (ambos rank 1)
        // a: 1/(60+1) + 1/(60+MAX) ≈ 1/61
        // x: 1/(60+MAX) + 1/(60+1) ≈ 1/61
        let score_a = fused.iter().find(|x| x.0 == "a").unwrap().1;
        let score_x = fused.iter().find(|x| x.0 == "x").unwrap().1;
        assert!(
            (score_a - score_x).abs() < 1e-6,
            "Rank-1 items from different rankers should have equal scores"
        );
    }

    #[test]
    fn test_rrf_vs_weighted() {
        // Compara: RRF puro deve ser estável independente da escala dos scores
        let vec_results = vec![
            ("a".to_string(), 0.99),
            ("b".to_string(), 0.50),
            ("c".to_string(), 0.01),
        ];
        let bm25_results = vec![
            ("b".to_string(), 100.0), // BM25 scores são em escala muito diferente
            ("c".to_string(), 50.0),
            ("a".to_string(), 1.0),
        ];

        let rrf = reciprocal_rank_fusion(&[vec_results.clone(), bm25_results.clone()], 60, 10);
        let weighted =
            weighted_fusion(&vec_results, &bm25_results, 0.5, 60, 10);

        // Ambos devem produzir resultados válidos
        assert_eq!(rrf.len(), 3);
        assert_eq!(weighted.len(), 3);

        // RRF: "b" está em rank 2 (vec) e rank 1 (bm25) → melhor
        // "a" está em rank 1 (vec) e rank 3 (bm25)
        // Com alpha=0.5 e RRF, ambos métodos devem ter "b" ou "a" no topo
        // O importante é que ambos retornam todos os IDs
        let rrf_ids: HashSet<_> = rrf.iter().map(|x| x.0.as_str()).collect();
        let weighted_ids: HashSet<_> = weighted.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(rrf_ids, weighted_ids);
    }

    #[test]
    fn test_weighted_backward_compat() {
        // Verifica que weighted_fusion produz os mesmos scores que a implementação
        // original inline em collection.rs
        let vec_results = vec![
            ("a".to_string(), 0.9),
            ("b".to_string(), 0.7),
        ];
        let bm25_results = vec![
            ("b".to_string(), 5.0),
            ("c".to_string(), 3.0),
        ];

        let alpha = 0.6_f32;
        let k: usize = 60;
        let result = weighted_fusion(&vec_results, &bm25_results, alpha, k, 10);

        // Calcula manualmente o score esperado para "a"
        // rank_vec = 1, rank_bm25 = MAX
        let k_f = k as f32;
        let expected_a = alpha * (1.0 / (k_f + 1.0))
            + (1.0 - alpha) * (1.0 / (k_f + u32::MAX as f32));

        let actual_a = result.iter().find(|x| x.0 == "a").unwrap().1;
        assert!(
            (actual_a - expected_a).abs() < 1e-9,
            "expected {}, got {}",
            expected_a,
            actual_a
        );

        // Score para "b" (rank 2 em vec, rank 1 em bm25)
        let expected_b =
            alpha * (1.0 / (k_f + 2.0)) + (1.0 - alpha) * (1.0 / (k_f + 1.0));
        let actual_b = result.iter().find(|x| x.0 == "b").unwrap().1;
        assert!(
            (actual_b - expected_b).abs() < 1e-9,
            "expected {}, got {}",
            expected_b,
            actual_b
        );
    }

    #[test]
    fn test_rrf_limit() {
        let r1 = make_ranking(&["a", "b", "c", "d", "e"]);
        let r2 = make_ranking(&["f", "g", "h", "i", "j"]);

        let fused = reciprocal_rank_fusion(&[r1, r2], 60, 3);
        assert_eq!(fused.len(), 3);
    }

    #[test]
    fn test_rrf_empty_rankings() {
        let fused = reciprocal_rank_fusion(&[], 60, 10);
        assert!(fused.is_empty());

        let fused2 = reciprocal_rank_fusion(&[vec![], vec![]], 60, 10);
        assert!(fused2.is_empty());
    }

    #[test]
    fn test_rrf_single_ranking() {
        let r = make_ranking(&["a", "b", "c"]);
        let fused = reciprocal_rank_fusion(&[r], 60, 10);

        // Com um único ranking, a ordem deve ser preservada
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].0, "a");
        assert_eq!(fused[1].0, "b");
        assert_eq!(fused[2].0, "c");
    }

    #[test]
    fn test_weighted_fusion_extreme_alpha() {
        let vec_results = make_ranking(&["v1", "v2", "v3"]);
        let bm25_results = make_ranking(&["b1", "b2", "b3"]);

        // alpha = 1.0 → apenas vetor importa
        let result = weighted_fusion(&vec_results, &bm25_results, 1.0, 60, 3);
        assert_eq!(result[0].0, "v1");
        assert_eq!(result[1].0, "v2");
        assert_eq!(result[2].0, "v3");

        // alpha = 0.0 → apenas BM25 importa
        let result = weighted_fusion(&vec_results, &bm25_results, 0.0, 60, 3);
        assert_eq!(result[0].0, "b1");
        assert_eq!(result[1].0, "b2");
        assert_eq!(result[2].0, "b3");
    }

    #[test]
    fn test_fusion_strategy_default() {
        let default = FusionStrategy::default();
        assert_eq!(default, FusionStrategy::WeightedScore { alpha: 0.5 });
    }

    #[test]
    fn test_rrf_three_rankers() {
        // RRF genérico suporta N rankers
        let r1 = make_ranking(&["a", "b"]);
        let r2 = make_ranking(&["b", "c"]);
        let r3 = make_ranking(&["c", "a"]);

        let fused = reciprocal_rank_fusion(&[r1, r2, r3], 60, 10);

        // Todos devem aparecer
        assert_eq!(fused.len(), 3);

        // "a": rank 1 + absent + rank 2 → 1/61 + ~0 + 1/62
        // "b": rank 2 + rank 1 + absent → 1/62 + 1/61 + ~0
        // "c": absent + rank 2 + rank 1 → ~0 + 1/62 + 1/61
        // Todos devem ter scores muito próximos (simétrico)
        let scores: Vec<f32> = fused.iter().map(|x| x.1).collect();
        assert!(
            (scores[0] - scores[1]).abs() < 1e-6,
            "symmetric rankings should produce equal scores"
        );
        assert!(
            (scores[1] - scores[2]).abs() < 1e-6,
            "symmetric rankings should produce equal scores"
        );
    }
}
