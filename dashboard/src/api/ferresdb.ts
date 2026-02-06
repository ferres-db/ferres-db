import axios from 'axios';
import type { Collection, Point, SearchResult, GlobalStats, QueryEntry, CollectionStats, ApiKeyInfo, CreateApiKeyResponse, UserInfo } from '@/types';

// Runtime (Docker): window.__RUNTIME_CONFIG__ é preenchido pelo entrypoint.
// Build-time (Vite): import.meta.env. Fallback para dev com proxy.
declare global {
  interface Window {
    __RUNTIME_CONFIG__?: { apiBaseUrl?: string; apiKey?: string };
  }
}
const runtime = typeof window !== 'undefined' ? window.__RUNTIME_CONFIG__ : undefined;
const API_BASE_URL =
  runtime?.apiBaseUrl ||
  (import.meta.env.DEV
    ? (import.meta.env.VITE_API_BASE_URL ?? '')
    : (import.meta.env.VITE_API_BASE_URL || 'http://localhost:8080'));

/** API key (env ou runtime). Usada quando o usuário não está logado. */
function getApiKey(): string | null {
  const key = runtime?.apiKey || import.meta.env.VITE_API_KEY;
  return (typeof key === 'string' && key.trim()) ? key.trim() : null;
}

const TOKEN_KEY = 'ferresdb_token';
const ROLE_KEY = 'ferresdb_role';

export type Role = 'admin' | 'editor' | 'viewer';

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
  if (r === 'admin' || r === 'editor' || r === 'viewer') return r;
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
  headers: { 'Content-Type': 'application/json' },
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
    console.error('API Error:', error.response?.data || error.message);
    return Promise.reject(error);
  }
);

// Collections API
export const collectionsApi = {
  list: async (): Promise<Collection[]> => {
    const response = await apiClient.get('/api/v1/collections');
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
    // Mapeia para o formato esperado pelo frontend
    return {
      name: data.name,
      dimension: data.dimension,
      num_points: data.num_points,
      created_at: data.last_updated || data.created_at, // Backend retorna last_updated nos detalhes
      vector_size: data.dimension,
      point_count: data.num_points,
      distance: data.distance,
      distance_metric: data.distance,
    };
  },

  create: async (name: string, vectorSize: number, distanceMetric: string = 'cosine'): Promise<void> => {
    await apiClient.post('/api/v1/collections', {
      name,
      vector_size: vectorSize,
      distance_metric: distanceMetric,
    });
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
    }
  ): Promise<{ points: Point[]; total: number; limit: number; offset: number; has_more: boolean }> => {
    const params = new URLSearchParams();
    if (options?.limit) params.append('limit', options.limit.toString());
    if (options?.offset) params.append('offset', options.offset.toString());
    if (options?.filter) params.append('filter', JSON.stringify(options.filter));
    
    const queryString = params.toString();
    const url = `/api/v1/collections/${collection}/points${queryString ? `?${queryString}` : ''}`;
    const response = await apiClient.get(url);
    return response.data;
  },

  upsert: async (collection: string, points: Point[]): Promise<void> => {
    await apiClient.post(`/api/v1/collections/${collection}/points`, { points });
  },

  get: async (collection: string, id: string): Promise<Point> => {
    const response = await apiClient.get(`/api/v1/collections/${collection}/points/${id}`);
    return response.data;
  },

  delete: async (collection: string, ids: string[]): Promise<void> => {
    await apiClient.delete(`/api/v1/collections/${collection}/points`, {
      data: { ids },
    });
  },

  search: async (
    collection: string,
    vector: number[],
    limit: number = 10,
    filter?: Record<string, unknown>
  ): Promise<SearchResult[]> => {
    const response = await apiClient.post(`/api/v1/collections/${collection}/search`, {
      vector,
      limit,
      filter,
    });
    return response.data.results || [];
  },
};

// Stats API
export const statsApi = {
  global: async (): Promise<GlobalStats> => {
    const response = await apiClient.get('/api/v1/stats/global');
    return response.data;
  },

  queries: async (collection?: string): Promise<QueryEntry[]> => {
    const params = collection ? { collection } : {};
    const response = await apiClient.get('/api/v1/stats/queries', { params });
    return response.data;
  },

  slowQueries: async (): Promise<QueryEntry[]> => {
    const response = await apiClient.get('/api/v1/stats/slow-queries');
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
    const response = await apiClient.get('/health');
    return response.data;
  },
};

// Auth API (login — rota pública)
export const authApi = {
  login: async (
    username: string,
    password: string
  ): Promise<{ token: string; username: string; role: string }> => {
    const response = await apiClient.post<{
      token: string;
      username: string;
      role: string;
    }>('/api/v1/auth/login', { username, password });
    return response.data;
  },
};

// API Keys API (requires valid token or API key)
export const keysApi = {
  list: async (): Promise<ApiKeyInfo[]> => {
    const response = await apiClient.get<ApiKeyInfo[]>('/api/v1/keys');
    return Array.isArray(response.data) ? response.data : [];
  },

  create: async (name: string): Promise<CreateApiKeyResponse> => {
    const response = await apiClient.post<CreateApiKeyResponse>('/api/v1/keys', { name: name.trim() });
    return response.data;
  },

  delete: async (id: number): Promise<void> => {
    await apiClient.delete(`/api/v1/keys/${id}`);
  },
};

// Users API (dashboard users — list and create)
export const usersApi = {
  list: async (): Promise<UserInfo[]> => {
    const response = await apiClient.get<UserInfo[]>('/api/v1/users');
    return Array.isArray(response.data) ? response.data : [];
  },

  create: async (
    username: string,
    password: string,
    role?: string
  ): Promise<UserInfo> => {
    const response = await apiClient.post<UserInfo>('/api/v1/users', {
      username: username.trim(),
      password,
      ...(role && { role }),
    });
    return response.data;
  },

  delete: async (id: number): Promise<void> => {
    await apiClient.delete(`/api/v1/users/${id}`);
  },

  updatePassword: async (username: string, password: string): Promise<void> => {
    await apiClient.put(`/api/v1/users/${encodeURIComponent(username)}/password`, {
      password,
    });
  },
};
