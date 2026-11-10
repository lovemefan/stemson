//! Unified response body.
//!
//! Rust port of `backend/model/ResponseBody.py`:
//!
//! ```json
//! { "message": "...", "data": null, "code": "0000" }
//! ```
//!
//! Differences from the Python version (see `docs/MIGRATION.md`):
//! * `code` is always the 4 digit business code from [`StatusCode`].
//! * payloads go into `data`; the Python controllers sometimes stuffed lists
//!   into `message`.

use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;

use crate::model::status_code::StatusCode;

/// Unified JSON response body.
#[derive(Debug, Clone, Serialize)]
pub struct ApiResponse<T = Value> {
    pub message: String,
    pub data: Option<T>,
    pub code: &'static str,
}

impl<T> ApiResponse<T> {
    /// Build a response with an explicit payload.
    pub fn new(message: impl Into<String>, data: Option<T>, code: StatusCode) -> Self {
        Self {
            message: message.into(),
            data,
            code: code.code(),
        }
    }

    /// Map the payload while keeping message and code.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ApiResponse<U> {
        ApiResponse {
            message: self.message,
            data: self.data.map(f),
            code: self.code,
        }
    }
}

impl ApiResponse<Value> {
    /// `{"message":"success","data":null,"code":"0000"}`
    pub fn ok() -> Self {
        Self::new(
            StatusCode::Success.default_message(),
            None,
            StatusCode::Success,
        )
    }

    /// Successful response carrying a JSON payload.
    pub fn success(data: impl Serialize) -> Self {
        Self::new(
            StatusCode::Success.default_message(),
            Some(serde_json::to_value(data).unwrap_or(Value::Null)),
            StatusCode::Success,
        )
    }

    /// Successful response with a custom message and payload.
    pub fn success_with_message(message: impl Into<String>, data: impl Serialize) -> Self {
        Self::new(
            message,
            Some(serde_json::to_value(data).unwrap_or(Value::Null)),
            StatusCode::Success,
        )
    }

    /// Response with a message and no payload.
    pub fn message(message: impl Into<String>, code: StatusCode) -> Self {
        Self::new(message, None, code)
    }
}

impl IntoResponse for ApiResponse<Value> {
    fn into_response(self) -> Response {
        (http::StatusCode::OK, axum::Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_message_data_code_in_order() {
        let body = ApiResponse::message("missing", StatusCode::MissParameters);
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(json, r#"{"message":"missing","data":null,"code":"0010"}"#);
    }

    #[test]
    fn success_carries_payload_in_data() {
        let body = ApiResponse::success(serde_json::json!([{"uid": 1}]));
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["code"], "0000");
        assert_eq!(json["data"][0]["uid"], 1);
    }
}
