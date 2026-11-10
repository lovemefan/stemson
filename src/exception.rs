//! Unified error type.
//!
//! Rust port of `backend/exception/*.py` together with the global exception
//! handlers of `backend/app.py`. Every error becomes a
//! [`ApiResponse`](crate::model::response::ApiResponse) with a business `code`
//! and a semantically correct HTTP status.

use axum::response::{IntoResponse, Response};
use http::StatusCode as HttpStatus;

use crate::model::response::ApiResponse;
use crate::model::status_code::StatusCode;

/// Convenient result alias used across the framework.
pub type ApiResult<T> = Result<T, ApiError>;

/// An error that is safe to render as the unified API response.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// Business status code.
    pub code: StatusCode,
    /// HTTP status to send.
    pub status: HttpStatus,
    /// Message returned to the client.
    pub message: String,
    /// Internal details, logged but never sent to the client.
    pub detail: Option<String>,
}

impl ApiError {
    /// Build an error from a business code and a message.
    pub fn new(code: StatusCode, message: impl Into<String>) -> Self {
        Self {
            code,
            status: code.http_status(),
            message: message.into(),
            detail: None,
        }
    }

    /// Override the HTTP status.
    pub fn with_status(mut self, status: HttpStatus) -> Self {
        self.status = status;
        self
    }

    /// Attach internal detail (logged only).
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Missing request parameter (`MISS_PARAMETERS`, 400).
    pub fn miss_parameters(message: impl Into<String>) -> Self {
        Self::new(StatusCode::MissParameters, message)
    }

    /// Invalid parameter value (`INVALID_PARAMETER`, 400).
    pub fn invalid_parameter(message: impl Into<String>) -> Self {
        Self::new(StatusCode::InvalidParameter, message)
    }

    /// Missing or invalid credentials (`UNAUTHORIZED`, 401).
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::Unauthorized, message)
    }

    /// Valid credentials but insufficient permission (`PERMISSION_DENIED`, 403).
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PermissionDenied, message)
    }

    /// Resource not found (`NOT_FOUND`, 404).
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NotFound, message)
    }

    /// User does not exist (`USER_NOT_EXIST`, 404).
    pub fn user_not_exist(username: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UserNotExist,
            format!("User '{}' is not exist", username.into()),
        )
    }

    /// User already exists (`USER_ALREADY_EXIST`, 409).
    pub fn user_already_exist(username: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UserAlreadyExist,
            format!("User '{}' is already exist", username.into()),
        )
    }

    /// Wrong username or password (`USERNAME_OR_PASSWORD_ERROR`, 401).
    pub fn bad_credentials() -> Self {
        Self::new(
            StatusCode::UsernameOrPasswordError,
            "User not found or password is incorrect.",
        )
    }

    /// Request timeout (`REQUEST_TIMEOUT`, 408).
    pub fn request_timeout() -> Self {
        Self::new(StatusCode::RequestTimeout, "request timeout")
    }

    /// Internal failure (`INTERNAL_SERVER_ERROR`, 500).
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::InternalServerError, "Internal server error")
            .with_detail(detail.into())
    }

    /// SQL failure (`INTERNAL_SERVER_ERROR`, 500).
    pub fn sql(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::InternalServerError, "SQL execution failed.")
            .with_detail(detail.into())
    }

    /// Whether this error should be logged at error level.
    pub fn is_server_error(&self) -> bool {
        self.status.is_server_error()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{} [{}]: {}", self.code.name(), self.code.code(), detail),
            None => write!(
                f,
                "{} [{}]: {}",
                self.code.name(),
                self.code.code(),
                self.message
            ),
        }
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.is_server_error() {
            tracing::error!(
                code = self.code.code(),
                status = self.status.as_u16(),
                detail = self.detail.as_deref().unwrap_or(&self.message),
                "request failed"
            );
        } else {
            tracing::debug!(
                code = self.code.code(),
                status = self.status.as_u16(),
                message = %self.message,
                "request rejected"
            );
        }

        let body = ApiResponse::<serde_json::Value>::message(self.message, self.code);
        (self.status, axum::Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        ApiError::sql(err.to_string())
    }
}

impl From<jsonwebtoken::errors::Error> for ApiError {
    fn from(err: jsonwebtoken::errors::Error) -> Self {
        use jsonwebtoken::errors::ErrorKind;
        match err.kind() {
            ErrorKind::ExpiredSignature => {
                ApiError::new(StatusCode::TokenTimeout, "Signature has expired.")
                    .with_detail(err.to_string())
            }
            _ => ApiError::new(StatusCode::TokenNotAvailable, "Token is invalid.")
                .with_detail(err.to_string()),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::invalid_parameter("Malformed JSON body").with_detail(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_parameters_uses_400() {
        let err = ApiError::miss_parameters("username is empty");
        assert_eq!(err.status, HttpStatus::BAD_REQUEST);
        assert_eq!(err.code, StatusCode::MissParameters);
    }

    #[test]
    fn unauthorized_uses_401_and_permission_uses_403() {
        assert_eq!(
            ApiError::unauthorized("no token").status,
            HttpStatus::UNAUTHORIZED
        );
        assert_eq!(
            ApiError::forbidden("not admin").status,
            HttpStatus::FORBIDDEN
        );
    }

    #[test]
    fn internal_error_hides_detail_from_message() {
        let err = ApiError::internal("connection refused");
        assert_eq!(err.message, "Internal server error");
        assert_eq!(err.detail.as_deref(), Some("connection refused"));
    }
}
