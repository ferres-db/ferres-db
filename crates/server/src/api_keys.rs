//! # API Keys — armazenamento e validação de múltiplas chaves em SQLite
//!
//! As chaves são armazenadas em hash (SHA-256). Ao criar uma chave, o valor bruto
//! é retornado uma única vez. A validação usa um conjunto em memória (sincronizado com o SQLite).

use rusqlite::Connection;
use sha2::{Sha256, Digest};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;
use lazy_static::lazy_static;

lazy_static! {
    static ref KEY_HASHES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
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
    getrandom::getrandom(&mut bytes).expect("getrandom");
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
}

/// Informação de uma API key (sem o valor bruto).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiKeyInfo {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub created_at: i64,
}

/// Armazena e valida API keys em SQLite, mantendo um cache em memória.
pub struct ApiKeyStore {
    conn: Mutex<Connection>,
}

impl ApiKeyStore {
    /// Abre ou cria o banco em `path` e cria a tabela se não existir.
    pub fn new(path: &Path) -> Result<Self, ApiKeyError> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).ok();
        let conn = Connection::open(path)?;
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS api_keys (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL UNIQUE,
                key_prefix TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )
            "#,
            [],
        )?;
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
                let created_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO api_keys (name, key_hash, key_prefix, created_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![format!("bootstrap-{}", key_hash.chars().take(8).collect::<String>()), key_hash, key_prefix, created_at],
                );
            }
        }

        drop(conn);

        self.reload_hashes()?;
        Ok(())
    }

    fn reload_hashes(&self) -> Result<(), ApiKeyError> {
        let conn = self.conn.lock().map_err(|_| {
            rusqlite::Error::InvalidParameterName("lock poisoned".to_string())
        })?;
        let mut hashes = KEY_HASHES.lock().map_err(|_| ApiKeyError::LockPoisoned)?;
        hashes.clear();
        let mut stmt = conn.prepare("SELECT key_hash FROM api_keys")?;
        let iter = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for h in iter.flatten() {
            hashes.insert(h);
        }
        Ok(())
    }

    /// Valida se a chave está registrada (usa cache em memória).
    pub fn validate(key: &str) -> bool {
        let hash = hash_key(key);
        KEY_HASHES.lock().map(|set| set.contains(&hash)).unwrap_or(false)
    }

    /// Cria uma nova API key com o nome dado. Retorna (valor bruto, id, key_prefix, created_at).
    pub fn create_key(&self, name: &str) -> Result<(String, i64, String, i64), ApiKeyError> {
        let raw_key = generate_raw_key();
        let key_hash = hash_key(&raw_key);
        let key_prefix: String = raw_key.chars().take(KEY_PREFIX.len() + 8).collect();
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;

        conn.execute(
            "INSERT INTO api_keys (name, key_hash, key_prefix, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![name, key_hash, key_prefix, created_at],
        )?;

        let id = conn.last_insert_rowid();
        drop(conn);

        KEY_HASHES.lock().map_err(|_| ApiKeyError::LockPoisoned)?.insert(key_hash);

        Ok((raw_key, id, key_prefix, created_at))
    }

    /// Lista todas as chaves (sem o valor bruto).
    pub fn list_keys(&self) -> Result<Vec<ApiKeyInfo>, ApiKeyError> {
        let conn = self.conn.lock().map_err(|_| ApiKeyError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, key_prefix, created_at FROM api_keys ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ApiKeyInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                key_prefix: row.get(2)?,
                created_at: row.get(3)?,
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

        KEY_HASHES.lock().map_err(|_| ApiKeyError::LockPoisoned)?.remove(&key_hash);

        Ok(())
    }
}
