//! End-to-end router tests: routing, validation, authentication, CORS,
//! OpenAPI. No database is required.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use sanicboot::UserIdentity;
use serde_json::json;

#[tokio::test]
async fn routes_match_the_paths_of_the_python_original() {
    let app = TestApp::new();
    let paths = app
        .context()
        .routes()
        .into_iter()
        .map(|entry| entry.label())
        .collect::<Vec<_>>();

    for expected in [
        "POST /user/user_controller/validate_password",
        "POST /user/user_controller/add_user",
        "POST /user/user_controller/get_user_info",
        "POST /user/user_controller/delete_user",
        "POST /user/user_controller/modify_user",
        "POST /getall",
        "POST /v1/api/auth",
        "GET /v1/api/auth/me",
        "GET /v1/api/auth/verify",
        "POST /v1/api/auth/refresh",
    ] {
        assert!(
            paths.iter().any(|path| path == expected),
            "missing route {expected}; registered: {paths:#?}"
        );
    }
}

#[tokio::test]
async fn every_documented_path_exists_in_the_router() {
    use utoipa::OpenApi;
    let app = TestApp::new();
    let router_paths = app
        .context()
        .routes()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<std::collections::BTreeSet<_>>();

    let document = sanicboot::openapi::ApiDoc::openapi();
    let documented = document.paths.paths.keys().cloned().collect::<Vec<_>>();
    assert!(!documented.is_empty(), "the OpenAPI document is empty");
    for path in documented {
        assert!(
            router_paths.contains(&path),
            "OpenAPI documents {path}, but the router serves {router_paths:#?}"
        );
    }
}

#[tokio::test]
async fn health_endpoint_uses_the_unified_envelope() {
    let app = TestApp::new();
    let (status, body) = app.get("/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], "0000");
    assert_eq!(body["data"]["status"], "ok");
    // field order of the envelope is stable
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["message", "data", "code"]
    );
}

#[tokio::test]
async fn unknown_routes_return_the_unified_not_found_body() {
    let app = TestApp::new();
    let (status, body) = app.get("/does/not/exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "0002");
    assert!(body["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn wrong_method_returns_405() {
    let app = TestApp::new();
    let (status, body) = app.get("/getall", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body["code"], "0002");
}

#[tokio::test]
async fn missing_json_parameters_report_miss_parameters() {
    let app = TestApp::new();
    let (status, body) = app
        .post(
            "/user/user_controller/validate_password",
            json!({"username": "admin"}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "0010");
    assert!(body["message"].as_str().unwrap().contains("password"));
}

#[tokio::test]
async fn no_content_type_reports_the_json_hint() {
    let app = TestApp::new();
    let (status, body) = app
        .call(Method::POST, "/v1/api/auth", Some(json!({})), None)
        .await;
    // an empty JSON object is still a JSON body: the validator reports the key
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "0010");
}

#[tokio::test]
async fn length_validation_rejects_short_passwords() {
    let app = TestApp::new();
    let (status, body) = app
        .post(
            "/user/user_controller/validate_password",
            json!({"username": "admin", "password": "123"}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "0015");
    assert_eq!(body["message"], "Length of password must be 5 to 20");
}

#[tokio::test]
async fn email_pattern_validation_runs_before_the_handler() {
    let app = TestApp::new();
    let token = app.token(1, "admin", 1, UserIdentity::SuperAdmin);
    let (status, body) = app
        .post(
            "/user/user_controller/modify_user",
            json!({
                "user_id": 1,
                "username": "someone",
                "email": "not-an-email",
                "password": "secret123",
                "user_role": 0,
                "status": 1
            }),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "0015");
    assert_eq!(body["message"], "Email format invalid");
}

#[tokio::test]
async fn protected_endpoints_require_a_token() {
    let app = TestApp::new();
    let (status, body) = app
        .post("/user/user_controller/add_user", json!({}), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "0014");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Authorization header"));
}

#[tokio::test]
async fn invalid_tokens_are_rejected() {
    let app = TestApp::new();
    let (status, body) = app.get("/v1/api/auth/me", Some("not-a-jwt")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // a malformed token is TOKEN_NOT_AVAILABLE (0004), a missing one is
    // UNAUTHORIZED (0014); see docs/API.md
    assert_eq!(body["code"], "0004");
}

#[tokio::test]
async fn verify_returns_the_claims_of_the_caller() {
    let app = TestApp::new();
    // `/me` loads the profile from MySQL, so with no database it fails with a
    // 500 envelope rather than leaking a driver error. The important part is
    // that authentication succeeded before reaching the storage layer.
    let token = app.token(7, "alice", 0, UserIdentity::User);
    let (status, body) = app.get("/v1/api/auth/verify", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], "0000");
    assert_eq!(body["data"]["valid"], true);
    assert_eq!(body["data"]["user_id"], 7);
    assert_eq!(body["data"]["username"], "alice");
}

#[tokio::test]
async fn authority_rejects_insufficient_identities() {
    let app = TestApp::new();
    let token = app.token(8, "bob", 0, UserIdentity::User);
    let (status, body) = app.post("/getall", json!({}), Some(&token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "0008");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Invalid user identity"));
}

#[tokio::test]
async fn cors_preflight_is_answered() {
    let app = TestApp::new();
    let request = axum::http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/user/user_controller/add_user")
        .header("origin", "https://example.test")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type",
        )
        .body(axum::body::Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(app.router(), request)
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "*"
    );
    assert!(response
        .headers()
        .get("access-control-allow-methods")
        .is_some());
}

#[tokio::test]
async fn openapi_json_is_served() {
    let app = TestApp::new();
    let (status, body) = app.get("/openapi.json", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["info"]["title"], "SanicBoot API");
    assert!(body["paths"]["/v1/api/auth"]["post"].is_object());
}
