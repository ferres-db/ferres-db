// Types for FerresDB Dashboard

export interface Collection {
  name: string;
  dimension: number; // Backend usa "dimension" ao invés de "vector_size"
  num_points: number; // Backend usa "num_points" ao invés de "point_count"
  created_at: number; // Backend retorna como u64 (timestamp Unix)
  distance?: string; // Backend retorna "distance" (DistanceMetric)
  // Campos opcionais/compatibilidade
  distance_metric?: string; // Alias para distance
  vector_size?: number; // Alias para dimension (para compatibilidade)
  point_count?: number; // Alias para num_points (para compatibilidade)
}

export interface Point {
  id: string;
  vector: number[];
  metadata?: Record<string, unknown>;
  created_at?: number; // Timestamp Unix
}

export interface SearchResult {
  id: string;
  score: number;
  metadata?: Record<string, unknown>;
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
}

export interface ApiKeyInfo {
  id: number;
  name: string;
  key_prefix: string;
  created_at: number;
}

export interface CreateApiKeyResponse {
  id: number;
  name: string;
  key: string;
  key_prefix: string;
  created_at: number;
}

export interface UserInfo {
  id: number;
  username: string;
  role: string;
  created_at: number;
}
