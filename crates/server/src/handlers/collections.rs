//! # Collection Handlers — handlers para gerenciamento de coleções

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use std::sync::Arc;
use std::sync::RwLock;

use ferres_db_core::{
    Collection, CollectionConfig, DistanceMetric, FileStorage, QuantizationConfig,
    TieredStorageConfig,
};

use crate::api_err;
use crate::audit::{self, AuditResult};
use crate::auth::{check_user_permission, AuthenticatedUser};
use crate::error::{ApiError, ApiResult};
use crate::permissions::Action;
use crate::state::AppState;
use crate::time::unix_now;

// ─── Request/Response Types ──────────────────────────────────────────────

// Validação de nome: apenas letras, números, hífens e underscores.
lazy_static::lazy_static! {
    static ref VALID_NAME_REGEX: regex::Regex =
        regex::Regex::new(r"^[a-zA-Z0-9_-]+$")
            .unwrap_or_else(|e| unreachable!("VALID_NAME_REGEX is a hardcoded literal: {e}"));
}

/// Validador customizado para nome de coleção usando validator crate.
fn validate_collection_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::new("name_cannot_be_empty"));
    }
    if !VALID_NAME_REGEX.is_match(name) {
        return Err(ValidationError::new("name_invalid_characters"));
    }
    Ok(())
}

/// Payload para criação de coleção.
#[derive(Debug, Deserialize, Validate)]
pub struct CreateCollectionRequest {
    #[validate(custom(function = "validate_collection_name"))]
    pub name: String,

    #[validate(range(min = 1, max = 4096))]
    pub dimension: usize,

    pub distance: DistanceMetric,
    /// Habilita índice BM25 para busca híbrida. Padrão: false.
    #[serde(default)]
    pub enable_bm25: bool,
    /// Chave em metadata usada como texto para BM25. Padrão: "text".
    #[serde(default = "default_bm25_text_field")]
    pub bm25_text_field: String,
    /// Configuração de quantização de vetores. Padrão: `None` (sem quantização).
    ///
    /// - SQ8: `{"Scalar": {"dtype": "Int8"}}`
    /// - PolarQuant: `{"Polar": {"bits_per_angle": 8}}`
    #[serde(default)]
    pub quantization: QuantizationConfig,
    /// Período de retenção em dias (WAL e dados antigos). None = manter indefinidamente.
    #[serde(default)]
    pub retention_days: Option<u32>,
}

fn default_bm25_text_field() -> String {
    "text".to_string()
}

/// Resposta de criação de coleção.
#[derive(Debug, Serialize)]
pub struct CreateCollectionResponse {
    pub name: String,
    pub dimension: usize,
    pub distance: DistanceMetric,
    pub created_at: u64,
}

/// Resposta de listagem de coleções.
#[derive(Debug, Serialize)]
pub struct ListCollectionsResponse {
    pub collections: Vec<CollectionListItem>,
}

/// Item da listagem de coleções.
#[derive(Debug, Serialize)]
pub struct CollectionListItem {
    pub name: String,
    pub dimension: usize,
    pub num_points: usize,
    pub created_at: u64,
    pub distance: DistanceMetric,
    /// Retenção em dias (None = indefinido).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}

/// Query params para GET /api/v1/collections.
#[derive(Debug, Default, Deserialize)]
pub struct ListCollectionsQuery {
    /// Quando definido, retorna apenas coleções que têm pelo menos um ponto neste namespace.
    pub namespace: Option<String>,
}

/// Resposta de detalhes de coleção.
#[derive(Debug, Serialize)]
pub struct GetCollectionResponse {
    pub name: String,
    pub dimension: usize,
    pub num_points: usize,
    pub last_updated: u64,
    pub distance: DistanceMetric,
    pub stats: CollectionStatsResponse,
    /// Quantização de vetores: `None`, `Scalar` (SQ8) ou `Polar` (PolarQuant).
    pub quantization: QuantizationConfig,
    /// BM25 habilitado para busca híbrida.
    pub enable_bm25: bool,
    /// Campo em metadata usado como texto para BM25.
    pub bm25_text_field: String,
    /// Configuração de tiered storage (Hot/Warm/Cold).
    pub tiered_storage: TieredStorageConfig,
    /// Retenção em dias (None = indefinido).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}

/// Estatísticas da coleção na resposta.
#[derive(Debug, Serialize)]
pub struct CollectionStatsResponse {
    pub index_size_bytes: usize,
}

/// Body para PATCH /api/v1/collections/{name} (atualizar retenção).
#[derive(Debug, Deserialize)]
pub struct PatchCollectionRetentionBody {
    /// Retenção em dias; null ou omitido = manter indefinidamente.
    pub retention_days: Option<u32>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────

/// Handler para POST /api/v1/collections
///
/// Cria uma nova coleção (Editor ou Admin, com verificação granular de permissão Create).
pub async fn create_collection(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Json(payload): Json<CreateCollectionRequest>,
) -> ApiResult<(StatusCode, Json<CreateCollectionResponse>)> {
    // Verificação de permissão granular (Create)
    let perm_result = check_user_permission(&user, &payload.name, &Action::Create);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "create_collection",
            &format!("collection:{}", payload.name),
            serde_json::json!({"denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: create collection '{}'",
            payload.name
        )));
    }

    // Valida o payload usando validator crate
    payload.validate().map_err(|e| {
        let mut messages = Vec::new();
        for (field, errors) in e.field_errors() {
            for error in errors {
                let msg = match error.code.as_ref() {
                    "name_cannot_be_empty" => "name cannot be empty".to_string(),
                    "name_invalid_characters" => {
                        "name can only contain letters, numbers, hyphens, and underscores"
                            .to_string()
                    }
                    "range" => "dimension must be between 1 and 4096".to_string(),
                    _ => error
                        .message
                        .as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| format!("invalid {field}")),
                };
                messages.push(msg);
            }
        }
        ApiError::invalid_payload(messages.join(", "))
    })?;

    let config = CollectionConfig {
        name: payload.name.clone(),
        dimension: payload.dimension,
        distance: payload.distance,
        hnsw: Default::default(),
        search_cache_size: 0,
        enable_bm25: payload.enable_bm25,
        bm25_text_field: payload.bm25_text_field.clone(),
        quantization: payload.quantization.clone(),
        tiered_storage: Default::default(),
        retention_days: payload.retention_days,
    };

    // Cria a coleção
    let created_at = unix_now();

    // Verifica se a coleção já existe
    if app_state.collections.contains_key(&payload.name) {
        return Err(ApiError::collection_already_exists(&payload.name));
    }

    // Valida a configuração
    if config.dimension == 0 {
        return Err(ApiError::invalid_payload(
            "dimension must be greater than 0",
        ));
    }

    // Cria a nova coleção
    let collection = Collection::new(config.clone());
    let collection_arc = Arc::new(RwLock::new(collection));

    // Insere no mapa de coleções
    app_state
        .collections
        .insert(payload.name.clone(), collection_arc.clone());

    // Inicializa estatísticas de queries para a nova coleção
    app_state
        .query_stats
        .insert(payload.name.clone(), crate::state::QueryStats::new());

    // Atualiza gauge de coleções ativas
    crate::metrics::COLLECTIONS_ACTIVE.set(app_state.collections.len() as f64);

    // Salva no disco
    let collection_dir = app_state
        .config
        .storage_path
        .join("collections")
        .join(&payload.name);
    {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        FileStorage::save_collection(
            &collection,
            &collection_dir,
            app_state.config.binary_snapshot,
            app_state.config.namespace_physical_isolation,
        )
        .map_err(ApiError::from)?;
    }

    // Marca como limpa após salvar
    {
        let collection = api_err!(collection_arc.write(), "failed to acquire write lock")?;
        collection.mark_clean();
    }

    // Audit trail
    {
        let entry = audit::audit_entry(
            &user.username,
            "create_collection",
            &format!("collection:{}", config.name),
            serde_json::json!({"dimension": config.dimension}),
            AuditResult::Success,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
    }

    Ok((
        StatusCode::CREATED,
        Json(CreateCollectionResponse {
            name: config.name,
            dimension: config.dimension,
            distance: config.distance,
            created_at,
        }),
    ))
}

/// Handler para GET /api/v1/collections
///
/// Lista todas as coleções. Se `namespace` for passado, retorna apenas coleções que têm
/// pelo menos um ponto nesse namespace.
pub async fn list_collections(
    State(app_state): State<AppState>,
    Query(params): Query<ListCollectionsQuery>,
) -> ApiResult<Json<ListCollectionsResponse>> {
    let mut collections = Vec::new();
    let filter_namespace = params.namespace.as_deref().filter(|s| !s.is_empty());

    for entry in app_state.collections.iter() {
        let name = entry.key();
        let collection_arc = entry.value();

        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

        if let Some(ns) = filter_namespace {
            let has_namespace = collection
                .points_owned()
                .iter()
                .any(|p| p.namespace.as_deref() == Some(ns));
            if !has_namespace {
                continue;
            }
        }

        let config = collection.config();
        let num_points = collection.len();

        // Calcula created_at a partir do ponto mais antigo (se houver)
        let created_at = collection
            .points_owned()
            .iter()
            .map(|p| p.created_at)
            .min()
            .unwrap_or_else(unix_now);

        collections.push(CollectionListItem {
            name: name.clone(),
            dimension: config.dimension,
            num_points,
            created_at,
            distance: config.distance,
            retention_days: config.retention_days,
        });
    }

    Ok(Json(ListCollectionsResponse { collections }))
}

/// Handler para GET /api/v1/collections/{name}
///
/// Retorna detalhes de uma coleção específica.
pub async fn get_collection(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<GetCollectionResponse>> {
    let collection_arc = app_state
        .collections
        .get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    let config = collection.config();
    let num_points = collection.len();

    // Calcula last_updated a partir do ponto mais recente (se houver)
    let last_updated = collection
        .points_owned()
        .iter()
        .map(|p| p.created_at)
        .max()
        .unwrap_or_else(unix_now);

    // Estima tamanho do índice (aproximação)
    let index_size_bytes = num_points * config.dimension * 4; // 4 bytes por f32

    Ok(Json(GetCollectionResponse {
        name: name.clone(),
        dimension: config.dimension,
        num_points,
        last_updated,
        distance: config.distance,
        stats: CollectionStatsResponse { index_size_bytes },
        quantization: config.quantization.clone(),
        enable_bm25: config.enable_bm25,
        bm25_text_field: config.bm25_text_field.clone(),
        tiered_storage: config.tiered_storage.clone(),
        retention_days: config.retention_days,
    }))
}

/// Handler para DELETE /api/v1/collections/{name}
///
/// Remove uma coleção (Editor ou Admin, com verificação granular de permissão Delete).
pub async fn delete_collection(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    // Verificação de permissão granular (Delete)
    let perm_result = check_user_permission(&user, &name, &Action::Delete);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "delete_collection",
            &format!("collection:{name}"),
            serde_json::json!({"denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: delete collection '{name}'"
        )));
    }

    // Remove do mapa de coleções
    let collection_arc = app_state
        .collections
        .remove(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    // Remove estatísticas de queries
    app_state.query_stats.remove(&name);

    // Atualiza gauge de coleções ativas
    crate::metrics::COLLECTIONS_ACTIVE.set(app_state.collections.len() as f64);

    // Remove do disco
    let collection_dir = app_state
        .config
        .storage_path
        .join("collections")
        .join(&name);
    if collection_dir.exists() {
        api_err!(
            std::fs::remove_dir_all(&collection_dir),
            "failed to delete collection directory"
        )?;
    }

    // Drop da coleção (libera recursos)
    drop(collection_arc);

    // Audit trail
    {
        let entry = audit::audit_entry(
            &user.username,
            "delete_collection",
            &format!("collection:{name}"),
            serde_json::json!({}),
            AuditResult::Success,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Handler para PATCH /api/v1/collections/{name}
///
/// Atualiza apenas a retenção (retention_days) da coleção. Persiste config.json no disco.
pub async fn patch_collection_retention(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<PatchCollectionRetentionBody>,
) -> ApiResult<StatusCode> {
    let perm_result = check_user_permission(&user, &name, &Action::Admin);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "patch_collection_retention",
            &format!("collection:{name}"),
            serde_json::json!({"denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: update collection '{name}'"
        )));
    }

    let collection_arc = app_state
        .collections
        .get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    {
        let mut coll = api_err!(collection_arc.write(), "failed to acquire write lock")?;
        coll.set_retention_days(body.retention_days);
    }

    let collection_dir = app_state
        .config
        .storage_path
        .join("collections")
        .join(&name);
    {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        app_state
            .storage_circuit_breaker
            .call(|| {
                FileStorage::save_collection(
                    &collection,
                    &collection_dir,
                    app_state.config.binary_snapshot,
                    app_state.config.namespace_physical_isolation,
                )
            })
            .map_err(ApiError::from)?;
    }
    {
        let coll = api_err!(collection_arc.write(), "failed to acquire write lock")?;
        coll.mark_clean();
    }

    let entry = audit::audit_entry(
        &user.username,
        "patch_collection_retention",
        &format!("collection:{name}"),
        serde_json::json!({ "retention_days": body.retention_days }),
        AuditResult::Success,
        None,
        None,
    );
    app_state.audit_logger.log(&entry);

    Ok(StatusCode::NO_CONTENT)
}

/// Response for tier distribution endpoint.
#[derive(Debug, Serialize)]
pub struct TierDistributionResponse {
    pub hot: usize,
    pub warm: usize,
    pub cold: usize,
    pub hot_memory_bytes: usize,
    pub warm_memory_bytes: usize,
    pub cold_memory_bytes: usize,
}

/// Handler for GET /api/v1/collections/{name}/tiers
///
/// Returns the distribution of points across storage tiers.
pub async fn get_tier_distribution(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<TierDistributionResponse>> {
    let collection_arc = app_state
        .collections
        .get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    let config = collection.config();
    let num_points = collection.len();
    let dimension = config.dimension;

    // If tiered storage is not enabled, all points are in HOT tier
    if !config.tiered_storage.enabled {
        let vector_bytes = dimension * 4;
        let metadata_est = 200;
        let hnsw_node_est = 128;
        return Ok(Json(TierDistributionResponse {
            hot: num_points,
            warm: 0,
            cold: 0,
            hot_memory_bytes: num_points * (vector_bytes + metadata_est + hnsw_node_est),
            warm_memory_bytes: 0,
            cold_memory_bytes: 0,
        }));
    }

    // When tiered storage is enabled, report all as hot for now
    // (actual tier tracking is done in TieredCollection wrapper)
    let vector_bytes = dimension * 4;
    let metadata_est = 200;
    let hnsw_node_est = 128;
    Ok(Json(TierDistributionResponse {
        hot: num_points,
        warm: 0,
        cold: 0,
        hot_memory_bytes: num_points * (vector_bytes + metadata_est + hnsw_node_est),
        warm_memory_bytes: 0,
        cold_memory_bytes: 0,
    }))
}
