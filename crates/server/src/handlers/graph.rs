//! # Graph Handlers — endpoints para relações entre pontos (grafos)
//!
//! Permite linkar dois pontos para construir um grafo não direcionado
//! persistido no campo `relations` de cada ponto, e consultar subgrafos
//! para visualização (dashboard).

use axum::{
    extract::{Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};

use ferres_db_core::graph;
use ferres_db_core::point::Point;

use crate::audit::{self, AuditResult};
use crate::auth::{check_user_permission, AuthenticatedUser};
use crate::error::{ApiError, ApiResult};
use crate::permissions::Action;
use crate::state::AppState;

// ─── Request/Response ───────────────────────────────────────────────────

/// Payload para linkagem de dois pontos.
#[derive(Debug, Deserialize)]
pub struct LinkPointsRequest {
    /// ID do ponto de origem (storage_id; para pontos sem namespace, o id lógico).
    pub from: String,
    /// ID do ponto de destino (storage_id).
    pub to: String,
}

/// Resposta do endpoint de link.
#[derive(Debug, Serialize)]
pub struct LinkPointsResponse {
    pub ok: bool,
    pub from: String,
    pub to: String,
}

// ─── Handler ─────────────────────────────────────────────────────────────

/// Handler para POST /api/v1/collections/{name}/points/link
///
/// Cria uma relação não direcionada entre dois pontos: atualiza o campo
/// `relations` em ambos. A persistência ocorre no próximo snapshot (save).
pub async fn link_points(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(payload): Json<LinkPointsRequest>,
) -> ApiResult<Json<LinkPointsResponse>> {
    let perm_result = check_user_permission(&user, &name, &Action::Write);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "link_points",
            &format!("collection:{name}"),
            serde_json::json!({
                "from": payload.from,
                "to": payload.to,
                "denied": true
            }),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: write on collection '{name}'"
        )));
    }

    if payload.from.is_empty() || payload.to.is_empty() {
        return Err(ApiError::invalid_payload(
            "from and to are required and cannot be empty",
        ));
    }

    let collection_arc = app_state
        .collections
        .get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let mut collection = crate::api_err!(collection_arc.write(), "failed to acquire write lock")?;
    collection.add_relation(&payload.from, &payload.to)?;
    collection.mark_dirty();
    drop(collection);

    let entry = audit::audit_entry(
        &user.username,
        "link_points",
        &format!("collection:{name}"),
        serde_json::json!({ "from": payload.from, "to": payload.to }),
        AuditResult::Success,
        None,
        None,
    );
    app_state.audit_logger.log(&entry);

    Ok(Json(LinkPointsResponse {
        ok: true,
        from: payload.from,
        to: payload.to,
    }))
}

// ─── Subgraph (para Graph Explorer no dashboard) ─────────────────────────

/// Query params para GET subgraph.
#[derive(Debug, Deserialize)]
pub struct SubgraphQuery {
    /// Se presente, retorna o subgrafo a partir deste nó (storage_id) em 1-hop.
    pub seed: Option<String>,
    /// Centro do subgrafo para BFS (usa com `depth`). Precedência sobre `seed` quando ambos presentes.
    pub center_id: Option<String>,
    /// Profundidade em saltos para BFS a partir de `center_id` (ex.: 2 = até 2 saltos).
    pub depth: Option<u32>,
    /// Limite de nós ao listar grafo inteiro (sem seed/center_id). Default 500.
    #[serde(default = "default_subgraph_limit")]
    pub limit: usize,
}

fn default_subgraph_limit() -> usize {
    500
}

/// Nó do grafo para o frontend (id = storage_id para consistência).
#[derive(Debug, Serialize)]
pub struct GraphNodeResponse {
    pub id: String,
    pub metadata: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relations: Option<Vec<String>>,
    /// Vetor omitido para reduzir payload; detalhes completos via GET /points/{id}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

/// Aresta do grafo (serializada como "edges" no JSON para o frontend).
#[derive(Debug, Serialize)]
pub struct GraphEdgeResponse {
    pub source: String,
    pub target: String,
}

/// Resposta do endpoint subgraph: `{ "nodes": [...], "edges": [...] }`.
#[derive(Debug, Serialize)]
pub struct SubgraphResponse {
    pub nodes: Vec<GraphNodeResponse>,
    pub edges: Vec<GraphEdgeResponse>,
}

fn point_to_graph_node(p: &Point, include_vector: bool) -> GraphNodeResponse {
    GraphNodeResponse {
        id: p.storage_id(),
        metadata: p.metadata.clone(),
        namespace: p.namespace.clone(),
        created_at: p.created_at,
        relations: p.relations.clone(),
        vector: if include_vector {
            Some(p.vector.clone())
        } else {
            None
        },
    }
}

/// Handler para GET /api/v1/collections/{name}/graph/subgraph
///
/// - Com `center_id` + `depth`: BFS a partir do centro (até `depth` saltos); retorna `nodes` e `edges`.
/// - Com `seed` (sem center_id): 1-hop a partir do seed; retorna `nodes` e `edges`.
/// - Sem center_id nem seed: nós com relações até `limit`; retorna `nodes` e `edges`.
pub async fn get_subgraph(
    State(app_state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Query(query): Query<SubgraphQuery>,
) -> ApiResult<Json<SubgraphResponse>> {
    let collection_arc = app_state
        .collections
        .get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = crate::api_err!(collection_arc.read(), "failed to acquire read lock")?;

    let limit = query.limit.min(2000);

    // Preferência: center_id + depth (BFS)
    if let (Some(center_id), Some(depth)) = (&query.center_id, query.depth) {
        if center_id.is_empty() {
            return Err(ApiError::invalid_payload("center_id cannot be empty"));
        }
        let get_point = |id: &str| collection.get(id).cloned();
        let points =
            graph::traverse_bfs(get_point, center_id, depth).map_err(|e| ApiError::from(e))?;
        let node_ids: std::collections::HashSet<String> =
            points.iter().map(|p| p.storage_id()).collect();
        let nodes: Vec<GraphNodeResponse> = points
            .iter()
            .map(|p| point_to_graph_node(p, false))
            .collect();
        let mut edges = Vec::new();
        for p in &points {
            let sid = p.storage_id();
            if let Some(ref rels) = p.relations {
                for t in rels {
                    if node_ids.contains(t) {
                        edges.push(GraphEdgeResponse {
                            source: sid.clone(),
                            target: t.clone(),
                        });
                    }
                }
            }
        }
        return Ok(Json(SubgraphResponse { nodes, edges }));
    }

    // seed: 1-hop (compatibilidade)
    if let Some(seed_id) = &query.seed {
        if seed_id.is_empty() {
            return Err(ApiError::invalid_payload("seed cannot be empty"));
        }
        let seed_point = collection
            .get(seed_id)
            .ok_or_else(|| ApiError::point_not_found(seed_id))?;

        let mut node_ids = std::collections::HashSet::<String>::new();
        node_ids.insert(seed_point.storage_id());

        let mut edges = Vec::new();
        if let Some(ref rels) = seed_point.relations {
            for target in rels {
                node_ids.insert(target.clone());
                edges.push(GraphEdgeResponse {
                    source: seed_point.storage_id(),
                    target: target.clone(),
                });
            }
        }

        let mut nodes: Vec<GraphNodeResponse> = vec![point_to_graph_node(seed_point, false)];
        for id in &node_ids {
            if *id == seed_point.storage_id() {
                continue;
            }
            if let Some(p) = collection.get(id) {
                nodes.push(point_to_graph_node(p, false));
                if let Some(ref rels) = p.relations {
                    for t in rels {
                        if node_ids.contains(t) {
                            edges.push(GraphEdgeResponse {
                                source: p.storage_id(),
                                target: t.clone(),
                            });
                        }
                    }
                }
            }
        }

        return Ok(Json(SubgraphResponse { nodes, edges }));
    }

    // Sem center_id nem seed: todos os pontos com relações, até `limit` nós
    let points = collection.points_owned();
    let mut node_ids = std::collections::HashSet::<String>::new();
    let mut edges = Vec::new();
    let mut nodes = Vec::new();

    for point in &points {
        if node_ids.len() >= limit {
            break;
        }
        let sid = point.storage_id();
        let rels = match &point.relations {
            Some(r) if !r.is_empty() => r,
            _ => continue,
        };
        node_ids.insert(sid.clone());
        for target in rels {
            node_ids.insert(target.clone());
            edges.push(GraphEdgeResponse {
                source: sid.clone(),
                target: target.clone(),
            });
        }
    }

    for point in &points {
        if !node_ids.contains(&point.storage_id()) {
            continue;
        }
        nodes.push(point_to_graph_node(point, false));
    }

    Ok(Json(SubgraphResponse { nodes, edges }))
}
