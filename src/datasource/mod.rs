//! Datasources, mirrors `backend/core/datasource/`.
//!
//! * [`MysqlDataSource`] — `@Datasource("mysql")`, parameterized SQL.
//! * [`PostgresDataSource`] — `@Datasource("postgres")`, native PostgreSQL queries.
//! * [`SqliteDataSource`] — `@Datasource("sqlite")`, file or in-memory SQLite.
//! * [`RedisDataSource`] — the Redis connection pool of
//!   `RedisConnectPool.py`.

pub mod mysql;
pub mod postgres;
pub mod redis;
pub mod sqlite;

pub use mysql::{MysqlDataSource, SqlParam};
pub use postgres::PostgresDataSource;
pub use redis::RedisDataSource;
pub use sqlite::SqliteDataSource;
