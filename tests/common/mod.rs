//! Shared integration test fixture.
//!
//! Starts the real application (components, controllers, middleware) without
//! touching MySQL or Redis: both datasources connect lazily.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use stemson::core::context::AppContext;
use stemson::core::state::AppState;
use tower::ServiceExt;

const CONFIG: &str = r#"
server:
  name: stemson_backend
  http:
    request_max_size: 1048576
    request_timeout: 30
  jwt:
    secret: integration-secret
    expires_in: 1800
  cors:
    allowed_origins: ["*"]
  log:
    level: WARN
datasource:
  mysql:
    host: 127.0.0.1
    port: 3306
    user: root
    password: ""
    db_name: stemson_backend_system
    max_usage: 2
    acquire_timeout: 1
redis:
  host: 127.0.0.1
  port: 6379
"#;

/// A booted application under test.
pub struct TestApp {
    directory: PathBuf,
    context: Arc<AppContext>,
    router: Router,
}

impl TestApp {
    /// Boot the application with a throwaway configuration file.
    pub fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "stemson-it-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("config.yaml");
        std::fs::write(&path, CONFIG).expect("write config");

        let config = stemson::Config::load(&path).expect("load config");
        let context = AppContext::new(config);
        context.build_components().expect("build components");
        let router = stemson::build_router(AppState::new(Arc::clone(&context)));

        Self {
            directory,
            context,
            router,
        }
    }

    /// A clone of the assembled router (for requests that need custom headers).
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// The application context.
    pub fn context(&self) -> &Arc<AppContext> {
        &self.context
    }

    /// Issue a token for the given user.
    pub fn token(
        &self,
        user_id: i64,
        username: &str,
        role: i32,
        identity: stemson::UserIdentity,
    ) -> String {
        self.context
            .jwt()
            .issue_for(user_id, username, role, identity)
            .expect("issue token")
    }

    /// Perform a request, returning the HTTP status and the JSON body.
    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = match body {
            Some(value) => Body::from(serde_json::to_vec(&value).expect("json")),
            None => Body::empty(),
        };
        let request = builder.body(body).expect("request");

        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 10 * 1024 * 1024)
            .await
            .expect("body");
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, json)
    }

    /// Perform a `POST` with a JSON body.
    pub async fn post(&self, uri: &str, body: Value, token: Option<&str>) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(body), token).await
    }

    /// Perform a `GET`.
    pub async fn get(&self, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.call(Method::GET, uri, None, token).await
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
