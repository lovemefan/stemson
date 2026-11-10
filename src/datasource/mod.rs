//! Datasources, mirrors `backend/core/datasource/`.
//!
//! * [`MysqlDataSource`] — `@Datasource("mysql")`, parameterized SQL.
//! * [`RedisDataSource`] — the Redis connection pool of
//!   `RedisConnectPool.py`.

pub mod mysql;
pub mod redis;

pub use mysql::{MysqlDataSource, SqlParam};
pub use redis::RedisDataSource;
