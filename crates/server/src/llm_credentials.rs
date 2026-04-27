//! # LLM Credentials — provider API keys stored in SQLite
//!
//! Tabela `llm_credentials(provider TEXT PRIMARY KEY, api_key TEXT)`.
//! As API keys nunca são retornadas em GET — somente um booleano `configured`.
//! Endpoints administrativos atualizam/limpam por provider; o handler de proxy
//! consome a chave server-side para falar com o provedor real.

use crate::time::unix_now;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmCredentialsError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("lock poisoned")]
    LockPoisoned,
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("migration error: {0}")]
    Migration(#[from] crate::db::migrations::MigrationError),
}

/// Provedor LLM suportado pelo proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Openai,
    Anthropic,
    Gemini,
}

impl LlmProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            LlmProvider::Openai => "openai",
            LlmProvider::Anthropic => "anthropic",
            LlmProvider::Gemini => "gemini",
        }
    }

    pub fn parse(s: &str) -> Result<Self, LlmCredentialsError> {
        match s {
            "openai" => Ok(LlmProvider::Openai),
            "anthropic" => Ok(LlmProvider::Anthropic),
            "gemini" => Ok(LlmProvider::Gemini),
            other => Err(LlmCredentialsError::UnknownProvider(other.to_string())),
        }
    }

    pub fn env_var(self) -> &'static str {
        match self {
            LlmProvider::Openai => "FERRESDB_OPENAI_API_KEY",
            LlmProvider::Anthropic => "FERRESDB_ANTHROPIC_API_KEY",
            LlmProvider::Gemini => "FERRESDB_GEMINI_API_KEY",
        }
    }
}

/// Status de configuração de um provider, para o dashboard (sem expor a chave).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmProviderStatus {
    pub provider: String,
    /// True se há chave configurada (DB ou env). False caso contrário.
    pub configured: bool,
    /// Onde a chave está configurada: "db", "env" ou null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    /// Timestamp Unix em segundos quando a chave foi atualizada (apenas se em DB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

/// Armazena chaves de API LLM em SQLite (uma linha por provider).
pub struct LlmCredentialsStore {
    conn: Mutex<Connection>,
}

impl LlmCredentialsStore {
    pub fn new(path: &Path) -> Result<Self, LlmCredentialsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        crate::db::migrations::run_migrations(
            &conn,
            crate::db::migrations::MIGRATIONS_LLM_CREDENTIALS,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Retorna a chave de um provider a partir do DB (ou None se não configurada).
    pub fn get(&self, provider: LlmProvider) -> Result<Option<String>, LlmCredentialsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| LlmCredentialsError::LockPoisoned)?;
        let mut stmt = conn.prepare("SELECT api_key FROM llm_credentials WHERE provider = ?1")?;
        let row = stmt.query_row([provider.as_str()], |r| r.get::<_, String>(0));
        match row {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve a chave: primeiro tenta a env var, depois o DB. Retorna `(api_key, source)`.
    pub fn resolve(
        &self,
        provider: LlmProvider,
    ) -> Result<Option<(String, &'static str)>, LlmCredentialsError> {
        if let Ok(v) = std::env::var(provider.env_var()) {
            if !v.trim().is_empty() {
                return Ok(Some((v, "env")));
            }
        }
        if let Some(v) = self.get(provider)? {
            return Ok(Some((v, "db")));
        }
        Ok(None)
    }

    /// Insere ou atualiza (UPSERT) a chave de um provider.
    pub fn set(&self, provider: LlmProvider, api_key: &str) -> Result<(), LlmCredentialsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| LlmCredentialsError::LockPoisoned)?;
        let now = unix_now() as i64;
        conn.execute(
            "INSERT INTO llm_credentials (provider, api_key, updated_at) VALUES (?1, ?2, ?3)\n             ON CONFLICT(provider) DO UPDATE SET api_key = excluded.api_key, updated_at = excluded.updated_at",
            rusqlite::params![provider.as_str(), api_key, now],
        )?;
        Ok(())
    }

    /// Remove a chave de um provider.
    pub fn delete(&self, provider: LlmProvider) -> Result<bool, LlmCredentialsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| LlmCredentialsError::LockPoisoned)?;
        let n = conn.execute(
            "DELETE FROM llm_credentials WHERE provider = ?1",
            rusqlite::params![provider.as_str()],
        )?;
        Ok(n > 0)
    }

    /// Lista todos os providers (configurados em DB ou env), sem expor chaves.
    pub fn list_status(&self) -> Result<Vec<LlmProviderStatus>, LlmCredentialsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| LlmCredentialsError::LockPoisoned)?;
        let mut stmt = conn.prepare("SELECT provider, updated_at FROM llm_credentials")?;
        let mut db_entries = std::collections::HashMap::<String, i64>::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (p, ts) = row?;
            db_entries.insert(p, ts);
        }
        drop(stmt);
        drop(conn);

        let providers = [
            LlmProvider::Openai,
            LlmProvider::Anthropic,
            LlmProvider::Gemini,
        ];
        let mut out = Vec::with_capacity(providers.len());
        for p in providers {
            let env_set = std::env::var(p.env_var())
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let db_ts = db_entries.get(p.as_str()).copied();
            // Env tem precedência (resolve prioriza env)
            let (configured, source, updated_at) = if env_set {
                (true, Some("env"), None)
            } else if let Some(ts) = db_ts {
                (true, Some("db"), Some(ts))
            } else {
                (false, None, None)
            };
            out.push(LlmProviderStatus {
                provider: p.as_str().to_string(),
                configured,
                source,
                updated_at,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("llm_credentials.db");
        (dir, path)
    }

    #[test]
    fn provider_parse_roundtrip() {
        for p in [
            LlmProvider::Openai,
            LlmProvider::Anthropic,
            LlmProvider::Gemini,
        ] {
            assert_eq!(LlmProvider::parse(p.as_str()).unwrap(), p);
        }
        assert!(LlmProvider::parse("nope").is_err());
    }

    #[test]
    fn upsert_and_get_roundtrip() {
        let (_dir, path) = temp_db();
        let store = LlmCredentialsStore::new(&path).unwrap();

        assert!(store.get(LlmProvider::Openai).unwrap().is_none());

        store.set(LlmProvider::Openai, "sk-test-1").unwrap();
        assert_eq!(
            store.get(LlmProvider::Openai).unwrap().as_deref(),
            Some("sk-test-1")
        );

        // Update overwrites
        store.set(LlmProvider::Openai, "sk-test-2").unwrap();
        assert_eq!(
            store.get(LlmProvider::Openai).unwrap().as_deref(),
            Some("sk-test-2")
        );

        // Delete clears
        assert!(store.delete(LlmProvider::Openai).unwrap());
        assert!(store.get(LlmProvider::Openai).unwrap().is_none());
        assert!(!store.delete(LlmProvider::Openai).unwrap());
    }

    #[test]
    fn list_status_three_providers() {
        let (_dir, path) = temp_db();
        let store = LlmCredentialsStore::new(&path).unwrap();
        let status = store.list_status().unwrap();
        assert_eq!(status.len(), 3);
        assert!(status.iter().all(|s| !s.configured));
    }
}
