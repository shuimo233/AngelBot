//! Shared database seam for orchestration components.
//!
//! The wrapper keeps the ownership/concurrency boundary in one place while
//! allowing callers to hold a connection only for the duration of one closure.
//! Existing runtime code still accepts its legacy `Arc<Mutex<Connection>>`
//! until it is migrated behind this seam.

use rusqlite::Connection;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDbError {
    message: String,
}

impl fmt::Display for SharedDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SharedDbError {}

/// Cloneable handle for the single application SQLite connection.
#[derive(Clone)]
pub struct SharedDb {
    inner: Arc<Mutex<Connection>>,
}

impl SharedDb {
    pub fn new(connection: Connection) -> Self {
        Self {
            inner: Arc::new(Mutex::new(connection)),
        }
    }

    pub fn from_arc(inner: Arc<Mutex<Connection>>) -> Self {
        Self { inner }
    }

    pub fn arc(&self) -> Arc<Mutex<Connection>> {
        self.inner.clone()
    }

    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Run a read-only operation and release the mutex before returning.
    pub fn with_conn<T>(
        &self,
        operation: impl FnOnce(&Connection) -> T,
    ) -> Result<T, SharedDbError> {
        let guard = self.lock()?;
        Ok(operation(&guard))
    }

    /// Run a mutating operation and release the mutex before returning.
    pub fn with_conn_mut<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> T,
    ) -> Result<T, SharedDbError> {
        let mut guard = self.lock()?;
        Ok(operation(&mut guard))
    }

    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, Connection>, SharedDbError> {
        self.inner
            .lock()
            .map_err(|error: PoisonError<_>| SharedDbError {
                message: format!("shared database mutex poisoned: {error}"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::SharedDb;
    use rusqlite::Connection;
    use std::sync::Arc;

    #[test]
    fn clones_share_the_same_connection_pointer() {
        let db = SharedDb::new(Connection::open_in_memory().unwrap());
        let clone = db.clone();
        assert!(Arc::ptr_eq(&db.arc(), &clone.arc()));
    }

    #[test]
    fn closure_scope_releases_the_mutex_immediately() {
        let db = SharedDb::new(Connection::open_in_memory().unwrap());
        db.with_conn(|connection| connection.is_autocommit())
            .unwrap();
        assert!(db.arc().try_lock().is_ok());
    }

    #[test]
    fn in_memory_database_round_trips_through_both_accessors() {
        let db = SharedDb::new(Connection::open_in_memory().unwrap());
        db.with_conn_mut(|connection| {
            connection
                .execute("CREATE TABLE values_table (value TEXT NOT NULL)", [])
                .unwrap();
            connection
                .execute("INSERT INTO values_table (value) VALUES ('ok')", [])
                .unwrap();
        })
        .unwrap();
        let value = db
            .with_conn(|connection| {
                connection
                    .query_row("SELECT value FROM values_table", [], |row| {
                        row.get::<_, String>(0)
                    })
                    .unwrap()
            })
            .unwrap();
        assert_eq!(value, "ok");
    }
}
