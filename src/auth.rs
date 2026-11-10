//! JWT authentication.
//!
//! Replaces `sanic-jwt` (`backend/app.py`) with `jsonwebtoken`.
//!
//! * `POST /v1/api/auth` issues a token (`access_token`).
//! * `GET  /v1/api/auth/me` returns the current profile.
//! * `GET  /v1/api/auth/verify` validates a token.
//! * `POST /v1/api/auth/refresh` issues a new token.
//!
//! Unlike the original configuration, the token always carries a single,
//! unambiguous `user_id` claim (the original set `user_id = "username"` in the
//! JWT config but read `payload["user_id"]` when retrieving the user, so `/me`
//! and `@protected` could never resolve a user).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::exception::{ApiError, ApiResult};
use crate::model::identity::UserIdentity;

/// JWT claims issued by stemson.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Primary key of the user (snowflake id).
    pub user_id: i64,
    /// Login name.
    pub username: String,
    /// 0 = normal user, 1 = administrator.
    pub role: i32,
    /// 1 = super admin, 2 = admin, 3 = user.
    pub identity: i32,
    /// Granted scopes.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Issued at (unix seconds).
    pub iat: i64,
    /// Not before (unix seconds).
    pub nbf: i64,
    /// Expiration (unix seconds).
    pub exp: i64,
}

impl Claims {
    /// Build a fresh claim set valid for `ttl` seconds.
    pub fn new(
        user_id: i64,
        username: impl Into<String>,
        role: i32,
        identity: UserIdentity,
        ttl_seconds: i64,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            user_id,
            username: username.into(),
            role,
            identity: identity.as_i32(),
            scopes: vec![identity.name().to_string()],
            iat: now,
            nbf: now,
            exp: now + ttl_seconds,
        }
    }

    /// Typed identity.
    pub fn user_identity(&self) -> Option<UserIdentity> {
        UserIdentity::from_i32(self.identity)
    }

    /// Whether the user is an administrator (`role != 0`), matching the
    /// original `if not user.role` checks.
    pub fn is_admin(&self) -> bool {
        self.role != 0
    }
}

/// Issuer / verifier for access tokens.
pub struct JwtService {
    encoding: jsonwebtoken::EncodingKey,
    decoding: jsonwebtoken::DecodingKey,
    validation: jsonwebtoken::Validation,
    ttl_seconds: i64,
}

impl std::fmt::Debug for JwtService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtService")
            .field("ttl_seconds", &self.ttl_seconds)
            .finish_non_exhaustive()
    }
}

impl JwtService {
    /// Build the service from `server.jwt.*` configuration.
    ///
    /// The secret can be overridden with `STEMSON_JWT_SECRET`, which is the
    /// recommended way to configure it in production.
    pub fn from_config(config: &Arc<Config>) -> Self {
        let secret = std::env::var("STEMSON_JWT_SECRET")
            .unwrap_or_else(|_| config.string("server.jwt.secret", "stemson-dev-secret"));
        let ttl_seconds = config.i64("server.jwt.expires_in", 1800).max(60);
        Self::new(secret.as_bytes(), ttl_seconds)
    }

    /// Build the service from an explicit secret.
    pub fn new(secret: &[u8], ttl_seconds: i64) -> Self {
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.leeway = 0;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "nbf"]);
        Self {
            encoding: jsonwebtoken::EncodingKey::from_secret(secret),
            decoding: jsonwebtoken::DecodingKey::from_secret(secret),
            validation,
            ttl_seconds,
        }
    }

    /// Token lifetime in seconds.
    pub fn ttl_seconds(&self) -> i64 {
        self.ttl_seconds
    }

    /// Sign a token for the given claims.
    pub fn issue(&self, claims: &Claims) -> ApiResult<String> {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            claims,
            &self.encoding,
        )
        .map_err(ApiError::from)
    }

    /// Build claims and sign them.
    pub fn issue_for(
        &self,
        user_id: i64,
        username: impl Into<String>,
        role: i32,
        identity: UserIdentity,
    ) -> ApiResult<String> {
        let claims = Claims::new(user_id, username, role, identity, self.ttl_seconds);
        self.issue(&claims)
    }

    /// Verify a token and return its claims.
    pub fn verify(&self, token: &str) -> ApiResult<Claims> {
        let data = jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation)?;
        Ok(data.claims)
    }

    /// Verify a token and issue a fresh one with a new expiration.
    pub fn refresh(&self, token: &str) -> ApiResult<(String, i64)> {
        let claims = self.verify(token)?;
        let now = chrono::Utc::now().timestamp();
        let renewed = Claims {
            iat: now,
            nbf: now,
            exp: now + self.ttl_seconds,
            ..claims
        };
        Ok((self.issue(&renewed)?, self.ttl_seconds))
    }
}

/// The authenticated caller of a request.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Verified claims.
    pub claims: Claims,
}

impl AuthUser {
    /// Wrap verified claims.
    pub fn new(claims: Claims) -> Self {
        Self { claims }
    }

    /// Typed identity, if the claim value is known.
    pub fn identity(&self) -> Option<UserIdentity> {
        self.claims.user_identity()
    }

    /// Administrator flag (`role != 0`).
    pub fn is_admin(&self) -> bool {
        self.claims.is_admin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> JwtService {
        JwtService::new(b"unit-test-secret", 1800)
    }

    #[test]
    fn round_trips_claims() {
        let service = service();
        let token = service
            .issue_for(42, "alice", 1, UserIdentity::Admin)
            .unwrap();
        let claims = service.verify(&token).unwrap();
        assert_eq!(claims.user_id, 42);
        assert_eq!(claims.username, "alice");
        assert!(claims.is_admin());
        assert_eq!(claims.user_identity(), Some(UserIdentity::Admin));
        assert!(claims.scopes.contains(&"ADMIN".to_string()));
    }

    #[test]
    fn rejects_tokens_signed_with_another_secret() {
        let token = JwtService::new(b"other-secret", 1800)
            .issue_for(1, "bob", 0, UserIdentity::User)
            .unwrap();
        let err = service().verify(&token).unwrap_err();
        assert_eq!(err.status, http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn rejects_expired_tokens_with_the_token_timeout_code() {
        let service = JwtService::new(b"unit-test-secret", 60);
        let mut claims = Claims::new(1, "bob", 0, UserIdentity::User, 60);
        let now = chrono::Utc::now().timestamp();
        claims.iat = now - 3_600;
        claims.nbf = now - 3_600;
        claims.exp = now - 1_800;
        let token = service.issue(&claims).unwrap();
        let err = service.verify(&token).unwrap_err();
        assert_eq!(
            err.code,
            crate::model::status_code::StatusCode::TokenTimeout
        );
    }
}
