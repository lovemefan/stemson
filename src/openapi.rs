//! OpenAPI document and Swagger UI, replacing the `sanic-openapi` blueprint.
//!
//! The document is declared explicitly (the original relied on
//! `sanic_openapi` decorators); `tests/router.rs` asserts that every declared
//! path also exists in the router, so the two cannot drift silently.

use axum::Router;
use utoipa::OpenApi;

use crate::controller::auth_controller::{LoginRequest, TokenResponse};
use crate::controller::user::user_controller::{
    CreateUserRequest, DeleteUserRequest, GetUserInfoRequest, ModifyUserRequest,
    ValidatePasswordRequest,
};

/// Response envelope used in the documentation.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ApiResponseDoc {
    /// Human readable message.
    #[schema(example = "success")]
    pub message: String,
    /// Payload, `null` when there is none.
    pub data: Option<serde_json::Value>,
    /// 4 digit business code.
    #[schema(example = "0000")]
    pub code: String,
}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/v1/api/auth",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "A token was issued", body = ApiResponseDoc),
        (status = 400, description = "Missing or malformed parameters", body = ApiResponseDoc),
        (status = 401, description = "Wrong username or password", body = ApiResponseDoc)
    )
)]
fn doc_login() {}

#[allow(dead_code)]
#[utoipa::path(
    get,
    path = "/v1/api/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The current profile", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc)
    )
)]
fn doc_me() {}

#[allow(dead_code)]
#[utoipa::path(
    get,
    path = "/v1/api/auth/verify",
    tag = "auth",
    responses(
        (status = 200, description = "The token is valid", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc)
    )
)]
fn doc_verify() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/v1/api/auth/refresh",
    tag = "auth",
    responses(
        (status = 200, description = "A new token was issued", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc)
    )
)]
fn doc_refresh() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/user/user_controller/validate_password",
    tag = "user",
    request_body = ValidatePasswordRequest,
    responses(
        (status = 200, description = "Credentials are valid", body = ApiResponseDoc),
        (status = 400, description = "Validation failed", body = ApiResponseDoc),
        (status = 401, description = "Wrong credentials", body = ApiResponseDoc)
    )
)]
fn doc_validate_password() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/user/user_controller/add_user",
    tag = "user",
    request_body = CreateUserRequest,
    responses(
        (status = 200, description = "The created user", body = ApiResponseDoc),
        (status = 400, description = "Invalid parameters", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc),
        (status = 403, description = "Not an administrator", body = ApiResponseDoc),
        (status = 409, description = "Username already exists", body = ApiResponseDoc)
    )
)]
fn doc_add_user() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/user/user_controller/get_user_info",
    tag = "user",
    request_body = GetUserInfoRequest,
    responses(
        (status = 200, description = "The requested user", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc),
        (status = 403, description = "Not an administrator", body = ApiResponseDoc),
        (status = 404, description = "User does not exist", body = ApiResponseDoc)
    )
)]
fn doc_get_user_info() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/user/user_controller/delete_user",
    tag = "user",
    request_body = DeleteUserRequest,
    responses(
        (status = 200, description = "The deleted user", body = ApiResponseDoc),
        (status = 400, description = "Missing user_id", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc),
        (status = 403, description = "Not an administrator", body = ApiResponseDoc)
    )
)]
fn doc_delete_user() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/user/user_controller/modify_user",
    tag = "user",
    request_body = ModifyUserRequest,
    responses(
        (status = 200, description = "The modified user", body = ApiResponseDoc),
        (status = 400, description = "Invalid parameters", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc),
        (status = 403, description = "Not an administrator", body = ApiResponseDoc)
    )
)]
fn doc_modify_user() {}

#[allow(dead_code)]
#[utoipa::path(
    post,
    path = "/getall",
    tag = "user",
    responses(
        (status = 200, description = "Every user", body = ApiResponseDoc),
        (status = 401, description = "Missing or invalid token", body = ApiResponseDoc),
        (status = 403, description = "Insufficient identity", body = ApiResponseDoc)
    )
)]
fn doc_get_all() {}

#[allow(dead_code)]
#[utoipa::path(
    get,
    path = "/health",
    tag = "ops",
    responses((status = 200, description = "Liveness probe", body = ApiResponseDoc))
)]
fn doc_health() {}

/// The generated OpenAPI document.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Stemson API",
        version = "2.0.0",
        description = "Spring-Boot-like Axum template. Every response uses the \
                       `{message, data, code}` envelope described in `docs/API.md`."
    ),
    paths(
        doc_login,
        doc_me,
        doc_verify,
        doc_refresh,
        doc_validate_password,
        doc_add_user,
        doc_get_user_info,
        doc_delete_user,
        doc_modify_user,
        doc_get_all,
        doc_health
    ),
    components(schemas(
        ApiResponseDoc,
        LoginRequest,
        TokenResponse,
        ValidatePasswordRequest,
        CreateUserRequest,
        GetUserInfoRequest,
        DeleteUserRequest,
        ModifyUserRequest
    ))
)]
pub struct ApiDoc;

/// Swagger UI mounted at `/swagger`, spec served at `/openapi.json`.
pub fn swagger() -> Router {
    Router::new().merge(
        utoipa_swagger_ui::SwaggerUi::new("/swagger").url("/openapi.json", ApiDoc::openapi()),
    )
}
