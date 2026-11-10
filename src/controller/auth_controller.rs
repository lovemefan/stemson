//! Authentication endpoints, replacing `sanic-jwt` from `backend/app.py`.
//!
//! | method | path | description |
//! |--------|------|-------------|
//! | POST | `/v1/api/auth` | issue a token |
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
#[derive(sanicboot_macros::Autowired)]
pub struct AuthController {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller("/v1/api/auth")]
impl AuthController {
    /// Exchange a username and password for a JWT.
    #[post]
    #[validate(
        json,
        not_empty(["username", "password"]),
        length("password", 1, 256, "Length of password must be 1 to 256"),
    )]
    async fn login(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
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
}
