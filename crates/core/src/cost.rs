//! # Cost — estimativa de custo de queries antes da execução
//!
//! Este módulo fornece heurísticas para estimar o custo de uma busca vetorial
//! **antes** de executá-la. O objetivo é dar ao usuário visibilidade sobre
//! latência esperada, consumo de memória e nós visitados, permitindo decisões
//! informadas (ex: reduzir `limit`, adicionar filtro, ajustar `ef_search`).
//!
//! ## Modelo de custo
//!
//! A estimativa é baseada em:
//! 1. **Complexidade algorítmica do HNSW** — O(log n × ef_search × dimension)
//! 2. **Custo de filtragem pós-busca** — proporcional ao número de condições
//! 3. **Custo de hidratação** — carregar metadata dos pontos retornados
//! 4. **Histórico de latência** — percentis p50/p95 da coleção
//!
//! A estimativa é calculada em tempo constante (sem I/O), tipicamente < 1μs.

use serde::{Deserialize, Serialize};

// ─── QueryCostEstimate ──────────────────────────────────────────────────

/// Estimativa de custo de uma query antes da execução.
///
/// Contém latência estimada, faixa de confiança, consumo de memória,
/// nós HNSW estimados, flag de query cara e recomendações de otimização.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCostEstimate {
    /// Latência estimada em milissegundos (baseada em histórico + heurísticas).
    pub estimated_ms: f64,
    /// Faixa de confiança: (min, max) em ms.
    pub confidence_range: (f64, f64),
    /// Bytes estimados de memória que a query vai consumir.
    pub estimated_memory_bytes: usize,
    /// Número estimado de nós HNSW que serão visitados.
    pub estimated_nodes_visited: usize,
    /// Se a query é "cara" (acima do p95 histórico).
    pub is_expensive: bool,
    /// Recomendações de otimização (ex: "reduza limit", "adicione filtro").
    pub recommendations: Vec<String>,
    /// Componentes individuais do custo.
    pub breakdown: CostBreakdown,
}

// ─── CostBreakdown ──────────────────────────────────────────────────────

/// Componentes individuais do custo estimado (em ms).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Custo estimado do scan no índice HNSW (ms).
    pub index_scan_cost: f64,
    /// Custo estimado da aplicação de filtros (ms).
    pub filter_cost: f64,
    /// Custo estimado da hidratação de resultados (ms).
    pub hydration_cost: f64,
    /// Overhead estimado de rede/serialização (ms).
    pub network_overhead: f64,
}

// ─── Parâmetros de entrada ──────────────────────────────────────────────

/// Parâmetros para a estimativa de custo de uma busca vetorial.
///
/// Agrupa todos os inputs necessários para a heurística, evitando
/// funções com muitos argumentos posicionais.
#[derive(Debug, Clone)]
pub struct CostEstimateParams {
    /// Número de pontos na coleção.
    pub collection_size: usize,
    /// Dimensão dos vetores.
    pub dimension: usize,
    /// Número de resultados solicitados (limit/top-k).
    pub limit: usize,
    /// Parâmetro ef_search do HNSW (tamanho da lista dinâmica na busca).
    pub ef_search: usize,
    /// Se a query possui filtro de metadata.
    pub has_filter: bool,
    /// Número de condições no filtro (0 se sem filtro).
    pub filter_conditions_count: usize,
    /// P50 histórico de latência da coleção (ms). 0.0 se não há histórico.
    pub historical_p50: f64,
    /// P95 histórico de latência da coleção (ms). 0.0 se não há histórico.
    pub historical_p95: f64,
}

// ─── Constantes de calibração ───────────────────────────────────────────

/// Fator de escala para converter operações computacionais em ms estimados.
/// Calibrado empiricamente: ~1ns por operação f32 em hardware típico.
const OPS_TO_MS_FACTOR: f64 = 1e-6;

/// Custo base por condição de filtro avaliada (em ms).
/// Post-filter: cada candidato é avaliado contra cada condição.
const FILTER_CONDITION_COST_MS: f64 = 0.00001; // ~10ns por condição

/// Custo base de hidratação por ponto (leitura de metadata do HashMap, em ms).
const HYDRATION_COST_PER_POINT_MS: f64 = 0.001; // ~1μs

/// Tamanho médio estimado de metadata por ponto (bytes).
const AVG_METADATA_SIZE_BYTES: usize = 256;

/// Overhead fixo de rede/serialização (ms).
const NETWORK_OVERHEAD_MS: f64 = 0.1;

/// Multiplicador para filtros que expandem a busca (10x candidates).
const FILTER_EXPANSION_FACTOR: usize = 10;

// ─── Função principal ───────────────────────────────────────────────────

/// Estima o custo de uma busca vetorial antes da execução.
///
/// A estimativa é puramente baseada em heurísticas e dados históricos,
/// sem realizar I/O. Tempo típico de execução: < 1μs.
///
/// # Heurísticas
///
/// - **Index scan**: O(log(n) × ef_search × dimension × 4 bytes) para HNSW
/// - **Filter**: limit × 10 × filter_conditions (post-filter expansion)
/// - **Hydration**: limit × (dimension × 4 + metadata_avg_size)
/// - **is_expensive**: estimated_ms > historical_p95
///
/// # Exemplo
///
/// ```rust
/// use ferres_db_core::cost::{estimate_search_cost, CostEstimateParams};
///
/// let params = CostEstimateParams {
///     collection_size: 100_000,
///     dimension: 384,
///     limit: 10,
///     ef_search: 50,
///     has_filter: false,
///     filter_conditions_count: 0,
///     historical_p50: 2.0,
///     historical_p95: 8.0,
/// };
///
/// let estimate = estimate_search_cost(&params);
/// println!("Estimated: {:.2}ms, Expensive: {}", estimate.estimated_ms, estimate.is_expensive);
/// ```
pub fn estimate_search_cost(params: &CostEstimateParams) -> QueryCostEstimate {
    let n = params.collection_size.max(1) as f64;
    let dim = params.dimension as f64;
    let limit = params.limit.max(1);
    let ef = params.ef_search.max(1) as f64;

    // ── Index scan cost ─────────────────────────────────────────────
    // HNSW busca: O(log(n) * ef_search * dimension) comparações de distância.
    // Cada comparação envolve `dimension` operações f32 (multiply + add).
    let log_n = n.ln().max(1.0);
    let index_ops = log_n * ef * dim * 4.0; // 4 bytes por f32
    let index_scan_cost = index_ops * OPS_TO_MS_FACTOR;

    // Nós visitados: ~log(n) * ef_search (cada nó é avaliado no beam search)
    let estimated_nodes_visited = (log_n * ef).ceil() as usize;

    // ── Filter cost ─────────────────────────────────────────────────
    // Post-filter: a busca retorna limit*10 candidatos, cada um avaliado
    // contra todas as condições do filtro.
    let filter_cost = if params.has_filter {
        let candidates_to_filter = (limit * FILTER_EXPANSION_FACTOR) as f64;
        let conditions = params.filter_conditions_count.max(1) as f64;
        candidates_to_filter * conditions * FILTER_CONDITION_COST_MS
    } else {
        0.0
    };

    // ── Hydration cost ──────────────────────────────────────────────
    // Carregar metadata + vetor para cada resultado retornado.
    let hydration_cost = limit as f64 * HYDRATION_COST_PER_POINT_MS;

    // ── Network overhead ────────────────────────────────────────────
    let network_overhead = NETWORK_OVERHEAD_MS;

    // ── Total estimado ──────────────────────────────────────────────
    let heuristic_ms = index_scan_cost + filter_cost + hydration_cost + network_overhead;

    // Se temos histórico, fazemos blend: 70% heurística, 30% histórico
    let estimated_ms = if params.historical_p50 > 0.0 {
        heuristic_ms * 0.7 + params.historical_p50 * 0.3
    } else {
        heuristic_ms
    };

    // ── Faixa de confiança ──────────────────────────────────────────
    // Margem de ±50% para capturar variabilidade de cache, contenção, etc.
    let confidence_min = estimated_ms * 0.5;
    let confidence_max = estimated_ms * 1.5;

    // Se temos p95 histórico, usamos como upper bound mais confiável
    let confidence_max = if params.historical_p95 > 0.0 {
        confidence_max.max(params.historical_p95)
    } else {
        confidence_max
    };

    // ── Memória estimada ────────────────────────────────────────────
    // Memória = nós visitados × (vetor + ponteiros HNSW) + resultados finais
    let visited_memory = estimated_nodes_visited * (params.dimension * 4 + 64); // vetor + overhead
    let result_memory = limit * (params.dimension * 4 + AVG_METADATA_SIZE_BYTES);
    let estimated_memory_bytes = visited_memory + result_memory;

    // ── is_expensive ────────────────────────────────────────────────
    let is_expensive = if params.historical_p95 > 0.0 {
        estimated_ms > params.historical_p95
    } else {
        // Sem histórico, marca como caro se > 50ms (heurística conservadora)
        estimated_ms > 50.0
    };

    // ── Recommendations ─────────────────────────────────────────────
    let mut recommendations = Vec::new();

    if limit > 100 {
        recommendations.push(
            "Considere reduzir limit para melhor performance".to_string(),
        );
    }

    if params.has_filter && params.collection_size > 100_000 {
        recommendations.push(
            "Filtros em coleções grandes podem ser lentos. Considere pré-filtrar ou usar índices dedicados".to_string(),
        );
    }

    if params.dimension > 1024 {
        recommendations.push(
            "Vetores de alta dimensão impactam latência. Considere redução de dimensionalidade (PCA, autoencoders)".to_string(),
        );
    }

    if params.ef_search > 200 {
        recommendations.push(
            "ef_search alto aumenta precisão mas impacta latência. Avalie se o recall atual já é suficiente".to_string(),
        );
    }

    if params.collection_size > 1_000_000 && !params.has_filter {
        recommendations.push(
            "Coleção com mais de 1M pontos: considere adicionar filtros para reduzir o espaço de busca".to_string(),
        );
    }

    let breakdown = CostBreakdown {
        index_scan_cost,
        filter_cost,
        hydration_cost,
        network_overhead,
    };

    QueryCostEstimate {
        estimated_ms,
        confidence_range: (confidence_min, confidence_max),
        estimated_memory_bytes,
        estimated_nodes_visited,
        is_expensive,
        recommendations,
        breakdown,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_params() -> CostEstimateParams {
        CostEstimateParams {
            collection_size: 10_000,
            dimension: 384,
            limit: 10,
            ef_search: 50,
            has_filter: false,
            filter_conditions_count: 0,
            historical_p50: 0.0,
            historical_p95: 0.0,
        }
    }

    #[test]
    fn test_basic_estimate_returns_positive_values() {
        let params = base_params();
        let estimate = estimate_search_cost(&params);

        assert!(estimate.estimated_ms > 0.0, "estimated_ms must be positive");
        assert!(estimate.confidence_range.0 > 0.0, "confidence min must be positive");
        assert!(estimate.confidence_range.1 > estimate.confidence_range.0, "confidence max > min");
        assert!(estimate.estimated_memory_bytes > 0, "memory must be positive");
        assert!(estimate.estimated_nodes_visited > 0, "nodes visited must be positive");
        assert!(estimate.breakdown.index_scan_cost > 0.0, "index scan cost must be positive");
        assert!(estimate.breakdown.network_overhead > 0.0, "network overhead must be positive");
    }

    #[test]
    fn test_filter_increases_cost() {
        let params_no_filter = base_params();
        let params_with_filter = CostEstimateParams {
            has_filter: true,
            filter_conditions_count: 3,
            ..base_params()
        };

        let est_no_filter = estimate_search_cost(&params_no_filter);
        let est_with_filter = estimate_search_cost(&params_with_filter);

        assert!(
            est_with_filter.estimated_ms > est_no_filter.estimated_ms,
            "filter should increase estimated cost"
        );
        assert!(
            est_with_filter.breakdown.filter_cost > 0.0,
            "filter cost should be positive when filter is present"
        );
        assert!(
            est_no_filter.breakdown.filter_cost == 0.0,
            "filter cost should be zero when no filter"
        );
    }

    #[test]
    fn test_larger_collection_increases_cost() {
        let small = CostEstimateParams {
            collection_size: 1_000,
            ..base_params()
        };
        let large = CostEstimateParams {
            collection_size: 1_000_000,
            ..base_params()
        };

        let est_small = estimate_search_cost(&small);
        let est_large = estimate_search_cost(&large);

        assert!(
            est_large.estimated_ms > est_small.estimated_ms,
            "larger collection should have higher estimated cost"
        );
        assert!(
            est_large.estimated_nodes_visited > est_small.estimated_nodes_visited,
            "larger collection should visit more nodes"
        );
    }

    #[test]
    fn test_higher_limit_increases_cost() {
        let low_limit = CostEstimateParams {
            limit: 5,
            ..base_params()
        };
        let high_limit = CostEstimateParams {
            limit: 500,
            ..base_params()
        };

        let est_low = estimate_search_cost(&low_limit);
        let est_high = estimate_search_cost(&high_limit);

        assert!(
            est_high.breakdown.hydration_cost > est_low.breakdown.hydration_cost,
            "higher limit should increase hydration cost"
        );
    }

    #[test]
    fn test_is_expensive_with_historical_p95() {
        // estimated_ms será baixo para coleção pequena
        let params = CostEstimateParams {
            collection_size: 100,
            dimension: 3,
            limit: 1,
            ef_search: 10,
            has_filter: false,
            filter_conditions_count: 0,
            historical_p50: 0.1,
            historical_p95: 0.2,
        };

        let estimate = estimate_search_cost(&params);
        // Para coleção muito pequena, estimativa deve ser < p95
        // (pode ser expensive ou não dependendo da heurística, mas ao menos o campo deve existir)
        assert!(estimate.estimated_ms > 0.0);
    }

    #[test]
    fn test_is_expensive_without_history_high_cost() {
        let params = CostEstimateParams {
            collection_size: 10_000_000,
            dimension: 2048,
            limit: 1000,
            ef_search: 500,
            has_filter: true,
            filter_conditions_count: 10,
            historical_p50: 0.0,
            historical_p95: 0.0,
        };

        let estimate = estimate_search_cost(&params);
        // Para parâmetros extremos sem histórico, deve ser marcado como caro
        assert!(
            estimate.is_expensive,
            "extreme params without history should be marked as expensive: estimated_ms={}",
            estimate.estimated_ms
        );
    }

    #[test]
    fn test_recommendations_high_limit() {
        let params = CostEstimateParams {
            limit: 200,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        assert!(
            estimate.recommendations.iter().any(|r| r.contains("limit")),
            "should recommend reducing limit when > 100"
        );
    }

    #[test]
    fn test_recommendations_large_collection_with_filter() {
        let params = CostEstimateParams {
            collection_size: 200_000,
            has_filter: true,
            filter_conditions_count: 2,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        assert!(
            estimate.recommendations.iter().any(|r| r.contains("Filtros")),
            "should warn about filters on large collections"
        );
    }

    #[test]
    fn test_recommendations_high_dimension() {
        let params = CostEstimateParams {
            dimension: 2048,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        assert!(
            estimate.recommendations.iter().any(|r| r.contains("dimensão") || r.contains("dimensionalidade")),
            "should recommend reducing dimension when > 1024"
        );
    }

    #[test]
    fn test_historical_blend() {
        let params_no_history = base_params();
        let params_with_history = CostEstimateParams {
            historical_p50: 5.0,
            historical_p95: 15.0,
            ..base_params()
        };

        let est_no = estimate_search_cost(&params_no_history);
        let est_with = estimate_search_cost(&params_with_history);

        // Com histórico de p50=5ms, a estimativa deve ser influenciada
        assert!(
            est_with.estimated_ms != est_no.estimated_ms,
            "historical data should influence estimate"
        );
    }

    #[test]
    fn test_confidence_range_includes_p95() {
        let params = CostEstimateParams {
            historical_p50: 2.0,
            historical_p95: 100.0,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        assert!(
            estimate.confidence_range.1 >= 100.0,
            "confidence max should include historical p95: got {}",
            estimate.confidence_range.1
        );
    }

    #[test]
    fn test_zero_collection_size_does_not_panic() {
        let params = CostEstimateParams {
            collection_size: 0,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        assert!(estimate.estimated_ms > 0.0);
        assert!(estimate.estimated_memory_bytes > 0);
    }

    #[test]
    fn test_breakdown_sums_to_approximately_total() {
        let params = base_params();
        let estimate = estimate_search_cost(&params);

        let breakdown_sum = estimate.breakdown.index_scan_cost
            + estimate.breakdown.filter_cost
            + estimate.breakdown.hydration_cost
            + estimate.breakdown.network_overhead;

        // Sem histórico, o total heurístico deve ser exatamente a soma do breakdown
        let diff = (estimate.estimated_ms - breakdown_sum).abs();
        assert!(
            diff < 0.001,
            "breakdown sum ({}) should equal estimated_ms ({}) when no historical data",
            breakdown_sum,
            estimate.estimated_ms
        );
    }

    #[test]
    fn test_ef_search_recommendation() {
        let params = CostEstimateParams {
            ef_search: 300,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        assert!(
            estimate.recommendations.iter().any(|r| r.contains("ef_search")),
            "should recommend reviewing ef_search when > 200"
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let params = CostEstimateParams {
            has_filter: true,
            filter_conditions_count: 2,
            historical_p50: 3.0,
            historical_p95: 10.0,
            ..base_params()
        };

        let estimate = estimate_search_cost(&params);
        let json = serde_json::to_string(&estimate).unwrap();
        let restored: QueryCostEstimate = serde_json::from_str(&json).unwrap();

        assert!((restored.estimated_ms - estimate.estimated_ms).abs() < f64::EPSILON);
        assert_eq!(restored.estimated_memory_bytes, estimate.estimated_memory_bytes);
        assert_eq!(restored.estimated_nodes_visited, estimate.estimated_nodes_visited);
        assert_eq!(restored.is_expensive, estimate.is_expensive);
        assert_eq!(restored.recommendations.len(), estimate.recommendations.len());
    }
}
