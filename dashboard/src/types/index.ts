// Types for FerresDB Dashboard

/** Backend pode retornar quantization como "None" ou { "Scalar": { dtype, always_ram?, quantile? } } */
export type CollectionQuantizationResponse =
  | "None"
  | { Scalar: { dtype: string; always_ram?: boolean; quantile?: number } };

export interface Collection {
  name: string;
  dimension: number;
  num_points: number;
  created_at: number;
  distance?: string;
  distance_metric?: string;
  vector_size?: number;
  point_count?: number;
  /** Quantização (GET collection). None ou Scalar (SQ8). */
  quantization?: CollectionQuantizationResponse;
  enable_bm25?: boolean;
  bm25_text_field?: string;
  tiered_storage?: TieredStorageConfig;
  /** Retenção em dias (WAL e dados antigos). Opcional. */
  retention_days?: number | null;
}

export interface Point {
  id: string;
  vector: number[];
  metadata?: Record<string, unknown>;
  created_at?: number; // Timestamp Unix
  /** Logical namespace (multitenancy). Optional on upsert and returned by API when set. */
  namespace?: string;
  /** TTL in seconds; point expires after this time (vacuum worker removes it). Optional on upsert. */
  ttl?: number;
  /** Named vectors (e.g. title_vector, content_vector). Optional; search can target one via vector_field. */
  vectors?: Record<string, number[]>;
  /** Related point IDs (graph edges). Returned by API when present. */
  relations?: string[];
}

/** Graph node from subgraph API (id = storage_id). */
export interface GraphNode {
  id: string;
  metadata?: Record<string, unknown>;
  namespace?: string;
  created_at?: number;
  relations?: string[];
  vector?: number[];
}

export interface GraphLink {
  source: string;
  target: string;
}

/** Alias for API "edges" response (same shape as GraphLink). */
export type GraphEdge = GraphLink;

export interface SubgraphResponse {
  nodes: GraphNode[];
  /** Edges (links) of the subgraph. */
  edges: GraphEdge[];
}

export interface SearchResult {
  id: string;
  score: number;
  metadata?: Record<string, unknown>;
  /** Set when the point was stored with a namespace (multitenancy). */
  namespace?: string;
}

/** Response from POST /api/v1/collections/{name}/search (includes optional rerank_ms). */
export interface SearchPointsResponse {
  results: SearchResult[];
  took_ms?: number;
  query_id?: string;
  /** Present when rerank=true was used and server applied re-ranking. */
  rerank_ms?: number;
}

export interface QueriesPerMinuteBucket {
  timestamp: number; // u64 no backend
  count: number; // u64 no backend
}

export interface GlobalStats {
  total_collections: number;
  total_points: number;
  total_queries_24h: number;
  avg_latency_ms: number;
  queries_per_minute: QueriesPerMinuteBucket[];
  simd_enabled: boolean;
  /** Replication role: "leader" or "replica" (experimental). */
  role?: string;
  /** Namespace physical isolation (multitenancy storage). */
  namespace_physical_isolation?: boolean;
  /** HNSW ef_search auto-tuning active (FerresEngine). */
  hnsw_auto_tune_enabled?: boolean;
  /** Dashboard label: e.g. "Optimized by FerresEngine". */
  index_optimization_label?: string;
}

/** Cluster node (Raft foundation). */
export interface ClusterNodeInfo {
  id: string;
  addr: string;
  role: string;
  replication_lag?: number;
}

/** Cluster status from GET /api/v1/cluster (nodes, leader, replication). */
export interface ClusterStatus {
  raft_enabled: boolean;
  leader_id?: string;
  nodes: ClusterNodeInfo[];
}

export interface QueryEntry {
  timestamp: string;
  collection: string;
  limit: number;
  filter?: unknown;
  took_ms: number;
  results_count: number;
  query_id: string;
}

// Mantido para compatibilidade, mas não é usado pela API real
export interface QueryStats {
  timestamp: string;
  count: number;
}

export interface SlowQuery {
  query_id: string;
  collection: string;
  latency_ms: number;
  timestamp: string;
}

export interface CollectionStats {
  num_points: number;
  num_queries: number;
  avg_latency_ms: number;
  p50_latency_ms: number;
  p95_latency_ms: number;
  p99_latency_ms: number;
  tombstone_count?: number;
  tombstone_memory_waste_bytes?: number;
  /** Current HNSW ef_search (may be auto-tuned). */
  ef_search_current?: number;
  /** HNSW auto-tune enabled for this instance. */
  hnsw_auto_tune_enabled?: boolean;
}

// Analytics (GET /api/v1/stats/analytics)
export interface AnalyticsTierDistribution {
  hot: number;
  warm: number;
  cold: number;
  hot_memory_bytes: number;
  warm_memory_bytes: number;
  cold_memory_bytes: number;
}

export interface LatencyPerMinuteBucket {
  timestamp: number;
  avg_ms: number;
  p50_ms: number;
}

export interface AnalyticsLatency {
  avg_ms: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  latency_per_minute: LatencyPerMinuteBucket[];
}

export interface AnalyticsTombstones {
  total_count: number;
  total_memory_waste_bytes: number;
  total_points: number;
}

export interface AnalyticsCircuitBreaker {
  state: "closed" | "open" | "half_open" | string;
  failure_count: number;
}

export interface ThroughputPerMinuteBucket {
  timestamp: number;
  points: number;
}

export interface RecentLatencyEntry {
  timestamp: number;
  took_ms: number;
}

export interface TimeSeries10m {
  avg_points_per_second: number;
  p95_latency_ms: number;
  throughput_per_minute: ThroughputPerMinuteBucket[];
  recent_latencies: RecentLatencyEntry[];
}

/** Top namespace por armazenamento (tenant que mais consome recursos). */
export interface TopNamespaceByStorage {
  namespace: string;
  point_count: number;
  storage_bytes_estimate: number;
}

export interface AnalyticsResponse {
  tier_distribution: AnalyticsTierDistribution;
  latency: AnalyticsLatency;
  tombstones: AnalyticsTombstones;
  circuit_breaker: AnalyticsCircuitBreaker;
  time_series_10m: TimeSeries10m;
  cache_hit_rate_pct: number | null;
  /** Top namespaces por armazenamento (identificar tenants que mais consomem). */
  top_namespaces_by_storage?: TopNamespaceByStorage[];
  /** Média do tempo de re-ranking (ms) nas queries recentes que usaram rerank. */
  rerank_overhead_ms_avg?: number | null;
}

export interface ApiKeyInfo {
  id: number;
  name: string;
  key_prefix: string;
  created_at: number;
  /** Namespaces permitidos (vazio/undefined = todos). */
  allowed_namespaces?: string[] | null;
}

export interface CreateApiKeyResponse {
  id: number;
  name: string;
  key: string;
  key_prefix: string;
  created_at: number;
}

// RBAC — Permissions (match backend permissions.rs)
export type ResourceType = "all_collections" | "collection";
export interface ResourceAllCollections {
  type: "all_collections";
}
export interface ResourceCollection {
  type: "collection";
  name: string;
}
export type Resource = ResourceAllCollections | ResourceCollection;

export type Action = "read" | "write" | "create" | "delete" | "admin";

export interface MetadataRestriction {
  field: string;
  allowed_values: unknown[];
}

export interface Permission {
  resource: Resource;
  actions: Action[];
  metadata_restriction?: MetadataRestriction;
}

export interface UserInfo {
  id: number;
  username: string;
  role: string;
  created_at: number;
  permissions?: Permission[] | null;
}

// Audit (match backend audit.rs)
export type AuditResultType = "success" | "denied" | "error";

export interface AuditEntry {
  timestamp: string; // RFC 3339
  user_id: string;
  action: string;
  resource: string;
  details: Record<string, unknown>;
  result: AuditResultType;
  ip_address?: string;
  duration_ms?: number;
}

export interface AuditQueryParams {
  user?: string;
  action?: string;
  resource?: string;
  from?: string;
  to?: string;
  limit?: number;
}

// ─── Quantization (SQ8) ───────────────────────────────────────────────

export type ScalarType = "int8";

export interface ScalarQuantizationConfig {
  type: "scalar";
  dtype: ScalarType;
  always_ram?: boolean;
  quantile?: number; // 0.0 – 1.0
}

export type QuantizationConfig = "none" | ScalarQuantizationConfig;

// ─── Tiered Storage ──────────────────────────────────────────────────

export interface TieredStorageConfig {
  enabled: boolean;
  hot_threshold_hours?: number; // default: 24
  warm_threshold_hours?: number; // default: 168
  compaction_interval_secs?: number; // default: 3600
}

export interface TierDistribution {
  hot: number;
  warm: number;
  cold: number;
  hot_memory_bytes: number;
  warm_memory_bytes: number;
  cold_memory_bytes: number;
}

// ─── Create Collection (extended) ─────────────────────────────────────

export interface CreateCollectionRequest {
  name: string;
  dimension: number;
  distance: string; // "Cosine" | "Euclidean" | "DotProduct"
  quantization?: QuantizationConfig;
  enable_bm25?: boolean;
  bm25_text_field?: string;
  tiered_storage?: TieredStorageConfig;
}

// ─── Hybrid Search ────────────────────────────────────────────────────

export interface HybridSearchRequest {
  query_vector: number[];
  query_text: string; // text query for BM25
  limit?: number;
  alpha?: number; // 0.0 (pure BM25) – 1.0 (pure vector). Used with fusion: "weighted"
  fusion?: "weighted" | "rrf"; // fusion strategy (default: "weighted")
  rrf_k?: number; // RRF constant k (default: 60). Only used with fusion: "rrf"
  filter?: Record<string, unknown>;
  /** Restrict results to this namespace (multitenancy). */
  namespace?: string;
}

export interface HybridSearchResult {
  results: SearchResult[];
}

// ─── Search Explain ───────────────────────────────────────────────────

export interface SearchExplainRequest {
  vector: number[];
  limit?: number;
  filter?: Record<string, unknown>;
  /** Restrict results to this namespace (multitenancy). */
  namespace?: string;
  /** Vector field to search against: "default" or named (e.g. "title_vector", "content_vector"). */
  vector_field?: string;
}

export interface ExplainedResult {
  id: string;
  score: number;
  distance_metric: string;
  raw_distance: number;
  score_breakdown: Record<string, number>;
  filter_evaluation?: {
    conditions: Array<{
      field: string;
      operator: string;
      expected: unknown;
      actual: unknown;
      passed: boolean;
    }>;
    passed: boolean;
  };
  rank_before_filter: number;
  rank_after_filter: number;
  metadata?: Record<string, unknown>;
}

/** Metadados do percurso da busca (HNSW): camadas percorridas e comparações de distância. */
export interface ExplainMeta {
  /** Número de candidatos visitados (comparações de distância realizadas). */
  candidates_visited: number;
  /** Número de camadas do grafo HNSW percorridas. */
  layers_traversed: number;
  /** Número de tombstones (pontos removidos) ignorados durante a busca. */
  tombstones_skipped: number;
}

export interface SearchExplainResponse {
  query_vector_norm: number;
  distance_metric: string;
  candidates_scanned: number;
  candidates_after_filter: number;
  results: ExplainedResult[];
  index_stats: {
    total_points: number;
    hnsw_layers: number;
    ef_search_used: number;
    tombstones_skipped: number;
  };
  /** Metadados do percurso da busca (camadas, comparações). Presente quando o índice retorna (ex.: HNSW). */
  explain_meta?: ExplainMeta;
}

// ─── Search Estimate ──────────────────────────────────────────────────

export interface SearchEstimateRequest {
  vector: number[];
  limit?: number;
  filter?: Record<string, unknown>;
  /** Restrict estimate to this namespace (multitenancy). */
  namespace?: string;
  /** Vector field to search against: "default" or named (e.g. "title_vector", "content_vector"). */
  vector_field?: string;
}

export interface SearchEstimateResponse {
  estimated_ms: number;
  confidence_range: [number, number];
  estimated_memory_bytes: number;
  estimated_nodes_visited: number;
  is_expensive: boolean;
  recommendations: string[];
  breakdown: {
    index_scan_cost: number;
    filter_cost: number;
    hydration_cost: number;
    network_overhead: number;
  };
  historical_latency?: {
    p50_ms: number;
    p95_ms: number;
    p99_ms: number;
    avg_ms: number;
    total_queries: number;
  };
}

// ─── WebSocket Messages ───────────────────────────────────────────────

export type WsClientMessageType = "upsert" | "subscribe" | "ping";

export interface WsUpsertMessage {
  type: "upsert";
  collection: string;
  points: Array<{
    id: string;
    vector: number[];
    metadata?: Record<string, unknown>;
    namespace?: string;
    ttl?: number;
  }>;
}

export interface WsSubscribeMessage {
  type: "subscribe";
  collection: string;
  events?: Array<"upsert" | "delete">;
}

export interface WsPingMessage {
  type: "ping";
}

export type WsClientMessage =
  | WsUpsertMessage
  | WsSubscribeMessage
  | WsPingMessage;

export interface WsAckMessage {
  type: "ack";
  upserted: number;
  failed: number;
  took_ms: number;
}

export interface WsEventMessage {
  type: "event";
  collection: string;
  action: "upsert" | "delete";
  point_ids: string[];
  timestamp: number;
}

export interface WsErrorMessage {
  type: "error";
  message: string;
  code: number;
}

export interface WsPongMessage {
  type: "pong";
}

export type WsServerMessage =
  | WsAckMessage
  | WsEventMessage
  | WsErrorMessage
  | WsPongMessage;

// ─── Embedding Types ──────────────────────────────────────────────────

export type EmbeddingProvider = "openai" | "gemini";

export interface EmbeddingModelInfo {
  id: string;
  name: string;
  dimensions: number;
  provider: EmbeddingProvider;
}

export const EMBEDDING_MODELS: EmbeddingModelInfo[] = [
  {
    id: "text-embedding-3-small",
    name: "text-embedding-3-small",
    dimensions: 1536,
    provider: "openai",
  },
  {
    id: "text-embedding-3-large",
    name: "text-embedding-3-large",
    dimensions: 3072,
    provider: "openai",
  },
  {
    id: "text-embedding-ada-002",
    name: "text-embedding-ada-002",
    dimensions: 1536,
    provider: "openai",
  },
  {
    id: "text-embedding-004",
    name: "text-embedding-004",
    dimensions: 768,
    provider: "gemini",
  },
];

export interface EmbeddingResult {
  vector: number[];
  dimensions: number;
  model: string;
  provider: EmbeddingProvider;
  took_ms: number;
}

// ─── Reindex ──────────────────────────────────────────────────────────

export type ReindexStatus =
  | "Queued"
  | "Building"
  | "Swapping"
  | "Completed"
  | "Failed";

export interface ReindexStats {
  points_processed: number;
  points_total: number;
  tombstones_cleaned: number;
  old_index_size_bytes: number;
  new_index_size_bytes: number;
}

export interface ReindexJob {
  id: string;
  collection: string;
  status: ReindexStatus;
  progress: number;
  started_at: number;
  completed_at?: number | null;
  error?: string | null;
  stats: ReindexStats;
}

export interface StartReindexResponse {
  job_id: string;
  collection: string;
  status: ReindexStatus;
  message: string;
}

// ─── WebSocket Log Entry (for UI) ────────────────────────────────────

export type WsDirection = "sent" | "received";

export interface WsLogEntry {
  id: string;
  timestamp: Date;
  direction: WsDirection;
  message: WsClientMessage | WsServerMessage;
  raw?: string;
}
