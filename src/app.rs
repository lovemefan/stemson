//! Application bootstrap and middleware, mirrors the bottom half of
//! `backend/app.py`.

use std::any::Any;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower::timeout::TimeoutLayer;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{Any as CorsAny, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::core::context::AppContext;
use crate::core::state::AppState;
use crate::exception::ApiError;
use crate::model::response::ApiResponse;
use crate::openapi;
use crate::utils::{logger, snowflake};

/// Load the configuration, build every component and serve HTTP.
///
/// This is what `python backend/app.py` did.
pub async fn run() -> anyhow::Result<()> {
    let config = Config::load_default()?;
    let _logger_guard = logger::init(&config)?;

    let datacenter_id = config.i64("server.snowflake.datacenter_id", 1).max(0) as u64;
    let worker_id = config.i64("server.snowflake.worker_id", 2).max(0) as u64;
    snowflake::init(datacenter_id, worker_id)?;

    let context = AppContext::new(Arc::clone(&config));
    if config.bool("server.config_watch", true) {
        if let Err(err) = context.watch_config() {
            tracing::warn!(error = %err, "configuration hot reload is unavailable");
        }
    }
    context.build_components()?;

    tracing::info!(
        name = %config.string("server.name", "sanic_backend"),
        components = context.component_count(),
        controllers = context.controllers().len(),
        "application initialized"
    );

    let state = AppState::new(Arc::clone(&context));
    let router = build_router(state);

    let host = config.string("server.http.host", "0.0.0.0");
    let port = config.u16("server.http.port", 80);
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|err| anyhow::anyhow!("invalid listen address {host}:{port}: {err}"))?;

    tracing::info!(%address, "listening");
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("server stopped");
    Ok(())
}

/// Assemble the router: controller routes, health probe, OpenAPI/Swagger, then
/// the global middleware stack.
pub fn build_router(state: AppState) -> Router {
    let context = Arc::clone(state.ctx());

    let mut api: Router<AppState> = Router::new();
    let mut entries = Vec::new();
    for controller in context.controllers() {
        entries.extend(controller.route_entries());
        api = api.merge(controller.routes());
    }
    context.register_routes(entries);

    api = api.route("/health", get(health));
    context.register_routes([crate::core::route::RouteEntry::new(
        "/health",
        Method::GET,
        "health",
    )]);
    let app: Router = api.with_state(state);

    let app = app
        .merge(openapi::swagger())
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed);

    app.layer(cors_layer(context.config()))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::custom(handle_panic))
        .pipe_timeout(context.config())
}

trait TimeoutExt: Sized {
    /// Apply the optional request timeout configured by
    /// `server.http.request_timeout` (seconds, `0` disables it).
    fn pipe_timeout(self, config: &Config) -> Self;
}

impl TimeoutExt for Router {
    fn pipe_timeout(self, config: &Config) -> Self {
        let seconds = config.i64("server.http.request_timeout", 30);
        if seconds <= 0 {
            return self;
        }
        let timeout = tower::ServiceBuilder::new()
            .layer(HandleErrorLayer::new(handle_timeout))
            .layer(TimeoutLayer::new(Duration::from_secs(seconds as u64)));
        self.layer(timeout)
    }
}

async fn handle_timeout(_error: tower::BoxError) -> ApiError {
    ApiError::request_timeout()
}

fn handle_panic(payload: Box<dyn Any + Send + 'static>) -> Response {
    let detail = payload
        .downcast_ref::<String>()
        .map(|value| value.as_str())
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic payload");
    tracing::error!(detail, "handler panicked");
    ApiError::internal(detail.to_string()).into_response()
}

/// CORS, mirroring `cors_middle_req` / `cors_middle_res` of `app.py`.
///
/// `*` (the original behavior) is the default; set
/// `server.cors.allowed_origins` to an explicit list in production.
fn cors_layer(config: &Config) -> CorsLayer {
    let layer = CorsLayer::new().allow_methods(CorsAny).allow_headers([
        header::AUTHORIZATION,
        header::CONTENT_TYPE,
        header::ACCEPT,
    ]);

    let origins: Vec<HeaderValue> = config
        .get("server.cors.allowed_origins")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str())
        .filter_map(|value| HeaderValue::from_str(value).ok())
        .collect();

    if origins.is_empty() || origins.iter().any(|value| value.as_bytes() == b"*") {
        layer.allow_origin(CorsAny)
    } else {
        layer.allow_origin(origins)
    }
}

async fn health(State(state): State<AppState>) -> ApiResponse {
    ApiResponse::success(serde_json::json!({
        "status": "ok",
        "name": state.config().string("server.name", "sanic_backend"),
        "uptime_seconds": state.ctx().uptime().as_secs(),
        "components": state.ctx().component_count(),
    }))
}

async fn not_found(uri: Uri) -> ApiError {
    ApiError::not_found(format!("Requested URL {uri} not found"))
}

async fn method_not_allowed(method: Method, uri: Uri) -> ApiError {
    ApiError::new(
        crate::model::status_code::StatusCode::NotFound,
        format!("Method {method} is not allowed for URL {uri}"),
    )
    .with_status(http::StatusCode::METHOD_NOT_ALLOWED)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to listen for ctrl-c");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
