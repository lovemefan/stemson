//! PostgreSQL datasource. Queries use native `$1`, `$2`, … placeholders.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::postgres::{PgArguments, PgConnectOptions, PgPool, PgPoolOptions, PgQueryResult, PgRow};
use sqlx::query::{Query, QueryAs};
use sqlx::{FromRow, Postgres};

use crate::config::Config;
use crate::exception::{ApiError, ApiResult};

/// Lazily connected PostgreSQL pool, injectable with `Arc<PostgresDataSource>`.
#[stemson_macros::datasource("postgres")]
pub struct PostgresDataSource {
    config: Arc<Config>,
    #[skip]
    pool: OnceLock<PgPool>,
}

impl PostgresDataSource {
    /// Connection pool, initialized without contacting the server.
    pub fn pool(&self) -> &PgPool {
        self.pool.get_or_init(|| {
            let max_connections = self
                .config
                .i64("datasource.postgres.max_usage", 30)
                .clamp(1, 1000) as u32;
            let acquire_timeout = self
                .config
                .i64("datasource.postgres.acquire_timeout", 10)
                .max(1) as u64;
            PgPoolOptions::new()
                .min_connections(1)
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(acquire_timeout))
                .idle_timeout(Some(Duration::from_secs(600)))
                .connect_lazy_with(self.connect_options())
        })
    }

    fn connect_options(&self) -> PgConnectOptions {
        let config = &self.config;
        PgConnectOptions::new()
            .host(&config.string("datasource.postgres.host", "127.0.0.1"))
            .port(config.u16("datasource.postgres.port", 5432))
            .username(&config.string("datasource.postgres.user", "postgres"))
            .password(&config.string("datasource.postgres.password", ""))
            .database(&config.string("datasource.postgres.db_name", "stemson_backend_system"))
            .options([("timezone", "UTC")])
    }

    /// Run a `SELECT` returning zero or more rows.
    pub async fn fetch_all<T>(
        &self,
        query: QueryAs<'_, Postgres, T, PgArguments>,
    ) -> ApiResult<Vec<T>>
    where
        T: for<'row> FromRow<'row, PgRow> + Send + Unpin,
    {
        query.fetch_all(self.pool()).await.map_err(ApiError::from)
    }

    /// Run a `SELECT` returning at most one row.
    pub async fn fetch_optional<T>(
        &self,
        query: QueryAs<'_, Postgres, T, PgArguments>,
    ) -> ApiResult<Option<T>>
    where
        T: for<'row> FromRow<'row, PgRow> + Send + Unpin,
    {
        query
            .fetch_optional(self.pool())
            .await
            .map_err(ApiError::from)
    }

    /// Run a `SELECT` that must return exactly one row.
    pub async fn fetch_one<T>(&self, query: QueryAs<'_, Postgres, T, PgArguments>) -> ApiResult<T>
    where
        T: for<'row> FromRow<'row, PgRow> + Send + Unpin,
    {
        query.fetch_one(self.pool()).await.map_err(ApiError::from)
    }

    /// Run a statement that returns no rows.
    pub async fn execute(
        &self,
        query: Query<'_, Postgres, PgArguments>,
    ) -> ApiResult<PgQueryResult> {
        query.execute(self.pool()).await.map_err(ApiError::from)
    }

    /// Execute parameter tuples atomically. Native arguments preserve each
    /// parameter's PostgreSQL type, including typed NULLs (`None::<i64>`, etc.).
    pub async fn execute_many(&self, sql: &str, rows: Vec<PgArguments>) -> ApiResult<u64> {
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
    pub async fn begin(&self) -> ApiResult<sqlx::Transaction<'_, Postgres>> {
        self.pool().begin().await.map_err(ApiError::from)
    }

    /// Check connectivity with a round trip.
    pub async fn ping(&self) -> ApiResult<()> {
        self.execute(sqlx::query("SELECT 1")).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestContext;

    #[tokio::test]
    async fn postgres_registers_and_stays_disconnected_until_used() {
        let fixture = TestContext::with_config(
            "datasource:\n  postgres:\n    host: db.example\n    port: 15432\n    user: alice\n    db_name: stemson_test\n",
        );
        fixture.ctx().build_components().unwrap();
        let datasource = fixture.ctx().resolve::<PostgresDataSource>().unwrap();
        assert!(fixture.ctx().resolve_by_name("postgres").is_some());
        assert!(datasource.pool.get().is_none());
        let options = datasource.connect_options();
        assert_eq!(options.get_host(), "db.example");
        assert_eq!(options.get_port(), 15432);
        assert_eq!(options.get_username(), "alice");
        assert_eq!(options.get_database(), Some("stemson_test"));
        assert_eq!(datasource.execute_many("unused", vec![]).await.unwrap(), 0);
        assert!(datasource.pool.get().is_none());
    }
}
