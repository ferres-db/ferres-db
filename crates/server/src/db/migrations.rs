use rusqlite::Connection;
use thiserror::Error;
use crate::time::unix_now;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("downgrade detected: database at version {current}, highest known migration is {max_known}")]
    DowngradeDetected { current: i64, max_known: i64 },
}

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub up: &'static str,
}

pub static MIGRATIONS_API_KEYS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_api_keys",
        up: "CREATE TABLE IF NOT EXISTS api_keys (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL, \
            key_hash TEXT NOT NULL UNIQUE, \
            key_prefix TEXT NOT NULL, \
            created_at INTEGER NOT NULL\
        )",
    },
    Migration {
        version: 2,
        name: "api_keys_add_allowed_namespaces",
        up: "ALTER TABLE api_keys ADD COLUMN allowed_namespaces TEXT",
    },
];

pub static MIGRATIONS_USERS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_users",
        up: "CREATE TABLE IF NOT EXISTS users (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            username TEXT NOT NULL UNIQUE, \
            password_hash TEXT NOT NULL, \
            created_at INTEGER NOT NULL\
        )",
    },
    Migration {
        version: 2,
        name: "users_add_role",
        up: "ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'viewer'",
    },
    Migration {
        version: 3,
        name: "users_add_permissions",
        up: "ALTER TABLE users ADD COLUMN permissions TEXT DEFAULT NULL",
    },
];

pub static MIGRATIONS_CLOUD_SETTINGS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_cloud_settings",
        up: "CREATE TABLE cloud_settings (\
            id INTEGER PRIMARY KEY CHECK (id = 1), \
            region TEXT, \
            bucket TEXT, \
            access_key_id TEXT, \
            secret_access_key TEXT\
        ); \
        INSERT OR IGNORE INTO cloud_settings (id, region, bucket, access_key_id, secret_access_key) \
        VALUES (1, NULL, NULL, NULL, NULL);",
    },
    Migration {
        version: 2,
        name: "cloud_settings_add_endpoint",
        up: "ALTER TABLE cloud_settings ADD COLUMN endpoint TEXT",
    },
];

pub static MIGRATIONS_LLM_CREDENTIALS: &[Migration] = &[Migration {
    version: 1,
    name: "create_llm_credentials",
    up: "CREATE TABLE IF NOT EXISTS llm_credentials (\
        provider TEXT PRIMARY KEY, \
        api_key TEXT NOT NULL, \
        updated_at INTEGER NOT NULL\
    )",
}];

pub fn run_migrations(conn: &Connection, migrations: &[Migration]) -> Result<(), MigrationError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
            version INTEGER PRIMARY KEY, \
            name TEXT NOT NULL, \
            applied_at INTEGER NOT NULL\
        )",
    )?;

    let schema_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if schema_count == 0 {
        let other_tables: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name != 'schema_migrations'",
            [],
            |row| row.get(0),
        )?;

        if other_tables > 0 {
            let now = unix_now() as i64;
            let tx = conn.unchecked_transaction()?;
            for m in migrations {
                tx.execute(
                    "INSERT OR IGNORE INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![m.version, m.name, now],
                )?;
            }
            tx.commit()?;
            return Ok(());
        }
    }

    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    let max_known = migrations.iter().map(|m| m.version).max().unwrap_or(0);

    if current > max_known {
        return Err(MigrationError::DowngradeDetected { current, max_known });
    }

    let now = unix_now() as i64;
    for m in migrations.iter().filter(|m| m.version > current) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(m.up)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![m.version, m.name, now],
        )?;
        tx.commit()?;
        tracing::info!(version = m.version, name = m.name, "migration applied");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn open_db(dir: &TempDir) -> Connection {
        Connection::open(dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_migrations_apply_in_order() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir);

        run_migrations(&conn, MIGRATIONS_API_KEYS).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        let versions: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT version FROM schema_migrations ORDER BY version")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(|v| v.unwrap())
                .collect()
        };
        assert_eq!(versions, vec![1, 2]);

        let col_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name='allowed_namespaces'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_exists, 1);
    }

    #[test]
    fn test_migrations_idempotent() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir);

        run_migrations(&conn, MIGRATIONS_API_KEYS).unwrap();
        run_migrations(&conn, MIGRATIONS_API_KEYS).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_migrations_partial_recovery() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir);

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                version INTEGER PRIMARY KEY, \
                name TEXT NOT NULL, \
                applied_at INTEGER NOT NULL\
            )",
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS api_keys (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                name TEXT NOT NULL, \
                key_hash TEXT NOT NULL UNIQUE, \
                key_prefix TEXT NOT NULL, \
                created_at INTEGER NOT NULL\
            )",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (1, 'create_api_keys', 0)",
            [],
        )
        .unwrap();

        run_migrations(&conn, MIGRATIONS_API_KEYS).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        let col_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name='allowed_namespaces'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_exists, 1);
    }

    #[test]
    fn test_migrations_detect_downgrade() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir);

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                version INTEGER PRIMARY KEY, \
                name TEXT NOT NULL, \
                applied_at INTEGER NOT NULL\
            )",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (99, 'future_migration', 0)",
            [],
        )
        .unwrap();

        let result = run_migrations(&conn, MIGRATIONS_API_KEYS);
        assert!(matches!(
            result,
            Err(MigrationError::DowngradeDetected { current: 99, max_known: 2 })
        ));
    }
}
