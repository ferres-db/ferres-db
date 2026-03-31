//! # Query Log Analytics — leitura e agregação de queries.log com cache em memória
//!
//! Lê o arquivo queries.log (JSONL), faz parse das linhas e mantém cache por 1h
//! para suportar os endpoints de analytics.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;
use std::time::{Duration, Instant};

const CACHE_TTL_SECS: u64 = 3600; // 1h

/// Uma linha do log (formato atual: com query_id; ignora campos extras como vector_preview).
#[derive(Debug, Deserialize)]
struct LogLine {
    #[serde(default)]
    query_id: Option<String>,
    timestamp: String,
    collection: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    filter: Option<serde_json::Value>,
    #[serde(default)]
    results_count: Option<usize>,
    took_ms: u64,
    /// Vetor completo (opcional); presente quando o log foi escrito com suporte a warmup.
    #[serde(default)]
    vector: Option<Vec<f32>>,
}

/// Entrada parseada para uso nos endpoints (últimas 24h por padrão).
#[derive(Debug, Clone)]
pub struct ParsedQueryEntry {
    pub timestamp: String,
    pub timestamp_secs: u64,
    pub collection: String,
    pub limit: usize,
    pub filter: Option<serde_json::Value>,
    pub took_ms: u64,
    pub results_count: usize,
    pub query_id: String,
    /// Vetor completo (opcional); usado pelo warmup para replay.
    pub vector: Option<Vec<f32>>,
}

/// Cache em memória do log parseado com TTL de 1h.
pub struct QueryLogCache {
    log_path: std::path::PathBuf,
    cache: RwLock<Option<(Vec<ParsedQueryEntry>, Instant)>>,
    cache_ttl: Duration,
}

impl QueryLogCache {
    pub fn new(log_path: std::path::PathBuf) -> Self {
        Self {
            log_path,
            cache: RwLock::new(None),
            cache_ttl: Duration::from_secs(CACHE_TTL_SECS),
        }
    }

    /// Carrega todas as entradas do log (re-lê do disco se cache expirado).
    fn get_entries(&self) -> Vec<ParsedQueryEntry> {
        let mut guard = self.cache.write().unwrap();
        let now = Instant::now();
        let refresh = match guard.as_ref() {
            None => true,
            Some((_, at)) => now.duration_since(*at) >= self.cache_ttl,
        };
        if refresh {
            let entries = Self::load_log(&self.log_path);
            *guard = Some((entries.clone(), now));
            entries
        } else {
            guard.as_ref().unwrap().0.clone()
        }
    }

    /// Lê e parseia o arquivo queries.log (JSONL).
    fn load_log(path: &Path) -> Vec<ParsedQueryEntry> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let log: LogLine = match serde_json::from_str(line) {
                Ok(l) => l,
                Err(_) => continue,
            };
            let timestamp_secs = DateTime::parse_from_rfc3339(&log.timestamp)
                .map(|dt| dt.with_timezone(&Utc).timestamp() as u64)
                .unwrap_or_else(|_| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                });
            out.push(ParsedQueryEntry {
                timestamp: log.timestamp,
                timestamp_secs,
                collection: log.collection,
                limit: log.limit.unwrap_or(0),
                filter: log.filter,
                took_ms: log.took_ms,
                results_count: log.results_count.unwrap_or(0),
                query_id: log
                    .query_id
                    .unwrap_or_else(|| format!("legacy-{}", out.len())),
                vector: log.vector,
            });
        }
        out
    }

    /// Entradas das últimas 24 horas.
    pub fn entries_24h(&self) -> Vec<ParsedQueryEntry> {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cutoff = now_secs.saturating_sub(24 * 3600);
        self.get_entries()
            .into_iter()
            .filter(|e| e.timestamp_secs >= cutoff)
            .collect()
    }

    /// Entradas das últimas 10 minutos (para séries temporais de monitoramento).
    /// Usa o cache com TTL de 1h; para dados sempre frescos no analytics use `entries_10m_fresh`.
    pub fn entries_10m(&self) -> Vec<ParsedQueryEntry> {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cutoff = now_secs.saturating_sub(10 * 60);
        self.get_entries()
            .into_iter()
            .filter(|e| e.timestamp_secs >= cutoff)
            .collect()
    }

    /// Entradas das últimas 10 minutos lendo o arquivo diretamente (sem cache).
    /// Garante que o endpoint de analytics veja as queries recém-logadas mesmo antes do cache atualizar.
    pub fn entries_10m_fresh(&self) -> Vec<ParsedQueryEntry> {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cutoff = now_secs.saturating_sub(10 * 60);
        Self::load_log(&self.log_path)
            .into_iter()
            .filter(|e| e.timestamp_secs >= cutoff)
            .collect()
    }

    /// P95 da latência de busca (ms) nas últimas 10 minutos (usa cache).
    pub fn p95_latency_10m(&self) -> f64 {
        let entries = self.entries_10m();
        if entries.is_empty() {
            return 0.0;
        }
        let mut sorted_ms: Vec<u64> = entries.iter().map(|e| e.took_ms).collect();
        sorted_ms.sort();
        let len = sorted_ms.len();
        sorted_ms[(len * 95 / 100).min(len.saturating_sub(1))] as f64
    }

    /// P95 da latência (ms) nas últimas 10 min com leitura fresca do arquivo (para analytics).
    pub fn p95_latency_10m_fresh(&self) -> f64 {
        let entries = self.entries_10m_fresh();
        if entries.is_empty() {
            return 0.0;
        }
        let mut sorted_ms: Vec<u64> = entries.iter().map(|e| e.took_ms).collect();
        sorted_ms.sort();
        let len = sorted_ms.len();
        sorted_ms[(len * 95 / 100).min(len.saturating_sub(1))] as f64
    }

    /// Total de queries nas últimas 24h.
    pub fn total_queries_24h(&self) -> u64 {
        self.entries_24h().len() as u64
    }

    /// Latência média (ms) nas últimas 24h.
    pub fn avg_latency_24h(&self) -> f64 {
        let entries = self.entries_24h();
        if entries.is_empty() {
            return 0.0;
        }
        let sum: u64 = entries.iter().map(|e| e.took_ms).sum();
        sum as f64 / entries.len() as f64
    }

    /// Queries por minuto (últimas 24h): (minute_ts, count).
    pub fn queries_per_minute_24h(&self) -> Vec<(u64, u64)> {
        let entries = self.entries_24h();
        let mut buckets: HashMap<u64, u64> = HashMap::new();
        for e in &entries {
            let minute = e.timestamp_secs / 60;
            *buckets.entry(minute).or_insert(0) += 1;
        }
        let mut out: Vec<(u64, u64)> = buckets.into_iter().collect();
        out.sort_by_key(|&(k, _)| k);
        out
    }

    /// Latência por minuto (últimas 24h): (minute_ts, avg_ms, p50_ms).
    /// Agrupa entries por minuto e calcula média e mediana de took_ms por bucket.
    pub fn latency_per_minute_24h(&self) -> Vec<(u64, f64, f64)> {
        let entries = self.entries_24h();
        let mut buckets: HashMap<u64, Vec<u64>> = HashMap::new();
        for e in &entries {
            let minute = e.timestamp_secs / 60;
            buckets.entry(minute).or_default().push(e.took_ms);
        }
        let mut out: Vec<(u64, f64, f64)> = buckets
            .into_iter()
            .map(|(minute_ts, mut latencies)| {
                latencies.sort();
                let len = latencies.len();
                let avg_ms = if len == 0 {
                    0.0
                } else {
                    latencies.iter().sum::<u64>() as f64 / len as f64
                };
                let p50_ms = if len == 0 {
                    0.0
                } else {
                    latencies[len * 50 / 100] as f64
                };
                (minute_ts, avg_ms, p50_ms)
            })
            .collect();
        out.sort_by_key(|&(k, _, _)| k);
        out
    }

    /// Lista de queries com filtro opcional por coleção, limit e ordenação (latency = mais lentas primeiro).
    pub fn get_queries(
        &self,
        collection_filter: Option<&str>,
        limit: usize,
        sort_by_latency: bool,
    ) -> Vec<ParsedQueryEntry> {
        let mut entries = self.entries_24h();
        if let Some(c) = collection_filter {
            entries.retain(|e| e.collection == c);
        }
        if sort_by_latency {
            entries.sort_by(|a, b| b.took_ms.cmp(&a.took_ms));
        } else {
            entries.sort_by(|a, b| b.timestamp_secs.cmp(&a.timestamp_secs));
        }
        entries.into_iter().take(limit).collect()
    }

    /// Queries com latência acima do threshold (mais lentas primeiro), limitadas.
    pub fn get_slow_queries(&self, threshold_ms: u64, limit: usize) -> Vec<ParsedQueryEntry> {
        let mut entries = self.entries_24h();
        entries.retain(|e| e.took_ms >= threshold_ms);
        entries.sort_by(|a, b| b.took_ms.cmp(&a.took_ms));
        entries.into_iter().take(limit).collect()
    }

    /// Últimas N entradas do log que possuem vetor (para warmup no startup).
    /// Lê o arquivo diretamente (sem cache). Retorna no ordem cronológica (mais antigas primeiro).
    pub fn last_n_entries_for_warmup(&self, n: usize) -> Vec<ParsedQueryEntry> {
        let all = Self::load_log(&self.log_path);
        let last_n: Vec<ParsedQueryEntry> = all.into_iter().rev().take(n).rev().collect();
        last_n
            .into_iter()
            .filter(|e| e.vector.as_ref().map_or(false, |v| !v.is_empty()))
            .collect()
    }
}

/// Calcula a média de pontos inseridos por segundo nos últimos 10 minutos.
/// `entries` deve ser uma lista de (timestamp_unix_secs, points_count) já filtrada para a janela de 10 min.
/// Retorna total_points / 600.0 (média por segundo na janela de 600 segundos).
pub fn avg_points_per_second_10m(entries: &[(u64, u64)]) -> f64 {
    const WINDOW_SECS: u64 = 10 * 60;
    if entries.is_empty() {
        return 0.0;
    }
    let total: u64 = entries.iter().map(|(_, count)| count).sum();
    total as f64 / WINDOW_SECS as f64
}
