//! Helpers shared by the unit tests.
//!
//! Integration tests (`tests/`) build their own fixtures from the public API.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request};
use serde_json::Value;

use crate::config::Config;
use crate::core::context::AppContext;
use crate::core::state::AppState;
use crate::request::context::RequestContext;

/// Minimal configuration used by tests; no network access is performed.
pub const DEFAULT_CONFIG: &str = r#"
server:
  jwt:
    secret: test-secret
    expires_in: 1800
  http:
    request_max_size: 1048576
    request_timeout: 30
  log:
    level: WARN
  cors:
    allowed_origins: ["*"]
datasource:
  mysql:
    host: 127.0.0.1
    port: 3306
    user: root
    password: ""
    db_name: sanic_backend_system
    max_usage: 5
redis:
  host: 127.0.0.1
  port: 6379
"#;

/// A temporary application with its own configuration file.
pub struct TestContext {
    directory: PathBuf,
    state: AppState,
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TestContext {
    /// Build a context with [`DEFAULT_CONFIG`].
    pub fn new() -> Self {
        Self::with_config(DEFAULT_CONFIG)
    }

    /// Build a context with an explicit YAML document.
    pub fn with_config(yaml: &str) -> Self {
        let directory = unique_directory();
        std::fs::create_dir_all(&directory).expect("create temp dir");
        let path = directory.join("config.yaml");
        std::fs::write(&path, yaml).expect("write config");
        let config = Config::load(&path).expect("load config");
        let context = AppContext::new(config);
        Self {
            directory,
            state: AppState::new(context),
        }
    }

    /// A clone of the application state.
    pub fn state(&self) -> AppState {
        self.state.clone()
    }

    /// The application context.
    pub fn ctx(&self) -> Arc<AppContext> {
        self.state.ctx().clone()
    }

    /// Parse a request into a [`RequestContext`].
    pub fn request(
        &self,
        method: Method,
        uri: &str,
        content_type: Option<&str>,
        body: impl Into<Body>,
        extra_headers: &[(&str, &str)],
    ) -> RequestContext {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let mut request = builder.body(body.into()).expect("request");
        for (name, value) in extra_headers {
            request.headers_mut().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        let state = self.state.clone();
        block_on(async move { RequestContext::from_request(&state, request).await })
            .expect("request context")
    }

    /// A JSON `POST` request context.
    pub fn json_request(&self, payload: &Value) -> RequestContext {
        self.request(
            Method::POST,
            "/",
            Some("application/json"),
            Body::from(serde_json::to_vec(payload).expect("json")),
            &[],
        )
    }

    /// An empty `GET` request context.
    pub fn empty_request(&self) -> RequestContext {
        self.request(Method::GET, "/", None, Body::empty(), &[])
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn unique_directory() -> PathBuf {
    let unique = format!(
        "sanicboot-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    std::env::temp_dir().join(unique)
}

/// Run a future to completion on a small current-thread runtime.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
        .block_on(future)
}
