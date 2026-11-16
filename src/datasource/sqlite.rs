//! SQLite datasource. Queries use native `?` (or `?1`, `?2`) placeholders.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::query::{Query, QueryAs};
use sqlx::sqlite::{
    SqliteArguments, SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
    SqliteQueryResult, SqliteRow,
};
use sqlx::{FromRow, Sqlite};

use crate::config::Config;
use crate::exception::{ApiError, ApiResult};

/// Lazily opened SQLite pool, injectable with `Arc<SqliteDataSource>`.
#[stemson_macros::datasource("sqlite")]
pub struct SqliteDataSource {
    config: Arc<Config>,
    #[skip]
    pool: OnceLock<SqlitePool>,
}

impl SqliteDataSource {
    /// Open the configured file lazily. Its parent directory must exist.
    /// `:memory:` uses one persistent connection so every query sees the same
    /// database and pool recycling cannot silently discard its contents.
    pub fn pool(&self) -> &SqlitePool {
        self.pool.get_or_init(|| {
            let path = self.config.string("datasource.sqlite.path", "stemson.db");
            let in_memory = path == ":memory:";
            let max_connections = if in_memory {
                1
            } else {
                self.config
                    .i64("datasource.sqlite.max_usage", 5)
                    .clamp(1, 1000) as u32
            };
            let acquire_timeout = self
                .config
                .i64("datasource.sqlite.acquire_timeout", 10)
                .max(1) as u64;
            let busy_timeout = self.config.i64("datasource.sqlite.busy_timeout", 5).max(1) as u64;
            let options = SqliteConnectOptions::new()
                .filename(&path)
                .in_memory(in_memory)
                .create_if_missing(
                    self.config
                        .bool("datasource.sqlite.create_if_missing", true),
                )
                .foreign_keys(true)
                .busy_timeout(Duration::from_secs(busy_timeout))
                .journal_mode(if in_memory {
                    SqliteJournalMode::Memory
                } else {
                    SqliteJournalMode::Wal
                });
            let pool_options = SqlitePoolOptions::new()
                .min_connections(1)
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(acquire_timeout));
            if in_memory {
                pool_options.idle_timeout(None).max_lifetime(None)
            } else {
                pool_options.idle_timeout(Some(Duration::from_secs(600)))
            }
            .connect_lazy_with(options)
        })
    }

    /// Run a `SELECT` returning zero or more rows.
    pub async fn fetch_all<'q, T>(
        &self,
        query: QueryAs<'q, Sqlite, T, SqliteArguments<'q>>,
    ) -> ApiResult<Vec<T>>
    where
        T: for<'row> FromRow<'row, SqliteRow> + Send + Unpin,
    {
        query.fetch_all(self.pool()).await.map_err(ApiError::from)
    }

    /// Run a `SELECT` returning at most one row.
    pub async fn fetch_optional<'q, T>(
        &self,
        query: QueryAs<'q, Sqlite, T, SqliteArguments<'q>>,
    ) -> ApiResult<Option<T>>
    where
        T: for<'row> FromRow<'row, SqliteRow> + Send + Unpin,
    {
        query
            .fetch_optional(self.pool())
            .await
            .map_err(ApiError::from)
    }

    /// Run a `SELECT` that must return exactly one row.
    pub async fn fetch_one<'q, T>(
        &self,
        query: QueryAs<'q, Sqlite, T, SqliteArguments<'q>>,
    ) -> ApiResult<T>
    where
        T: for<'row> FromRow<'row, SqliteRow> + Send + Unpin,
    {
        query.fetch_one(self.pool()).await.map_err(ApiError::from)
    }

    /// Run a statement that returns no rows.
    pub async fn execute<'q>(
        &self,
        query: Query<'q, Sqlite, SqliteArguments<'q>>,
    ) -> ApiResult<SqliteQueryResult> {
        query.execute(self.pool()).await.map_err(ApiError::from)
    }

    /// Execute parameter tuples atomically, preserving native argument types.
    pub async fn execute_many<'q>(
        &self,
        sql: &'q str,
        rows: Vec<SqliteArguments<'q>>,
    ) -> ApiResult<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.begin().await?;
        let mut affected = 0;
        for arguments in rows {
            affected += sqlx::query_with(sql, arguments)
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        }
        transaction.commit().await?;
        Ok(affected)
    }

    /// Begin a transaction; the caller commits or rolls it back.
    pub async fn begin(&self) -> ApiResult<sqlx::Transaction<'_, Sqlite>> {
        self.pool().begin().await.map_err(ApiError::from)
    }

    /// Check connectivity with a query.
    pub async fn ping(&self) -> ApiResult<()> {
        self.execute(sqlx::query("SELECT 1")).await?;
        Ok(())
    }
}
