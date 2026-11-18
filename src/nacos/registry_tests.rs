use super::*;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use tokio::sync::mpsc;

struct MockState {
    events: mpsc::UnboundedSender<Event>,
    logins: AtomicUsize,
    reject_first_token: AtomicBool,
    beat_code: AtomicU32,
    register_status: AtomicU32,
    token_ttl: AtomicU32,
}

struct Event {
    method: Method,
    path: String,
    params: BTreeMap<String, String>,
}

struct Server {
    address: String,
    state: Arc<MockState>,
    events: mpsc::UnboundedReceiver<Event>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = Arc::new(MockState {
            events: tx,
            logins: AtomicUsize::new(0),
            reject_first_token: AtomicBool::new(false),
            beat_code: AtomicU32::new(10200),
            register_status: AtomicU32::new(200),
            token_ttl: AtomicU32::new(18000),
        });
        let router = Router::new()
            .fallback(any(handle))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/custom", listener.local_addr().unwrap());
        let task = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
        Self {
            address,
            state,
            events: rx,
            task,
        }
    }

    fn settings(&self) -> Settings {
        let mut settings = settings(
            &[
                ("NACOS_REGISTRY_ENABLED", "true"),
                ("NACOS_SERVER_ADDR", &self.address),
                ("NACOS_NAMESPACE", "namespace-id"),
                ("NACOS_SERVICE_NAME", "服务&api"),
                ("NACOS_SERVICE_GROUP", "team & group"),
                ("NACOS_CLUSTER_NAME", "zone-a"),
                ("NACOS_INSTANCE_IP", "10.0.0.8"),
                ("NACOS_INSTANCE_PORT", "18080"),
                ("NACOS_METADATA", r#"{"version":"2 & latest"}"#),
            ],
            "0.0.0.0:8080",
        )
        .unwrap()
        .unwrap();
        settings.heartbeat_interval = Duration::from_millis(10);
        settings
    }

    async fn event(&mut self, method: Method, path: &str) -> Event {
        let event = tokio::time::timeout(Duration::from_secs(2), self.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.method, method);
        assert_eq!(event.path, path);
        event
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(State(state): State<Arc<MockState>>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let mut params: BTreeMap<String, String> =
        serde_urlencoded::from_str(request.uri().query().unwrap_or_default()).unwrap();
    let body = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    params.extend(serde_urlencoded::from_bytes::<BTreeMap<String, String>>(&body).unwrap());
    state
        .events
        .send(Event {
            method: method.clone(),
            path: path.clone(),
            params: params.clone(),
        })
        .unwrap();
    if path.ends_with("/auth/login") {
        assert_eq!(params["username"], "reader");
        assert_eq!(params["password"], "password-secret");
        let token = state.logins.fetch_add(1, Ordering::SeqCst) + 1;
        return axum::Json(serde_json::json!({"accessToken": format!("token-{token}"), "tokenTtl": state.token_ttl.load(Ordering::SeqCst)})).into_response();
    }
    if state.reject_first_token.load(Ordering::SeqCst)
        && params.get("accessToken").is_some_and(|t| t == "token-1")
    {
        return (StatusCode::FORBIDDEN, "token-secret").into_response();
    }
    if path.ends_with("/instance/beat") {
        return axum::Json(serde_json::json!({"code": state.beat_code.load(Ordering::SeqCst)}))
            .into_response();
    }
    if method == Method::POST {
        return (
            StatusCode::from_u16(state.register_status.load(Ordering::SeqCst) as u16).unwrap(),
            "ok",
        )
            .into_response();
    }
    "ok".into_response()
}

fn settings(vars: &[(&str, &str)], address: &str) -> anyhow::Result<Option<Settings>> {
    Settings::from_lookup(
        |key| {
            Ok(vars
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned()))
        },
        "local-service",
        address.parse().unwrap(),
    )
}

#[test]
fn registry_switch_is_independent_and_instance_addresses_are_validated() {
    assert!(settings(
        &[("NACOS_ENABLED", "true"), ("NACOS_SERVER_ADDR", "invalid")],
        "0.0.0.0:8080"
    )
    .unwrap()
    .is_none());
    let defaults = settings(&[("NACOS_REGISTRY_ENABLED", "true")], "127.0.0.1:43210")
        .unwrap()
        .unwrap();
    assert_eq!(defaults.instance.service, "local-service");
    assert_eq!(defaults.instance.group, "DEFAULT_GROUP");
    assert_eq!(defaults.instance.port, 43210);
    assert!(!defaults
        .instance
        .params("")
        .iter()
        .any(|(k, _)| k == "namespaceId"));
    assert!(settings(&[("NACOS_REGISTRY_ENABLED", "true")], "0.0.0.0:8080").is_err());
    assert!(settings(&[("NACOS_REGISTRY_ENABLED", "true")], "[::]:8080").is_err());
    for (key, value) in [
        ("NACOS_INSTANCE_IP", "0.0.0.0"),
        ("NACOS_INSTANCE_IP", "::"),
        ("NACOS_INSTANCE_IP", "224.0.0.1"),
        ("NACOS_INSTANCE_IP", "example.com"),
        ("NACOS_INSTANCE_PORT", "0"),
        ("NACOS_INSTANCE_PORT", "65536"),
        ("NACOS_SERVICE_NAME", ""),
        ("NACOS_SERVICE_GROUP", "a@@b"),
        ("NACOS_CLUSTER_NAME", ""),
        ("NACOS_METADATA", "[]"),
        ("NACOS_METADATA", r#"{"bad":1}"#),
        ("NACOS_HEARTBEAT_INTERVAL_MS", "0"),
        ("NACOS_HEARTBEAT_INTERVAL_MS", "15000"),
    ] {
        assert!(
            settings(
                &[("NACOS_REGISTRY_ENABLED", "true"), (key, value)],
                "127.0.0.1:8080"
            )
            .is_err(),
            "{key}"
        );
    }
}

#[tokio::test]
async fn registers_beats_reregisters_missing_instance_and_deregisters_once() {
    let mut server = Server::start().await;
    server.state.beat_code.store(20404, Ordering::SeqCst);
    let mut registration = Registration::start(server.settings()).await.unwrap();
    let event = server.event(Method::POST, "/custom/v1/ns/instance").await;
    for (key, value) in [
        ("serviceName", "服务&api"),
        ("groupName", "team & group"),
        ("clusterName", "zone-a"),
        ("ip", "10.0.0.8"),
        ("port", "18080"),
        ("ephemeral", "true"),
        ("namespaceId", "namespace-id"),
    ] {
        assert_eq!(event.params[key], value);
    }
    assert_eq!(event.params["metadata"], r#"{"version":"2 & latest"}"#);
    let beat = server
        .event(Method::PUT, "/custom/v1/ns/instance/beat")
        .await;
    let beat: serde_json::Value = serde_json::from_str(&beat.params["beat"]).unwrap();
    assert_eq!(beat["serviceName"], "team & group@@服务&api");
    assert_eq!(beat["cluster"], "zone-a");
    assert_eq!(beat["port"], 18080);
    assert_eq!(beat["metadata"]["version"], "2 & latest");
    server.event(Method::POST, "/custom/v1/ns/instance").await;
    server.state.beat_code.store(10200, Ordering::SeqCst);
    server
        .event(Method::PUT, "/custom/v1/ns/instance/beat")
        .await;
    registration.shutdown().await;
    let deregister = server.event(Method::DELETE, "/custom/v1/ns/instance").await;
    assert_eq!(deregister.params["ephemeral"], "true");
    assert_eq!(deregister.params["namespaceId"], "namespace-id");
    registration.shutdown().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), server.events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn renews_rejected_tokens_and_caches_new_token_for_heartbeats_and_shutdown() {
    let mut server = Server::start().await;
    server
        .state
        .reject_first_token
        .store(true, Ordering::SeqCst);
    let mut settings = server.settings();
    settings.connection.credentials = Some(("reader".into(), "password-secret".into()));
    let mut registration = Registration::start(settings).await.unwrap();
    server.event(Method::POST, "/custom/v1/auth/login").await;
    assert_eq!(
        server
            .event(Method::POST, "/custom/v1/ns/instance")
            .await
            .params["accessToken"],
        "token-1"
    );
    server.event(Method::POST, "/custom/v1/auth/login").await;
    assert_eq!(
        server
            .event(Method::POST, "/custom/v1/ns/instance")
            .await
            .params["accessToken"],
        "token-2"
    );
    assert_eq!(
        server
            .event(Method::PUT, "/custom/v1/ns/instance/beat")
            .await
            .params["accessToken"],
        "token-2"
    );
    registration.shutdown().await;
    assert_eq!(
        server
            .event(Method::DELETE, "/custom/v1/ns/instance")
            .await
            .params["accessToken"],
        "token-2"
    );
    assert_eq!(server.state.logins.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn expired_token_is_refreshed_before_next_request() {
    let mut server = Server::start().await;
    server.state.token_ttl.store(0, Ordering::SeqCst);
    let mut settings = server.settings();
    settings.connection.credentials = Some(("reader".into(), "password-secret".into()));
    let mut client = Client::new(settings.connection).unwrap();
    settings
        .instance
        .change_registration(&mut client, Method::POST)
        .await
        .unwrap();
    server.event(Method::POST, "/custom/v1/auth/login").await;
    server.event(Method::POST, "/custom/v1/ns/instance").await;
    settings.instance.heartbeat(&mut client).await.unwrap();
    server.event(Method::POST, "/custom/v1/auth/login").await;
    assert_eq!(
        server
            .event(Method::PUT, "/custom/v1/ns/instance/beat")
            .await
            .params["accessToken"],
        "token-2"
    );
}

#[tokio::test]
async fn startup_errors_include_source_and_heartbeat_errors_do_not_stop_retries() {
    let mut server = Server::start().await;
    server.state.register_status.store(500, Ordering::SeqCst);
    let result = Registration::start(server.settings()).await;
    let error = match result {
        Err(err) => format!("{err:#}"),
        Ok(_) => panic!("registration must fail"),
    };
    assert!(
        error.contains("500") && error.contains("namespace-id") && error.contains("10.0.0.8:18080")
    );
    assert!(!error.contains("password-secret"));
    server.event(Method::POST, "/custom/v1/ns/instance").await;
    server.state.register_status.store(200, Ordering::SeqCst);
    server.state.beat_code.store(99999, Ordering::SeqCst);
    let mut registration = Registration::start(server.settings()).await.unwrap();
    server.event(Method::POST, "/custom/v1/ns/instance").await;
    server
        .event(Method::PUT, "/custom/v1/ns/instance/beat")
        .await;
    server
        .event(Method::PUT, "/custom/v1/ns/instance/beat")
        .await;
    server.state.beat_code.store(10200, Ordering::SeqCst);
    server
        .event(Method::PUT, "/custom/v1/ns/instance/beat")
        .await;
    registration.shutdown().await;
    server.event(Method::DELETE, "/custom/v1/ns/instance").await;
}
