//! MySQL datasource, mirrors `backend/core/datasource/MysqlDataBasePool.py`.
//!
//! ## What changed and why
//!
//! The Python implementation built SQL with f-strings and then tried to detect
//! injection with a keyword blacklist. That blocked legitimate values
//! (`Tom or Jerry`, `100%`, `+86`) while still allowing real injection
//! (`/**/`, numeric contexts, encoding tricks). This port uses **bound
//! parameters only**; the blacklist is gone (see `docs/MIGRATION.md`).
//!
//! Repository methods still *declare* their SQL; the datasource executes it and
//! maps rows into typed structs.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::mysql::{
    MySqlArguments, MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlQueryResult, MySqlRow,
};
use sqlx::query::Query;
use sqlx::query::QueryAs;
use sqlx::{FromRow, MySql};

use crate::config::Config;
use crate::exception::{ApiError, ApiResult};

/// A dynamically typed bind parameter, used by [`MysqlDataSource::execute_many`].
#[derive(Debug, Clone, PartialEq)]
pub enum SqlParam {
    /// SQL NULL.
    Null,
    /// Signed integer.
    Int(i64),
    /// Unsigned integer.
    UInt(u64),
    /// Floating point number.
    Float(f64),
    /// UTF-8 text.
    Text(String),
    /// Binary blob.
    Bytes(Vec<u8>),
    /// Boolean (stored as 0/1).
    Bool(bool),
}

impl SqlParam {
    fn bind<'q>(
        &self,
        query: Query<'q, MySql, MySqlArguments>,
    ) -> Query<'q, MySql, MySqlArguments> {
        match self {
            SqlParam::Null => query.bind(Option::<String>::None),
            SqlParam::Int(value) => query.bind(*value),
            SqlParam::UInt(value) => query.bind(*value),
            SqlParam::Float(value) => query.bind(*value),
            SqlParam::Text(value) => query.bind(value.clone()),
            SqlParam::Bytes(value) => query.bind(value.clone()),
            SqlParam::Bool(value) => query.bind(*value),
        }
    }
}

impl From<String> for SqlParam {
    fn from(value: String) -> Self {
        SqlParam::Text(value)
    }
}

impl From<&str> for SqlParam {
    fn from(value: &str) -> Self {
        SqlParam::Text(value.to_string())
    }
}

impl From<i64> for SqlParam {
    fn from(value: i64) -> Self {
        SqlParam::Int(value)
    }
}

impl From<i32> for SqlParam {
    fn from(value: i32) -> Self {
        SqlParam::Int(value as i64)
    }
}

impl From<Option<String>> for SqlParam {
    fn from(value: Option<String>) -> Self {
        match value {
            Some(text) => SqlParam::Text(text),
            None => SqlParam::Null,
        }
    }
}

impl From<Option<i64>> for SqlParam {
    fn from(value: Option<i64>) -> Self {
        match value {
            Some(number) => SqlParam::Int(number),
            None => SqlParam::Null,
        }
    }
}

/// `@Datasource("mysql")`
#[sanicboot_macros::datasource("mysql")]
pub struct MysqlDataSource {
    config: Arc<Config>,
    #[skip]
    pool: OnceLock<MySqlPool>,
}

impl MysqlDataSource {
    /// Connection pool, created lazily on first use.
    ///
    /// Lazy creation mirrors the Python implementation (which deferred the
    /// `aiomysql` pool coroutine until the first query), and lets the service
    /// start even when MySQL is temporarily unavailable.
    pub fn pool(&self) -> &MySqlPool {
        self.pool.get_or_init(|| {
            let options = self.connect_options();
            let max_connections = self
                .config
                .i64("datasource.mysql.max_usage", 30)
                .clamp(1, 1000) as u32;
            let acquire_timeout = self
                .config
                .i64("datasource.mysql.acquire_timeout", 10)
                .max(1) as u64;
            MySqlPoolOptions::new()
                .min_connections(1)
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(acquire_timeout))
                .idle_timeout(Some(Duration::from_secs(600)))
                .connect_lazy_with(options)
        })
    }

    fn connect_options(&self) -> MySqlConnectOptions {
        let config = &self.config;
        let host = config.string("datasource.mysql.host", "127.0.0.1");
        let port = config.u16("datasource.mysql.port", 3306);
        let user = config.string("datasource.mysql.user", "root");
        let password = config.string("datasource.mysql.password", "");
        let database = config.string("datasource.mysql.db_name", "");

        MySqlConnectOptions::new()
            .host(&host)
            .port(port)
            .username(&user)
            .password(&password)
            .database(&database)
            .charset("utf8mb4")
            .timezone(Some("+00:00".to_string()))
    }

    /// Run a `SELECT` returning zero or more rows.
    pub async fn fetch_all<T>(
        &self,
        query: QueryAs<'_, MySql, T, MySqlArguments>,
    ) -> ApiResult<Vec<T>>
    where
        T: for<'row> FromRow<'row, MySqlRow> + Send + Unpin,
    {
        query.fetch_all(self.pool()).await.map_err(ApiError::from)
    }

    /// Run a `SELECT` returning at most one row.
    pub async fn fetch_optional<T>(
        &self,
        query: QueryAs<'_, MySql, T, MySqlArguments>,
    ) -> ApiResult<Option<T>>
    where
        T: for<'row> FromRow<'row, MySqlRow> + Send + Unpin,
    {
        query
            .fetch_optional(self.pool())
            .await
            .map_err(ApiError::from)
    }

    /// Run a `SELECT` that must return exactly one row.
    pub async fn fetch_one<T>(&self, query: QueryAs<'_, MySql, T, MySqlArguments>) -> ApiResult<T>
    where
        T: for<'row> FromRow<'row, MySqlRow> + Send + Unpin,
    {
        query.fetch_one(self.pool()).await.map_err(ApiError::from)
    }

    /// Run a statement that returns no rows.
    pub async fn execute(
        &self,
        query: Query<'_, MySql, MySqlArguments>,
    ) -> ApiResult<MySqlQueryResult> {
        query.execute(self.pool()).await.map_err(ApiError::from)
    }

    /// Execute the same statement for many parameter tuples inside one
    /// transaction, mirroring `cursor.executemany` of the Python pool.
    pub async fn execute_many(&self, sql: &str, rows: &[Vec<SqlParam>]) -> ApiResult<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.pool().begin().await.map_err(ApiError::from)?;
        let mut affected = 0u64;
        for row in rows {
            let mut query = sqlx::query(sql);
            for param in row {
                query = param.bind(query);
            }
            let result = query
                .execute(&mut *transaction)
                .await
                .map_err(ApiError::from)?;
            affected += result.rows_affected();
        }
        transaction.commit().await.map_err(ApiError::from)?;
        Ok(affected)
    }

    /// Run several statements inside one transaction.
    ///
    /// ```ignore
    /// let mut tx = self.mysql.begin().await?;
    /// sqlx::query("...").bind(uid).execute(&mut *tx).await?;
    /// sqlx::query("...").bind(uid).execute(&mut *tx).await?;
    /// tx.commit().await?;
    /// ```
    pub async fn begin(&self) -> ApiResult<sqlx::Transaction<'_, MySql>> {
        self.pool().begin().await.map_err(ApiError::from)
    }

    /// Round trip used by `/health` style probes.
    pub async fn ping(&self) -> ApiResult<()> {
        sqlx::query("SELECT 1").execute(self.pool()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_params_are_constructed_from_rust_values() {
        assert_eq!(SqlParam::from("alice"), SqlParam::Text("alice".into()));
        assert_eq!(SqlParam::from(7i32), SqlParam::Int(7));
        assert_eq!(SqlParam::from(None::<String>), SqlParam::Null);
    }
}
