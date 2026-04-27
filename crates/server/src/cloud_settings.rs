//! # Cloud Settings — S3 backup configuration stored in SQLite
//!
//! Allows the dashboard to configure region, bucket, and credentials for S3 backup.
//! Stored in SQLite so it can be changed at runtime without editing config/env.

use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CloudSettingsError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("lock poisoned")]
    LockPoisoned,
    #[error("migration error: {0}")]
    Migration(#[from] crate::db::migrations::MigrationError),
}

/// S3/cloud backup settings (stored in SQLite).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CloudSettings {
    pub region: Option<String>,
    pub bucket: Option<String>,
    /// Optional custom S3 endpoint (e.g. MinIO: http://localhost:9000).
    pub endpoint: Option<String>,
    pub access_key_id: Option<String>,
    /// Secret is never returned by GET; only accepted on PUT.
    #[serde(skip_serializing)]
    pub secret_access_key: Option<String>,
}

/// Store for cloud (S3) settings in SQLite. Single row table.
pub struct CloudSettingsStore {
    conn: Mutex<Connection>,
}

impl CloudSettingsStore {
    /// Opens or creates the DB at `path` and runs all pending migrations.
    pub fn new(path: &Path) -> Result<Self, CloudSettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        crate::db::migrations::run_migrations(
            &conn,
            crate::db::migrations::MIGRATIONS_CLOUD_SETTINGS,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Returns current cloud settings. Secret is never returned.
    pub fn get(&self) -> Result<CloudSettings, CloudSettingsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CloudSettingsError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT region, bucket, COALESCE(endpoint, ''), access_key_id FROM cloud_settings WHERE id = 1",
        )?;
        let row = stmt.query_row([], |r| {
            Ok(CloudSettings {
                region: r.get::<_, Option<String>>(0)?,
                bucket: r.get::<_, Option<String>>(1)?,
                endpoint: r
                    .get::<_, Option<String>>(2)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty()),
                access_key_id: r.get::<_, Option<String>>(3)?,
                secret_access_key: None,
            })
        });
        match row {
            Ok(s) => Ok(s),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CloudSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Updates cloud settings. Empty strings are stored as NULL. Secret is optional (only update if Some).
    pub fn set(&self, settings: &CloudSettings) -> Result<(), CloudSettingsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CloudSettingsError::LockPoisoned)?;
        let region = settings
            .region
            .as_deref()
            .and_then(|s| if s.is_empty() { None } else { Some(s) });
        let bucket = settings
            .bucket
            .as_deref()
            .and_then(|s| if s.is_empty() { None } else { Some(s) });
        let endpoint =
            settings
                .endpoint
                .as_deref()
                .and_then(|s| if s.is_empty() { None } else { Some(s) });
        let access_key_id =
            settings
                .access_key_id
                .as_deref()
                .and_then(|s| if s.is_empty() { None } else { Some(s) });
        let secret = settings.secret_access_key.as_deref().and_then(|s| {
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        });

        if let Some(secret) = secret {
            conn.execute(
                "UPDATE cloud_settings SET region = ?1, bucket = ?2, endpoint = ?3, access_key_id = ?4, secret_access_key = ?5 WHERE id = 1",
                rusqlite::params![region, bucket, endpoint, access_key_id, secret],
            )?;
        } else {
            conn.execute(
                "UPDATE cloud_settings SET region = ?1, bucket = ?2, endpoint = ?3, access_key_id = ?4 WHERE id = 1",
                rusqlite::params![region, bucket, endpoint, access_key_id],
            )?;
        }
        Ok(())
    }

    /// Returns settings including secret (for internal use by backup handler). Secret may be None if not set.
    pub fn get_with_secret(&self) -> Result<CloudSettings, CloudSettingsError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CloudSettingsError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT region, bucket, endpoint, access_key_id, secret_access_key FROM cloud_settings WHERE id = 1",
        )?;
        let row = stmt.query_row([], |r| {
            Ok(CloudSettings {
                region: r.get::<_, Option<String>>(0)?,
                bucket: r.get::<_, Option<String>>(1)?,
                endpoint: r
                    .get::<_, Option<String>>(2)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty()),
                access_key_id: r.get::<_, Option<String>>(3)?,
                secret_access_key: r.get::<_, Option<String>>(4)?,
            })
        });
        match row {
            Ok(s) => Ok(s),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CloudSettings::default()),
            Err(e) => Err(e.into()),
        }
    }
}
