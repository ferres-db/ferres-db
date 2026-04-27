//! # API Keys — armazenamento e validação de múltiplas chaves em SQLite
//!
//! As chaves são armazenadas em hash (SHA-256). Ao criar uma chave, o valor bruto
//! é retornado uma única vez. A validação usa um conjunto em memória (sincronizado com o SQLite).
//! Chaves podem ter restrição por namespace (allowed_namespaces); quando definida, a chave
//! só acessa os namespaces listados.

use lazy_static::lazy_static;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use thiserror::Error;

use crate::time::unix_now;

lazy_static! {
    static ref KEY_HASHES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    /// In-memory cache of API key hash → meta. Avoids hitting SQLite on every
    /// request.  Populated on init/reload and updated on key create/delete.
    static ref KEY_META_CACHE: Mutex<std::collections::HashMap<String, Option<ApiKeyMeta>>> =
        Mutex::new(std::collections::HashMap::new());
}

/// Store global (definido em main) para o middleware de auth obter meta da chave (namespace allowance).
static GLOBAL_STORE: OnceLock<Arc<ApiKeyStore>> = OnceLock::new();

/// Define o store global. Chamado em main após criar o ApiKeyStore.
pub fn set_global_store(store: Option<Arc<ApiKeyStore>>) {
    if let Some(s) = store {
        let _ = GLOBAL_STORE.set(s);
    }
}

/// Retorna metadados da chave (allowed_namespaces) se a chave for válida.
/// Uses an in-memory cache to avoid hitting SQLite on every request.
pub fn get_meta_global(key: &str) -> Option<ApiKeyMeta> {
    let hash = hash_key(key);
    // Fast path: check in-memory cache
    if let Ok(cache) = KEY_META_CACHE.lock() {
        if let Some(meta_opt) = cache.get(&hash) {
            return meta_opt.clone();
        }
    }
    // Slow path: query SQLite and populate cache
    let meta = GLOBAL_STORE.get().and_then(|store| store.get_meta(key));
    if let Ok(mut cache) = KEY_META_CACHE.lock() {
        cache.insert(hash, meta.clone());
    }
    meta
}

const KEY_PREFIX: &str = "ferres_sk_";
const KEY_RANDOM_BYTES: usize = 24; // 24 bytes = 48 hex chars

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn generate_raw_key() -> String {
    let mut bytes = [0u8; KEY_RANDOM_BYTES];
    getrandom::getrandom(&mut bytes).unwrap_or_else(|e| unreachable!("OS PRNG unavailable: {e}"));
    let hex_part: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{KEY_PREFIX}{hex_part}")
}

#[derive(Debug, Error)]
pub enum ApiKeyError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("lock poisoned")]
    LockPoisoned,
    #[error("duplicate key name")]
    DuplicateName,
    #[error("migration error: {0}")]
    Migration(#[from] crate::db::migrations::MigrationError),
}

/// Metadados de uma API key (para validação de namespace no middleware).
#[derive(Debug, Clone)]
pub struct ApiKeyMeta {
    /// Quando None ou lista vazia = acesso a todos os namespaces. Quando Some(non-empty) = só esses.
    pub allowed_namespaces: Option<Vec<String>>,
}

/// Informação de uma API key (sem o valor bruto).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiKeyInfo {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub created_at: i64,
    /// Namespaces permitidos para esta chave. Null/empty = todos.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_namespaces: Option<Vec<String>>,
}

/// Armazena e valida API keys em SQLite, mantendo um cache em memória.
pub struct ApiKeyStore {
    conn: Mutex<Connection>,
}

impl ApiKeyStore {
    /// Opens or creates the database at `path` and runs all pending migrations.
    pub fn new(path: &Path) -> Result<Self, ApiKeyError> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).ok();
        let conn = Connection::open(path)?;
        crate::db::migrations::run_migrations(&conn, crate::db::migrations::MIGRATIONS_API_KEYS)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Inicializa o store: insere chaves de bootstrap (config/env) se fornecidas
    /// e carrega todos os hashes em memória.
    pub fn init(&self, bootstrap_keys: Option<&str>) -> Result<(), ApiKeyError> {
        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;

        if let Some(raw) = bootstrap_keys {
            for key in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                let key_hash = hash_key(key);
                let key_prefix = if key.starts_with(KEY_PREFIX) {
                    key.chars().take(KEY_PREFIX.len() + 8).collect::<String>()
                } else {
                    format!("{}...", &key.chars().take(8).collect::<String>())
                };
                let created_at = unix_now() as i64;
                let allowed_json = None::<Vec<String>>;
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO api_keys (name, key_hash, key_prefix, created_at, allowed_namespaces) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        format!("bootstrap-{}", key_hash.chars().take(8).collect::<String>()),
                        key_hash,
                        key_prefix,
                        created_at,
                        allowed_json.and_then(|v| serde_json::to_string(&v).ok()),
                    ],
                );
            }
        }

        drop(conn);

        self.reload_hashes()?;
        Ok(())
    }

    fn reload_hashes(&self) -> Result<(), ApiKeyError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidParameterName("lock poisoned".to_string()))?;
        let mut hashes = KEY_HASHES.lock().map_err(|_| ApiKeyError::LockPoisoned)?;
        hashes.clear();

        // Also reload meta cache so get_meta_global never hits SQLite on the hot path.
        let mut meta_cache = KEY_META_CACHE
            .lock()
            .map_err(|_| ApiKeyError::LockPoisoned)?;
        meta_cache.clear();

        let mut stmt = conn.prepare("SELECT key_hash, allowed_namespaces FROM api_keys")?;
        let iter = stmt.query_map([], |row| {
            let hash: String = row.get(0)?;
            let allowed_raw: Option<String> = row.get(1)?;
            Ok((hash, allowed_raw))
        })?;
        for pair in iter.flatten() {
            let (hash, allowed_raw) = pair;
            hashes.insert(hash.clone());

            let allowed_namespaces: Option<Vec<String>> = allowed_raw
                .as_ref()
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(s).ok())
                .filter(|v: &Vec<String>| !v.is_empty());
            meta_cache.insert(hash, Some(ApiKeyMeta { allowed_namespaces }));
        }
        Ok(())
    }

    /// Valida se a chave está registrada (usa cache em memória).
    pub fn validate(key: &str) -> bool {
        let hash = hash_key(key);
        KEY_HASHES
            .lock()
            .map(|set| set.contains(&hash))
            .unwrap_or(false)
    }

    /// Retorna metadados da chave (allowed_namespaces) se a chave for válida.
    pub fn get_meta(&self, key: &str) -> Option<ApiKeyMeta> {
        let hash = hash_key(key);
        let conn = self.conn.lock().ok()?;
        let json_opt: Option<String> = conn
            .query_row(
                "SELECT allowed_namespaces FROM api_keys WHERE key_hash = ?1",
                rusqlite::params![hash],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        let list: Option<Vec<String>> = json_opt
            .as_deref()
            .and_then(|s| {
                if s.is_empty() {
                    None
                } else {
                    serde_json::from_str(s).ok()
                }
            })
            .filter(|v: &Vec<String>| !v.is_empty());
        Some(ApiKeyMeta {
            allowed_namespaces: list,
        })
    }

    /// Cria uma nova API key com o nome dado. Retorna (valor bruto, id, key_prefix, created_at).
    pub fn create_key(
        &self,
        name: &str,
        allowed_namespaces: Option<Vec<String>>,
    ) -> Result<(String, i64, String, i64), ApiKeyError> {
        let raw_key = generate_raw_key();
        let key_hash = hash_key(&raw_key);
        let key_prefix: String = raw_key.chars().take(KEY_PREFIX.len() + 8).collect();
        let created_at = unix_now() as i64;

        let allowed_json = allowed_namespaces
            .as_ref()
            .filter(|v| !v.is_empty())
            .and_then(|v| serde_json::to_string(v).ok());

        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;

        conn.execute(
            "INSERT INTO api_keys (name, key_hash, key_prefix, created_at, allowed_namespaces) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![name, key_hash, key_prefix, created_at, allowed_json],
        )?;

        let id = conn.last_insert_rowid();
        drop(conn);

        KEY_HASHES
            .lock()
            .map_err(|_| ApiKeyError::LockPoisoned)?
            .insert(key_hash.clone());

        // Update meta cache
        if let Ok(mut cache) = KEY_META_CACHE.lock() {
            cache.insert(key_hash, Some(ApiKeyMeta { allowed_namespaces }));
        }

        Ok((raw_key, id, key_prefix, created_at))
    }

    /// Lista todas as chaves (sem o valor bruto).
    pub fn list_keys(&self) -> Result<Vec<ApiKeyInfo>, ApiKeyError> {
        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, key_prefix, created_at, allowed_namespaces FROM api_keys ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let allowed_raw: Option<String> = row.get(4)?;
            let allowed_namespaces = allowed_raw
                .as_ref()
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(s).ok())
                .filter(|v: &Vec<String>| !v.is_empty());
            Ok(ApiKeyInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                key_prefix: row.get(2)?,
                created_at: row.get(3)?,
                allowed_namespaces,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Remove uma chave pelo id e atualiza o cache.
    pub fn delete_key(&self, id: i64) -> Result<(), ApiKeyError> {
        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;

        let key_hash: String = conn.query_row(
            "SELECT key_hash FROM api_keys WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )?;

        conn.execute("DELETE FROM api_keys WHERE id = ?1", rusqlite::params![id])?;

        drop(conn);

        KEY_HASHES
            .lock()
            .map_err(|_| ApiKeyError::LockPoisoned)?
            .remove(&key_hash);

        // Remove from meta cache
        if let Ok(mut cache) = KEY_META_CACHE.lock() {
            cache.remove(&key_hash);
        }

        Ok(())
    }

    /// Atualiza os namespaces permitidos para uma chave (por id).
    pub fn update_key_namespaces(
        &self,
        id: i64,
        allowed_namespaces: Option<Vec<String>>,
    ) -> Result<(), ApiKeyError> {
        let allowed_json = allowed_namespaces
            .as_ref()
            .filter(|v| !v.is_empty())
            .and_then(|v| serde_json::to_string(v).ok());
        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;
        conn.execute(
            "UPDATE api_keys SET allowed_namespaces = ?1 WHERE id = ?2",
            rusqlite::params![allowed_json, id],
        )?;
        Ok(())
    }
}
