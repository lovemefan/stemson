//! Business status codes.
//!
//! Rust port of `backend/utils/StatusCode.py`. The numeric string value is the
//! canonical, language independent business code returned in every response
//! body as `code`; the variant name is kept for logs and documentation.

use http::StatusCode as HttpStatus;

/// Business status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusCode {
    /// Success.
    Success,
    /// Request timeout.
    RequestTimeout,
    /// Resource not found.
    NotFound,
    /// Token available.
    TokenAvailable,
    /// Token not available.
    TokenNotAvailable,
    /// Username or password empty.
    UsernameOrPasswordEmpty,
    /// Username or password error.
    UsernameOrPasswordError,
    /// Token expired.
    TokenTimeout,
    /// Insufficient permission.
    PermissionDenied,
    /// Add user failed.
    AddUserFailed,
    /// Missing request parameters.
    MissParameters,
    /// Delete user failed.
    DeleteUserFailed,
    /// Modify user failed.
    ModifyUserFailed,
    /// User does not exist.
    UserNotExist,
    /// Unauthorized.
    Unauthorized,
    /// Invalid parameter.
    InvalidParameter,
    /// Internal server error.
    InternalServerError,
    /// User already exists (extension over the original enum, see `docs/MIGRATION.md`).
    UserAlreadyExist,
}

impl StatusCode {
    /// The canonical 4 digit business code.
    pub const fn code(self) -> &'static str {
        match self {
            StatusCode::Success => "0000",
            StatusCode::RequestTimeout => "0001",
            StatusCode::NotFound => "0002",
            StatusCode::TokenAvailable => "0003",
            StatusCode::TokenNotAvailable => "0004",
            StatusCode::UsernameOrPasswordEmpty => "0005",
            StatusCode::UsernameOrPasswordError => "0006",
            StatusCode::TokenTimeout => "0007",
            StatusCode::PermissionDenied => "0008",
            StatusCode::AddUserFailed => "0009",
            StatusCode::MissParameters => "0010",
            StatusCode::DeleteUserFailed => "0011",
            StatusCode::ModifyUserFailed => "0012",
            StatusCode::UserNotExist => "0013",
            StatusCode::Unauthorized => "0014",
            StatusCode::InvalidParameter => "0015",
            StatusCode::InternalServerError => "0016",
            StatusCode::UserAlreadyExist => "0017",
        }
    }

    /// The original enum variant name (stable, used by docs and logs).
    pub const fn name(self) -> &'static str {
        match self {
            StatusCode::Success => "SUCCESS",
            StatusCode::RequestTimeout => "REQUEST_TIMEOUT",
            StatusCode::NotFound => "NOT_FOUND",
            StatusCode::TokenAvailable => "TOKEN_AVAILABLE",
            StatusCode::TokenNotAvailable => "TOKEN_NOT_AVAILABLE",
            StatusCode::UsernameOrPasswordEmpty => "USERNAME_OR_PASSWORD_EMPTY",
            StatusCode::UsernameOrPasswordError => "USERNAME_OR_PASSWORD_ERROR",
            StatusCode::TokenTimeout => "TOKEN_TIMEOUT",
            StatusCode::PermissionDenied => "PERMISSION_DENIED",
            StatusCode::AddUserFailed => "ADD_USER_FAILED",
            StatusCode::MissParameters => "MISS_PARAMETERS",
            StatusCode::DeleteUserFailed => "DELETE_USER_FAILED",
            StatusCode::ModifyUserFailed => "MODIFY_USER_FAILED",
            StatusCode::UserNotExist => "USER_NOT_EXIST",
            StatusCode::Unauthorized => "UNAUTHORIZED",
            StatusCode::InvalidParameter => "INVALID_PARAMETER",
            StatusCode::InternalServerError => "INTERNAL_SERVER_ERROR",
            StatusCode::UserAlreadyExist => "USER_ALREADY_EXIST",
        }
    }

    /// The HTTP status that carries this business code.
    ///
    /// The original project returned `401` for validation failures and `403`
    /// for missing authentication; that is corrected here (see
    /// `docs/MIGRATION.md`).
    pub const fn http_status(self) -> HttpStatus {
        match self {
            StatusCode::Success | StatusCode::TokenAvailable => HttpStatus::OK,
            StatusCode::RequestTimeout => HttpStatus::REQUEST_TIMEOUT,
            StatusCode::NotFound | StatusCode::UserNotExist => HttpStatus::NOT_FOUND,
            StatusCode::UsernameOrPasswordEmpty
            | StatusCode::AddUserFailed
            | StatusCode::MissParameters
            | StatusCode::DeleteUserFailed
            | StatusCode::ModifyUserFailed
            | StatusCode::InvalidParameter => HttpStatus::BAD_REQUEST,
            StatusCode::UsernameOrPasswordError
            | StatusCode::TokenTimeout
            | StatusCode::TokenNotAvailable
            | StatusCode::Unauthorized => HttpStatus::UNAUTHORIZED,
            StatusCode::PermissionDenied => HttpStatus::FORBIDDEN,
            StatusCode::UserAlreadyExist => HttpStatus::CONFLICT,
            StatusCode::InternalServerError => HttpStatus::INTERNAL_SERVER_ERROR,
        }
    }

    /// Default English message used when no specific message is supplied.
    pub const fn default_message(self) -> &'static str {
        match self {
            StatusCode::Success => "success",
            StatusCode::RequestTimeout => "request timeout",
            StatusCode::NotFound => "resource not found",
            StatusCode::TokenAvailable => "token is available",
            StatusCode::TokenNotAvailable => "token is not available",
            StatusCode::UsernameOrPasswordEmpty => "username or password is empty",
            StatusCode::UsernameOrPasswordError => "username or password is incorrect",
            StatusCode::TokenTimeout => "token has expired",
            StatusCode::PermissionDenied => "permission denied",
            StatusCode::AddUserFailed => "add user failed",
            StatusCode::MissParameters => "missing parameters",
            StatusCode::DeleteUserFailed => "delete user failed",
            StatusCode::ModifyUserFailed => "modify user failed",
            StatusCode::UserNotExist => "user does not exist",
            StatusCode::Unauthorized => "unauthorized",
            StatusCode::InvalidParameter => "invalid parameter",
            StatusCode::InternalServerError => "internal server error",
            StatusCode::UserAlreadyExist => "user already exists",
        }
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_four_digits() {
        let all = [
            StatusCode::Success,
            StatusCode::RequestTimeout,
            StatusCode::NotFound,
            StatusCode::TokenAvailable,
            StatusCode::TokenNotAvailable,
            StatusCode::UsernameOrPasswordEmpty,
            StatusCode::UsernameOrPasswordError,
            StatusCode::TokenTimeout,
            StatusCode::PermissionDenied,
            StatusCode::AddUserFailed,
            StatusCode::MissParameters,
            StatusCode::DeleteUserFailed,
            StatusCode::ModifyUserFailed,
            StatusCode::UserNotExist,
            StatusCode::Unauthorized,
            StatusCode::InvalidParameter,
            StatusCode::InternalServerError,
            StatusCode::UserAlreadyExist,
        ];
        let mut codes: Vec<&str> = all.iter().map(|c| c.code()).collect();
        assert!(codes.iter().all(|c| c.len() == 4));
        let len = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), len, "business codes must be unique");
    }

    #[test]
    fn original_codes_are_preserved() {
        assert_eq!(StatusCode::Success.code(), "0000");
        assert_eq!(StatusCode::MissParameters.code(), "0010");
        assert_eq!(StatusCode::InternalServerError.code(), "0016");
    }
}
