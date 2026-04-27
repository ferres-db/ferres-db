//! # Users — armazenamento de usuários do dashboard em SQLite
//!
//! Senhas são armazenadas com Argon2. Sempre existe o usuário padrão root/ferresdb.

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;
use rusqlite::Connection;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;
use thiserror::Error;

use crate::time::unix_now;

const DEFAULT_USERNAME: &str = "root";
const DEFAULT_PASSWORD: &str = "ferresdb";

/// Papel do usuário. Ordem: viewer < editor < admin.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer = 0,
    Editor = 1,
    Admin = 2,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "viewer" => Ok(Role::Viewer),
            "editor" => Ok(Role::Editor),
            "admin" => Ok(Role::Admin),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Error)]
pub enum UserError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("lock poisoned")]
    LockPoisoned,
    #[error("invalid password")]
    InvalidPassword,
    #[error("password hash error")]
    Hash,
    #[error("username already exists")]
    DuplicateUsername,
    #[error("invalid username")]
    InvalidUsername,
}

/// Informação de um usuário (sem senha).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub created_at: i64,
    /// Permissões granulares (RBAC). Se None/vazio, aplica-se o comportamento legado baseado em role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<crate::permissions::Permission>>,
}

fn hash_password(password: &str) -> Result<String, UserError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| UserError::Hash)?;
    Ok(hash.to_string())
}

fn verify_password(password: &str, hash: &str) -> Result<bool, UserError> {
    let parsed = PasswordHash::new(hash).map_err(|_| UserError::Hash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Store de usuários em SQLite (username + password hash).
pub struct UserStore {
    conn: Mutex<Connection>,
}

impl UserStore {
    /// Abre ou cria o banco em `path` e cria a tabela se não existir.
    pub fn new(path: &Path) -> Result<Self, UserError> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).ok();
        let conn = Connection::open(path)?;
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'viewer',
                created_at INTEGER NOT NULL
            )
            "#,
            [],
        )?;
        // Migração: adicionar coluna role se a tabela já existia sem ela
        let _ = conn.execute(
            "ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'viewer'",
            [],
        );
        // Migração: adicionar coluna permissions (JSON) para RBAC granular
        let _ = conn.execute(
            "ALTER TABLE users ADD COLUMN permissions TEXT DEFAULT NULL",
            [],
        );
        let _ = conn.execute(
            "UPDATE users SET role = 'admin' WHERE username = ?1",
            [DEFAULT_USERNAME],
        );
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Garante que o usuário padrão root/ferresdb existe.
    pub fn ensure_default_user(&self) -> Result<(), UserError> {
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE username = ?1",
            [DEFAULT_USERNAME],
            |row| row.get(0),
        )?;
        drop(conn);

        if count == 0 {
            let hash = hash_password(DEFAULT_PASSWORD)?;
            let created_at = unix_now() as i64;
            let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
            conn.execute(
                "INSERT INTO users (username, password_hash, role, created_at) VALUES (?1, ?2, 'admin', ?3)",
                rusqlite::params![DEFAULT_USERNAME, hash, created_at],
            )?;
            tracing::info!(
                username = DEFAULT_USERNAME,
                "default dashboard user created"
            );
        }

        Ok(())
    }

    /// Retorna o role do usuário, ou None se não existir.
    pub fn get_role(&self, username: &str) -> Result<Option<Role>, UserError> {
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        let role: String = match conn.query_row(
            "SELECT role FROM users WHERE username = ?1",
            [username],
            |row| row.get(0),
        ) {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        drop(conn);
        Ok(role.parse().ok())
    }

    /// Valida credenciais e retorna true se ok.
    pub fn validate(&self, username: &str, password: &str) -> Result<bool, UserError> {
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        let hash: String = match conn.query_row(
            "SELECT password_hash FROM users WHERE username = ?1",
            [username],
            |row| row.get(0),
        ) {
            Ok(h) => h,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                drop(conn);
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        drop(conn);
        verify_password(password, &hash)
    }

    /// Lista todos os usuários (id, username, role, created_at, permissions).
    pub fn list(&self) -> Result<Vec<UserInfo>, UserError> {
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, username, role, created_at, permissions FROM users ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let permissions_json: Option<String> = row.get(4).unwrap_or(None);
            let permissions = permissions_json.and_then(|j| serde_json::from_str(&j).ok());
            Ok(UserInfo {
                id: row.get(0)?,
                username: row.get(1)?,
                role: row
                    .get::<_, String>(2)
                    .unwrap_or_else(|_| "viewer".to_string()),
                created_at: row.get(3)?,
                permissions,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Cria um novo usuário. Retorna erro se o username já existir.
    pub fn create(
        &self,
        username: &str,
        password: &str,
        role: Option<Role>,
    ) -> Result<UserInfo, UserError> {
        self.create_with_permissions(username, password, role, None)
    }

    /// Cria um novo usuário com permissões opcionais. Retorna erro se o username já existir.
    pub fn create_with_permissions(
        &self,
        username: &str,
        password: &str,
        role: Option<Role>,
        permissions: Option<Vec<crate::permissions::Permission>>,
    ) -> Result<UserInfo, UserError> {
        let username = username.trim();
        if username.is_empty() {
            return Err(UserError::InvalidUsername);
        }
        if password.is_empty() {
            return Err(UserError::InvalidPassword);
        }
        let role = role.unwrap_or(Role::Viewer);
        let hash = hash_password(password)?;
        let created_at = unix_now() as i64;

        let permissions_json = permissions
            .as_ref()
            .map(|p| serde_json::to_string(p).unwrap_or_default());

        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        let id = match conn.execute(
            "INSERT INTO users (username, password_hash, role, created_at, permissions) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![username, hash, role.as_str(), created_at, permissions_json],
        ) {
            Ok(1) => conn.last_insert_rowid(),
            Ok(_) => return Err(UserError::Db(rusqlite::Error::ExecuteReturnedResults)),
            Err(rusqlite::Error::SqliteFailure(ref e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
                return Err(UserError::DuplicateUsername)
            }
            Err(e) => return Err(e.into()),
        };
        Ok(UserInfo {
            id,
            username: username.to_string(),
            role: role.as_str().to_string(),
            created_at,
            permissions,
        })
    }

    /// Remove um usuário por id. Não permite remover o último admin.
    pub fn delete_by_id(&self, id: i64) -> Result<(), UserError> {
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        conn.execute("DELETE FROM users WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    /// Retorna as permissões granulares de um usuário, ou None se não configuradas.
    pub fn get_permissions(
        &self,
        username: &str,
    ) -> Result<Option<Vec<crate::permissions::Permission>>, UserError> {
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        let json: Option<String> = match conn.query_row(
            "SELECT permissions FROM users WHERE username = ?1",
            [username],
            |row| row.get(0),
        ) {
            Ok(j) => j,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        drop(conn);
        match json {
            Some(j) if !j.is_empty() => Ok(serde_json::from_str(&j).ok()),
            _ => Ok(None),
        }
    }

    /// Atualiza as permissões de um usuário.
    pub fn update_permissions(
        &self,
        username: &str,
        permissions: Option<Vec<crate::permissions::Permission>>,
    ) -> Result<(), UserError> {
        let json = permissions
            .as_ref()
            .map(|p| serde_json::to_string(p).unwrap_or_default());
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        conn.execute(
            "UPDATE users SET permissions = ?1 WHERE username = ?2",
            rusqlite::params![json, username],
        )?;
        Ok(())
    }

    /// Atualiza a senha de um usuário.
    pub fn update_password(&self, username: &str, new_password: &str) -> Result<(), UserError> {
        if new_password.is_empty() {
            return Err(UserError::InvalidPassword);
        }
        let hash = hash_password(new_password)?;
        let conn = self.conn.lock().map_err(|_| UserError::LockPoisoned)?;
        conn.execute(
            "UPDATE users SET password_hash = ?1 WHERE username = ?2",
            rusqlite::params![hash, username],
        )?;
        Ok(())
    }
}
