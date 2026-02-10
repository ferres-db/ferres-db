import axios from "axios";
import type {
  Collection,
  Point,
  SearchResult,
  GlobalStats,
  QueryEntry,
  CollectionStats,
  AnalyticsResponse,
  ApiKeyInfo,
  CreateApiKeyResponse,
  UserInfo,
  Permission,
  AuditEntry,
  AuditQueryParams,
  QuantizationConfig,
  TieredStorageConfig,
  TierDistribution,
  HybridSearchRequest,
  SearchExplainRequest,
  SearchExplainResponse,
  SearchEstimateRequest,
  SearchEstimateResponse,
  ReindexJob,
  StartReindexResponse,
} from "@/types";

// Runtime (Docker): window.__RUNTIME_CONFIG__ é preenchido pelo entrypoint.
// Build-time (Vite): import.meta.env. Fallback para dev com proxy.
declare global {
  interface Window {
    __RUNTIME_CONFIG__?: { apiBaseUrl?: string; apiKey?: string };
  }
}
const runtime =
  typeof window !== "undefined" ? window.__RUNTIME_CONFIG__ : undefined;
const API_BASE_URL =
  runtime?.apiBaseUrl ||
  (import.meta.env.DEV
    ? (import.meta.env.VITE_API_BASE_URL ?? "")
    : import.meta.env.VITE_API_BASE_URL || "http://localhost:8080");

/** API key (env ou runtime). Usada quando o usuário não está logado. */
function getApiKey(): string | null {
  const key = runtime?.apiKey || import.meta.env.VITE_API_KEY;
  return typeof key === "string" && key.trim() ? key.trim() : null;
}

const TOKEN_KEY = "ferresdb_token";
const ROLE_KEY = "ferresdb_role";

export type Role = "admin" | "editor" | "viewer";

/** Token do login do dashboard (localStorage). */
export function getStoredToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function setStoredToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token);
}

/** Role do usuário logado (admin, editor, viewer). */
export function getStoredRole(): Role | null {
  const r = localStorage.getItem(ROLE_KEY);
  if (r === "admin" || r === "editor" || r === "viewer") return r;
  return null;
}

export function setStoredRole(role: string): void {
  localStorage.setItem(ROLE_KEY, role);
}

export function clearStoredToken(): void {
  localStorage.removeItem(TOKEN_KEY);
  localStorage.removeItem(ROLE_KEY);
}

const apiClient = axios.create({
  baseURL: API_BASE_URL,
  headers: { "Content-Type": "application/json" },
});

// Interceptor: Bearer = token de login (se houver) ou API key (VITE_API_KEY / runtime)
apiClient.interceptors.request.use((config) => {
  const token = getStoredToken();
  const apiKey = getApiKey();
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  } else if (apiKey) {
    config.headers.Authorization = `Bearer ${apiKey}`;
  }
  return config;
});

// Interceptor para tratar erros
apiClient.interceptors.response.use(
  (response) => response,
  (error) => {
    if (error.response?.status === 401) {
      clearStoredToken();
    }
    console.error("API Error:", error.response?.data || error.message);
    return Promise.reject(error);
  },
);

// Collections API
export const collectionsApi = {
  list: async (options?: { namespace?: string }): Promise<Collection[]> => {
    const params = new URLSearchParams();
    if (options?.namespace != null && options.namespace !== "")
      params.append("namespace", options.namespace);
    const queryString = params.toString();
    const url = `/api/v1/collections${queryString ? `?${queryString}` : ""}`;
    const response = await apiClient.get(url);
    // A API retorna { collections: [...] }
    const collections = response.data.collections || [];
    // Mapeia para o formato esperado pelo frontend
    return collections.map((c: any) => ({
      ...c,
      vector_size: c.dimension,
      point_count: c.num_points,
      distance_metric: c.distance || c.distance_metric, // Backend retorna "distance"
    }));
  },

  get: async (name: string): Promise<Collection> => {
    const response = await apiClient.get(`/api/v1/collections/${name}`);
    const data = response.data;
    return {
      name: data.name,
      dimension: data.dimension,
      num_points: data.num_points,
      created_at: data.last_updated ?? data.created_at,
      vector_size: data.dimension,
      point_count: data.num_points,
      distance: data.distance,
      distance_metric: data.distance,
      quantization: data.quantization,
      enable_bm25: data.enable_bm25,
      bm25_text_field: data.bm25_text_field,
      tiered_storage: data.tiered_storage,
    };
  },

  create: async (
    name: string,
    vectorSize: number,
    distanceMetric: string = "cosine",
    options?: {
      quantization?: QuantizationConfig;
      enable_bm25?: boolean;
      bm25_text_field?: string;
      tiered_storage?: TieredStorageConfig;
    },
  ): Promise<void> => {
    // Backend expects "dimension" and "distance" (PascalCase: Cosine, Euclidean, DotProduct)
    const distance =
      distanceMetric === "cosine"
        ? "Cosine"
        : distanceMetric === "euclidean"
          ? "Euclidean"
          : distanceMetric === "dot"
            ? "DotProduct"
            : "Cosine";

    const body: Record<string, unknown> = {
      name,
      dimension: vectorSize,
      distance,
    };

    if (options?.quantization && options.quantization !== "none") {
      body.quantization = {
        Scalar: {
          dtype: "Int8",
          always_ram: (options.quantization as any).always_ram ?? false,
          quantile: (options.quantization as any).quantile ?? 0.99,
        },
      };
    }
    if (options?.enable_bm25) {
      body.enable_bm25 = true;
      if (options.bm25_text_field) {
        body.bm25_text_field = options.bm25_text_field;
      }
    }
    if (options?.tiered_storage && options.tiered_storage.enabled) {
      body.tiered_storage = options.tiered_storage;
    }

    await apiClient.post("/api/v1/collections", body);
  },

  getTierDistribution: async (name: string): Promise<TierDistribution> => {
    const response = await apiClient.get(`/api/v1/collections/${name}/tiers`);
    return response.data;
  },

  delete: async (name: string): Promise<void> => {
    await apiClient.delete(`/api/v1/collections/${name}`);
  },
};

// Points API
export const pointsApi = {
  list: async (
    collection: string,
    options?: {
      limit?: number;
      offset?: number;
      filter?: Record<string, unknown>;
      /** When set, restricts list to this namespace via filter { $namespace: value }. */
      namespace?: string;
    },
  ): Promise<{
    points: Point[];
    total: number;
    limit: number;
    offset: number;
    has_more: boolean;
  }> => {
    const params = new URLSearchParams();
    if (options?.limit) params.append("limit", options.limit.toString());
    if (options?.offset) params.append("offset", options.offset.toString());
    let filter = options?.filter;
    if (options?.namespace != null && options.namespace !== "") {
      filter = { ...filter, $namespace: options.namespace };
    }
    if (filter != null && Object.keys(filter).length > 0)
      params.append("filter", JSON.stringify(filter));

    const queryString = params.toString();
    const url = `/api/v1/collections/${collection}/points${queryString ? `?${queryString}` : ""}`;
    const response = await apiClient.get(url);
    return response.data;
  },

  upsert: async (collection: string, points: Point[]): Promise<void> => {
    await apiClient.post(`/api/v1/collections/${collection}/points`, {
      points,
    });
  },

  get: async (
    collection: string,
    id: string,
    options?: { namespace?: string },
  ): Promise<Point> => {
    const params = new URLSearchParams();
    if (options?.namespace != null && options.namespace !== "")
      params.append("namespace", options.namespace);
    const queryString = params.toString();
    const url = `/api/v1/collections/${collection}/points/${encodeURIComponent(id)}${queryString ? `?${queryString}` : ""}`;
    const response = await apiClient.get(url);
    return response.data;
  },

  delete: async (
    collection: string,
    ids: string[],
    options?: { namespace?: string },
  ): Promise<void> => {
    const body: { ids: string[]; namespace?: string } = { ids };
    if (options?.namespace != null && options.namespace !== "")
      body.namespace = options.namespace;
    await apiClient.delete(`/api/v1/collections/${collection}/points`, {
      data: body,
    });
  },

  search: async (
    collection: string,
    vector: number[],
    limit: number = 10,
    filter?: Record<string, unknown>,
    options?: { namespace?: string; vector_field?: string },
  ): Promise<SearchResult[]> => {
    const body: Record<string, unknown> = { vector, limit, filter };
    if (options?.namespace != null && options.namespace !== "")
      body.namespace = options.namespace;
    if (options?.vector_field != null && options.vector_field !== "")
      body.vector_field = options.vector_field;
    const response = await apiClient.post(
      `/api/v1/collections/${collection}/search`,
      body,
    );
    return response.data.results || [];
  },

  hybridSearch: async (
    collection: string,
    params: HybridSearchRequest,
  ): Promise<SearchResult[]> => {
    const response = await apiClient.post(
      `/api/v1/collections/${collection}/search/hybrid`,
      params,
    );
    return response.data.results || [];
  },

  explain: async (
    collection: string,
    params: SearchExplainRequest,
  ): Promise<SearchExplainResponse> => {
    const response = await apiClient.post(
      `/api/v1/collections/${collection}/search/explain`,
      params,
    );
    return response.data;
  },

  estimate: async (
    collection: string,
    params: SearchEstimateRequest,
  ): Promise<SearchEstimateResponse> => {
    const response = await apiClient.post(
      `/api/v1/collections/${collection}/search/estimate`,
      params,
    );
    return response.data;
  },
};

// Stats API
export const statsApi = {
  global: async (): Promise<GlobalStats> => {
    const response = await apiClient.get("/api/v1/stats/global");
    return response.data;
  },

  analytics: async (): Promise<AnalyticsResponse> => {
    const response = await apiClient.get("/api/v1/stats/analytics");
    return response.data;
  },

  queries: async (collection?: string): Promise<QueryEntry[]> => {
    const params = collection ? { collection } : {};
    const response = await apiClient.get("/api/v1/stats/queries", { params });
    return response.data;
  },

  slowQueries: async (): Promise<QueryEntry[]> => {
    const response = await apiClient.get("/api/v1/stats/slow-queries");
    return response.data;
  },

  collection: async (name: string): Promise<CollectionStats> => {
    const response = await apiClient.get(`/api/v1/collections/${name}/stats`);
    return response.data;
  },
};

// Health API
export const healthApi = {
  check: async (): Promise<{ status: string }> => {
    const response = await apiClient.get("/health");
    return response.data;
  },
};

// Auth API (login — rota pública)
export const authApi = {
  login: async (
    username: string,
    password: string,
  ): Promise<{ token: string; username: string; role: string }> => {
    const response = await apiClient.post<{
      token: string;
      username: string;
      role: string;
    }>("/api/v1/auth/login", { username, password });
    return response.data;
  },
};

// API Keys API (requires valid token or API key)
export const keysApi = {
  list: async (): Promise<ApiKeyInfo[]> => {
    const response = await apiClient.get<ApiKeyInfo[]>("/api/v1/keys");
    return Array.isArray(response.data) ? response.data : [];
  },

  create: async (name: string): Promise<CreateApiKeyResponse> => {
    const response = await apiClient.post<CreateApiKeyResponse>(
      "/api/v1/keys",
      { name: name.trim() },
    );
    return response.data;
  },

  delete: async (id: number): Promise<void> => {
    await apiClient.delete(`/api/v1/keys/${id}`);
  },
};

// Users API (dashboard users — list, create, permissions)
export const usersApi = {
  list: async (): Promise<UserInfo[]> => {
    const response = await apiClient.get<UserInfo[]>("/api/v1/users");
    return Array.isArray(response.data) ? response.data : [];
  },

  create: async (
    username: string,
    password: string,
    role?: string,
    permissions?: Permission[] | null,
  ): Promise<UserInfo> => {
    const response = await apiClient.post<UserInfo>("/api/v1/users", {
      username: username.trim(),
      password,
      ...(role && { role }),
      ...(permissions !== undefined && { permissions: permissions ?? null }),
    });
    return response.data;
  },

  delete: async (id: number): Promise<void> => {
    await apiClient.delete(`/api/v1/users/${id}`);
  },

  updatePassword: async (username: string, password: string): Promise<void> => {
    await apiClient.put(
      `/api/v1/users/${encodeURIComponent(username)}/password`,
      {
        password,
      },
    );
  },

  updatePermissions: async (
    username: string,
    permissions: Permission[] | null,
  ): Promise<{ updated: boolean; username: string }> => {
    const response = await apiClient.put(
      `/api/v1/users/${encodeURIComponent(username)}/permissions`,
      {
        permissions,
      },
    );
    return response.data;
  },
};

// Reindex API
export const reindexApi = {
  start: async (collection: string): Promise<StartReindexResponse> => {
    const response = await apiClient.post<StartReindexResponse>(
      `/api/v1/collections/${encodeURIComponent(collection)}/reindex`,
    );
    return response.data;
  },

  getJob: async (collection: string, jobId: string): Promise<ReindexJob> => {
    const response = await apiClient.get<ReindexJob>(
      `/api/v1/collections/${encodeURIComponent(collection)}/reindex/${encodeURIComponent(jobId)}`,
    );
    return response.data;
  },

  listJobs: async (collection: string): Promise<ReindexJob[]> => {
    const response = await apiClient.get<{ jobs: ReindexJob[] }>(
      `/api/v1/collections/${encodeURIComponent(collection)}/reindex`,
    );
    return response.data.jobs;
  },
};

// Audit API (Admin only)
export const auditApi = {
  list: async (params?: AuditQueryParams): Promise<AuditEntry[]> => {
    const response = await apiClient.get<AuditEntry[]>("/api/v1/audit", {
      params: params ?? {},
    });
    return Array.isArray(response.data) ? response.data : [];
  },
};

// Cloud settings (Admin only — S3 backup configuration stored in SQLite)
export interface CloudSettings {
  region?: string | null;
  bucket?: string | null;
  endpoint?: string | null;
  access_key_id?: string | null;
}

export const settingsApi = {
  getCloud: async (): Promise<CloudSettings> => {
    const response = await apiClient.get<CloudSettings>("/api/v1/admin/settings/cloud");
    return response.data;
  },

  putCloud: async (body: {
    region?: string | null;
    bucket?: string | null;
    endpoint?: string | null;
    access_key_id?: string | null;
    secret_access_key?: string | null;
  }): Promise<{ ok: boolean }> => {
    const response = await apiClient.put<{ ok: boolean }>("/api/v1/admin/settings/cloud", body);
    return response.data;
  },

  testS3: async (): Promise<{ ok: boolean; message?: string }> => {
    const response = await apiClient.post<{ ok: boolean; message?: string }>(
      "/api/v1/admin/settings/test-s3"
    );
    return response.data;
  },
};

// Backup API (Admin only — export snapshot to S3)
export const backupApi = {
  exportToCloud: async (): Promise<{
    ok: boolean;
    key: string;
    bucket: string;
    size_bytes: number;
    region?: string;
  }> => {
    const response = await apiClient.post<{
      ok: boolean;
      key: string;
      bucket: string;
      size_bytes: number;
      region?: string;
    }>("/api/v1/admin/backup");
    return response.data;
  },
};

// WebSocket URL helper
export function getWsUrl(token?: string): string {
  const base = API_BASE_URL || window.location.origin;
  const wsProtocol = base.startsWith("https") ? "wss" : "ws";
  const httpStripped = base.replace(/^https?:\/\//, "");
  const authToken = token || getStoredToken() || getApiKey() || "";
  return `${wsProtocol}://${httpStripped}/api/v1/ws?token=${encodeURIComponent(authToken)}`;
}
