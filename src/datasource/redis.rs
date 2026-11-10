//! Redis datasource, mirrors `backend/core/datasource/RedisConnectPool.py`.
//!
//! One connection manager per process, created lazily. The Python version
//! offered both a sync and an async client; an async-only client is exposed
//! here because the whole server is async.

use std::sync::{Arc, OnceLock};

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde_json::Value;

use crate::config::Config;
use crate::exception::{ApiError, ApiResult};

/// `@Datasource("redis")`
#[stemson_macros::datasource("redis")]
pub struct RedisDataSource {
    config: Arc<Config>,
    #[skip]
    client: OnceLock<Client>,
    #[skip]
    connection: tokio::sync::OnceCell<ConnectionManager>,
}

impl RedisDataSource {
    fn client(&self) -> ApiResult<&Client> {
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let url = self.connection_url();
        let client = Client::open(url).map_err(|err| {
            ApiError::internal("failed to create the Redis client").with_detail(err.to_string())
        })?;
        let _ = self.client.set(client);
        self.client
            .get()
            .ok_or_else(|| ApiError::internal("Redis client initialization failed"))
    }

    fn connection_url(&self) -> String {
        let host = self.config.string("redis.host", "127.0.0.1");
        let port = self.config.u16("redis.port", 6379);
        let db = self.config.i64("redis.db", 0);
        let password = self.config.string("redis.password", "");
        if password.is_empty() {
            format!("redis://{host}:{port}/{db}")
        } else {
            format!("redis://:{password}@{host}:{port}/{db}")
        }
    }

    /// The shared connection manager (cloned cheaply, it multiplexes).
    pub async fn connection(&self) -> ApiResult<ConnectionManager> {
        if let Some(connection) = self.connection.get() {
            return Ok(connection.clone());
        }
        let client = self.client()?;
        let connection = ConnectionManager::new(client.clone())
            .await
            .map_err(|err| {
                ApiError::internal("failed to connect to Redis").with_detail(err.to_string())
            })?;
        let connection = self.connection.get_or_init(|| async { connection }).await;
        Ok(connection.clone())
    }

    /// `SET key value`
    pub async fn set(&self, key: &str, value: impl Into<String>) -> ApiResult<()> {
        let mut connection = self.connection().await?;
        connection
            .set::<_, _, ()>(key, value.into())
            .await
            .map_err(|err| ApiError::internal("Redis SET failed").with_detail(err.to_string()))
    }

    /// `SETEX key ttl value`
    pub async fn set_ex(&self, key: &str, seconds: u64, value: impl Into<String>) -> ApiResult<()> {
        let mut connection = self.connection().await?;
        connection
            .set_ex::<_, _, ()>(key, value.into(), seconds)
            .await
            .map_err(|err| ApiError::internal("Redis SETEX failed").with_detail(err.to_string()))
    }

    /// `GET key`
    pub async fn get(&self, key: &str) -> ApiResult<Option<String>> {
        let mut connection = self.connection().await?;
        connection
            .get::<_, Option<String>>(key)
            .await
            .map_err(|err| ApiError::internal("Redis GET failed").with_detail(err.to_string()))
    }

    /// `GET` and deserialize JSON.
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> ApiResult<Option<T>> {
        let Some(raw) = self.get(key).await? else {
            return Ok(None);
        };
        let value: Value = serde_json::from_str(&raw).map_err(|err| {
            ApiError::internal("stored Redis value is not valid JSON").with_detail(err.to_string())
        })?;
        serde_json::from_value(value).map(Some).map_err(|err| {
            ApiError::internal("stored Redis JSON has an unexpected shape")
                .with_detail(err.to_string())
        })
    }

    /// `DEL key`
    pub async fn del(&self, key: &str) -> ApiResult<u64> {
        let mut connection = self.connection().await?;
        connection
            .del::<_, u64>(key)
            .await
            .map_err(|err| ApiError::internal("Redis DEL failed").with_detail(err.to_string()))
    }

    /// `PING`
    pub async fn ping(&self) -> ApiResult<()> {
        let mut connection = self.connection().await?;
        redis::cmd("PING")
            .query_async::<String>(&mut connection)
            .await
            .map_err(|err| ApiError::internal("Redis PING failed").with_detail(err.to_string()))?;
        Ok(())
    }
}
