//! Authentication endpoints, replacing `sanic-jwt` from `backend/app.py`.
//!
//! | method | path | description |
//! |--------|------|-------------|
//! | POST | `/v1/api/auth` | issue a token (Argon2id; disabled in `opaque` mode) |
//! | POST | `/v1/api/auth/login/start` | OPAQUE round 1 |
//! | POST | `/v1/api/auth/login/finish` | OPAQUE round 2, issues the token |
//! | POST | `/v1/api/auth/register/start` | OPAQUE registration round 1 |
//! | POST | `/v1/api/auth/register/finish` | OPAQUE registration round 2 (invite) |
//! | GET  | `/v1/api/auth/me` | current profile |
//! | GET  | `/v1/api/auth/verify` | validate a token |
//! | POST | `/v1/api/auth/refresh` | renew a token |

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::exception::{ApiError, ApiResult};
use crate::model::response::ApiResponse;
use crate::request::context::{extract_bearer, RequestContext};
use crate::service::UserService;

/// Login payload.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    /// Login name.
    pub username: String,
    /// Plain text password.
    pub password: String,
}

/// OPAQUE login, first round.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct LoginStartRequest {
    /// Login name.
    pub username: String,
    /// Client `CredentialRequest`, base64url.
    pub credential_request: String,
}

/// OPAQUE login, first round response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LoginStartResponse {
    /// Server `CredentialResponse`, base64url.
    pub credential_response: String,
    /// Opaque, encrypted server state to send back in round two.
    pub session: String,
}

/// OPAQUE login, second round.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct LoginFinishRequest {
    /// The `session` returned by round one.
    pub session: String,
    /// Client `CredentialFinalization`, base64url.
    pub credential_finalization: String,
}

/// OPAQUE registration, first round.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct RegisterStartRequest {
    /// Login name.
    pub username: String,
    /// Client `RegistrationRequest`, base64url.
    pub registration_request: String,
}

/// OPAQUE registration, first round response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RegisterStartResponse {
    /// Server `RegistrationResponse`, base64url.
    pub registration_response: String,
}

/// OPAQUE registration, second round.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct RegisterFinishRequest {
    /// Login name.
    pub username: String,
    /// One-time invitation issued when the administrator created the account.
    pub invite_token: String,
    /// Client `RegistrationUpload`, base64url.
    pub registration_upload: String,
}

/// Token payload returned by the auth endpoints.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TokenResponse {
    /// Signed JWT.
    pub access_token: String,
    /// Always `Bearer`.
    pub token_type: String,
    /// Lifetime in seconds.
    pub expires_in: i64,
}

/// `/v1/api/auth`
#[derive(stemson_macros::Autowired)]
pub struct AuthController {
    user_service: Arc<UserService>,
}

#[stemson_macros::controller("/v1/api/auth")]
impl AuthController {
    /// Exchange a username and password for a JWT.
    #[post]
    #[validate(
        json,
        not_empty(["username", "password"]),
        length("password", 1, 256, "Length of password must be 1 to 256"),
    )]
    async fn login(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        if !ctx.ctx().auth_mode().allows_password() {
            return Err(ApiError::invalid_parameter(
                "password login is disabled; use /v1/api/auth/login/start",
            ));
        }
        let payload: LoginRequest = ctx.json_as()?;
        let user = self
            .user_service
            .authenticate(&payload.username, &payload.password)
            .await?;
        let token =
            ctx.ctx()
                .jwt()
                .issue_for(user.uid, &user.username, user.role, user.user_identity)?;
        Ok(ApiResponse::success(TokenResponse {
            access_token: token,
            token_type: "Bearer".to_string(),
            expires_in: ctx.ctx().jwt().ttl_seconds(),
        }))
    }

    /// Return the profile of the authenticated caller.
    #[get("/me")]
    #[protected]
    async fn me(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let claims = ctx.require_user()?;
        let user = self.user_service.get_user_by_uid(claims.user_id).await?;
        Ok(ApiResponse::success(serde_json::json!({
            "me": user.to_dict(),
        })))
    }

    /// Validate the bearer token.
    #[get("/verify")]
    #[protected]
    async fn verify(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let claims = ctx.require_user()?;
        Ok(ApiResponse::success(serde_json::json!({
            "valid": true,
            "user_id": claims.user_id,
            "username": claims.username,
            "expires_at": claims.exp,
        })))
    }

    /// Issue a new token from a still valid one.
    #[post("/refresh")]
    #[protected]
    async fn refresh(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let raw = ctx
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| ApiError::unauthorized("Authorization header not present."))?;
        let token = extract_bearer(raw)?;
        let (access_token, expires_in) = ctx.ctx().jwt().refresh(token)?;
        Ok(ApiResponse::success(TokenResponse {
            access_token,
            token_type: "Bearer".to_string(),
            expires_in,
        }))
    }

    /// OPAQUE login, round one: answer the client's `CredentialRequest`.
    #[post("/login/start")]
    #[validate(json, not_empty(["username", "credential_request"]))]
    async fn login_start(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let payload: LoginStartRequest = ctx.json_as()?;
        let material = self.user_service.login_material(&payload.username).await?;
        let registration = material
            .as_ref()
            .and_then(|material| material.opaque_registration.as_deref());
        let start = ctx.ctx().opaque()?.login_start(
            &payload.username,
            registration,
            &payload.credential_request,
        )?;
        Ok(ApiResponse::success(LoginStartResponse {
            credential_response: start.response,
            session: start.session,
        }))
    }

    /// OPAQUE login, round two: verify the `CredentialFinalization` and issue
    /// the token.
    #[post("/login/finish")]
    #[validate(json, not_empty(["session", "credential_finalization"]))]
    async fn login_finish(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let payload: LoginFinishRequest = ctx.json_as()?;
        let username = ctx
            .ctx()
            .opaque()?
            .login_finish(&payload.session, &payload.credential_finalization)?;
        let user = self.user_service.get_user_by_username(&username).await?;
        if user.status == 0 {
            return Err(ApiError::forbidden("This account has been disabled."));
        }
        self.user_service.touch_login(user.uid).await?;
        let token =
            ctx.ctx()
                .jwt()
                .issue_for(user.uid, &user.username, user.role, user.user_identity)?;
        Ok(ApiResponse::success(TokenResponse {
            access_token: token,
            token_type: "Bearer".to_string(),
            expires_in: ctx.ctx().jwt().ttl_seconds(),
        }))
    }

    /// OPAQUE registration, round one.
    #[post("/register/start")]
    #[validate(json, not_empty(["username", "registration_request"]))]
    async fn register_start(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let payload: RegisterStartRequest = ctx.json_as()?;
        let response = ctx
            .ctx()
            .opaque()?
            .registration_start(&payload.username, &payload.registration_request)?;
        Ok(ApiResponse::success(RegisterStartResponse {
            registration_response: response,
        }))
    }

    /// OPAQUE registration, round two: consume the invitation and store the
    /// registration record.
    #[post("/register/finish")]
    #[validate(json, not_empty(["username", "invite_token", "registration_upload"]))]
    async fn register_finish(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let payload: RegisterFinishRequest = ctx.json_as()?;
        let record = ctx
            .ctx()
            .opaque()?
            .registration_finish(&payload.registration_upload)?;
        self.user_service
            .register_opaque(&payload.username, &payload.invite_token, record)
            .await?;
        Ok(ApiResponse::ok())
    }
}
