//! Per-request context handed to controller methods.
//!
//! Plays the role of `sanic.Request` in the Python project, but pre-parses the
//! payload into the four parameter scopes used by the validators
//! (`json`, `form`, `args`, `files`), and carries the authenticated user.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{FromRequestParts, RawPathParams, Request};
use axum::http::{header, HeaderMap, Method, Uri};
use serde_json::{Map, Value};

use crate::auth::{AuthUser, Claims};
use crate::core::state::AppState;
use crate::exception::{ApiError, ApiResult};
use crate::model::identity::UserIdentity;

/// A file uploaded through `multipart/form-data`.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// Client supplied file name.
    pub filename: String,
    /// MIME type, when provided.
    pub content_type: Option<String>,
    /// Raw bytes.
    pub bytes: Bytes,
}

impl UploadedFile {
    /// Size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the file is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Which part of the request a validator looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamScope {
    /// form → query → files → json, json wins (same precedence as Python).
    All,
    /// JSON body.
    Json,
    /// `application/x-www-form-urlencoded` or multipart text fields.
    Form,
    /// Query string.
    Args,
    /// Matched path parameters.
    Path,
    /// Uploaded files.
    Files,
}

impl ParamScope {
    /// Lower case label used by generated code and logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            ParamScope::All => "all",
            ParamScope::Json => "json",
            ParamScope::Form => "form",
            ParamScope::Args => "args",
            ParamScope::Path => "path",
            ParamScope::Files => "files",
        }
    }
}

/// Everything a controller method needs about the current request.
pub struct RequestContext {
    /// Application state, for resolving services.
    pub state: AppState,
    /// HTTP method.
    pub method: Method,
    /// Request URI (path + query).
    pub uri: Uri,
    /// Request headers.
    pub headers: HeaderMap,
    /// Matched path parameters, e.g. `id` for `/items/{id}`.
    pub path_params: Map<String, Value>,
    /// Parsed query string.
    pub query: Map<String, Value>,
    /// Parsed JSON body.
    pub json: Option<Value>,
    /// Parsed form fields.
    pub form: Map<String, Value>,
    /// Uploaded files.
    pub files: HashMap<String, UploadedFile>,
    /// The authenticated caller, when a valid token was supplied.
    pub auth: Option<AuthUser>,
    /// Why authentication failed, when a token was supplied but rejected.
    auth_error: Option<ApiError>,
    /// Merged view used by `all` scoped validators.
    merged: Map<String, Value>,
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestContext")
            .field("method", &self.method)
            .field("uri", &self.uri)
            .field("json", &self.json)
            .field("form", &self.form)
            .field("query", &self.query)
            .field("files", &self.files.keys().collect::<Vec<_>>())
            .field("authenticated", &self.auth.is_some())
            .finish()
    }
}

impl RequestContext {
    /// Parse an Axum request into a context.
    pub async fn from_request(state: &AppState, request: Request) -> ApiResult<Self> {
        let (mut parts, body) = request.into_parts();

        // Axum stores the matched path parameters in the request extensions;
        // they are captured before the parts are consumed below.
        let path_params: Map<String, Value> =
            match RawPathParams::from_request_parts(&mut parts, state).await {
                Ok(params) => params
                    .iter()
                    .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
                    .collect(),
                Err(_) => Map::new(),
            };

        let max_body = state
            .config()
            .i64("server.http.request_max_size", 10 * 1024 * 1024)
            .max(1024) as usize;
        let bytes = axum::body::to_bytes(body, max_body).await.map_err(|err| {
            let message = err.to_string();
            if message.contains("length limit") {
                ApiError::invalid_parameter("Request body is too large")
                    .with_status(http::StatusCode::PAYLOAD_TOO_LARGE)
                    .with_detail(message)
            } else {
                ApiError::invalid_parameter("Failed to read request body").with_detail(message)
            }
        })?;

        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let mut json = None;
        let mut form = Map::new();
        let mut files = HashMap::new();

        if content_type.starts_with("application/json") && !bytes.is_empty() {
            let parsed: Value = serde_json::from_slice(&bytes)?;
            if !parsed.is_object() {
                return Err(ApiError::invalid_parameter("JSON body must be an object"));
            }
            json = Some(parsed);
        } else if content_type.starts_with("application/x-www-form-urlencoded") {
            form = parse_urlencoded(&bytes)?;
        } else if content_type.starts_with("multipart/form-data") {
            let (fields, uploaded) = parse_multipart(&content_type, bytes).await?;
            form = fields;
            files = uploaded;
        }

        let query = match parts.uri.query() {
            Some(raw) => parse_urlencoded(raw.as_bytes())?,
            None => Map::new(),
        };

        // Path parameters are added first, then the Python precedence:
        // form, args, files, json (json wins, so a body value overrides a path
        // parameter with the same name).
        let mut merged = Map::new();
        for (key, value) in &path_params {
            merged.insert(key.clone(), value.clone());
        }
        for (key, value) in form.iter().chain(query.iter()) {
            merged.insert(key.clone(), value.clone());
        }
        for (key, file) in &files {
            merged.insert(
                key.clone(),
                serde_json::json!({
                    "filename": file.filename,
                    "content_type": file.content_type,
                    "size": file.len(),
                }),
            );
        }
        if let Some(Value::Object(object)) = &json {
            for (key, value) in object {
                merged.insert(key.clone(), value.clone());
            }
        }

        let mut auth = None;
        let mut auth_error = None;
        if let Some(raw) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
        {
            match extract_bearer(raw) {
                Ok(token) => match state.ctx().jwt().verify(token) {
                    Ok(claims) => auth = Some(AuthUser::new(claims)),
                    Err(err) => auth_error = Some(err),
                },
                Err(err) => auth_error = Some(err),
            }
        }

        Ok(Self {
            state: state.clone(),
            method: parts.method,
            uri: parts.uri,
            headers: parts.headers,
            path_params,
            query,
            json,
            form,
            files,
            auth,
            auth_error,
            merged,
        })
    }

    /// Start validating parameters in a scope.
    pub fn validator(&self, scope: ParamScope) -> crate::core::decorator::validate::Validator<'_> {
        crate::core::decorator::validate::Validator::new(self, scope)
    }

    /// Borrow the application context.
    pub fn ctx(&self) -> &std::sync::Arc<crate::core::context::AppContext> {
        self.state.ctx()
    }

    /// Look up a parameter in a scope.
    pub fn param(&self, key: &str, scope: ParamScope) -> Option<&Value> {
        match scope {
            ParamScope::All => self.merged.get(key),
            ParamScope::Json => self.json.as_ref().and_then(|v| v.get(key)),
            ParamScope::Form => self.form.get(key),
            ParamScope::Args => self.query.get(key),
            ParamScope::Path => self.path_params.get(key),
            ParamScope::Files => {
                if self.files.contains_key(key) {
                    self.merged.get(key)
                } else {
                    None
                }
            }
        }
    }

    /// Look up a matched path parameter as text.
    pub fn path_param(&self, key: &str) -> Option<&str> {
        self.path_params.get(key).and_then(|value| value.as_str())
    }

    /// Look up an uploaded file.
    pub fn file(&self, key: &str) -> Option<&UploadedFile> {
        self.files.get(key)
    }

    /// Look up a text parameter across every scope (json wins).
    pub fn text(&self, key: &str) -> Option<String> {
        self.merged.get(key).and_then(value_to_text)
    }

    /// Deserialize the JSON body into `T`.
    pub fn json_as<T: serde::de::DeserializeOwned>(&self) -> ApiResult<T> {
        let value = self.json.clone().ok_or_else(|| {
            ApiError::miss_parameters(
                "Please make sure your header has `Content-Type: application/json`",
            )
        })?;
        serde_json::from_value(value).map_err(|err| {
            ApiError::invalid_parameter("Invalid JSON body").with_detail(err.to_string())
        })
    }

    /// Whether the JSON body was parsed.
    pub fn has_json(&self) -> bool {
        self.json.is_some()
    }

    /// Borrow the authenticated caller, or explain why it is missing.
    pub fn require_auth(&self) -> ApiResult<&AuthUser> {
        if let Some(auth) = &self.auth {
            return Ok(auth);
        }
        if let Some(err) = &self.auth_error {
            return Err(err.clone());
        }
        Err(ApiError::unauthorized("Authorization header not present."))
    }

    /// Shortcut for the claims of the authenticated caller.
    pub fn require_user(&self) -> ApiResult<&Claims> {
        Ok(&self.require_auth()?.claims)
    }

    /// Require one of the given identities.
    pub fn require_identity(&self, allowed: &[UserIdentity]) -> ApiResult<&AuthUser> {
        let auth = self.require_auth()?;
        let identity = auth
            .identity()
            .ok_or_else(|| ApiError::unauthorized("Invalid user identity."))?;
        if allowed.iter().any(|required| identity.satisfies(*required)) {
            Ok(auth)
        } else {
            Err(ApiError::forbidden(format!(
                "Invalid user identity: {identity} is not allowed here"
            )))
        }
    }

    /// Require an administrator (`role != 0`), like `if not user.role` in Python.
    pub fn require_admin(&self, action: &str) -> ApiResult<&AuthUser> {
        let auth = self.require_auth()?;
        if auth.is_admin() {
            Ok(auth)
        } else {
            Err(ApiError::forbidden(format!(
                "You have no authorized to {action}"
            )))
        }
    }
}

/// Extract the token from `Authorization: Bearer <token>`.
pub fn extract_bearer(raw: &str) -> ApiResult<&str> {
    let mut parts = raw.splitn(2, ' ');
    let scheme = parts.next().unwrap_or_default();
    let token = parts.next().unwrap_or_default().trim();
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(ApiError::unauthorized(
            "Authorization header must use the Bearer scheme.",
        ));
    }
    if token.is_empty() {
        return Err(ApiError::unauthorized("Authorization token is empty."));
    }
    Ok(token)
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn parse_urlencoded(raw: &[u8]) -> ApiResult<Map<String, Value>> {
    let parsed: Vec<(String, String)> = serde_urlencoded::from_bytes(raw).map_err(|err| {
        ApiError::invalid_parameter("Malformed form/query parameters").with_detail(err.to_string())
    })?;
    let mut map = Map::new();
    for (key, value) in parsed {
        map.insert(key, Value::String(value));
    }
    Ok(map)
}

async fn parse_multipart(
    content_type: &str,
    bytes: Bytes,
) -> ApiResult<(Map<String, Value>, HashMap<String, UploadedFile>)> {
    let boundary = content_type
        .split(';')
        .find_map(|part| {
            let part = part.trim();
            part.strip_prefix("boundary=")
                .map(|value| value.trim_matches('"').to_string())
        })
        .filter(|boundary| !boundary.is_empty())
        .ok_or_else(|| {
            ApiError::invalid_parameter("multipart/form-data is missing its boundary")
        })?;

    let stream = futures_util::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
    let mut multipart = multer::Multipart::new(stream, boundary);

    let mut fields = Map::new();
    let mut files = HashMap::new();
    while let Some(field) = multipart.next_field().await.map_err(|err| {
        ApiError::invalid_parameter("Malformed multipart body").with_detail(err.to_string())
    })? {
        let name = field.name().unwrap_or_default().to_string();
        if name.is_empty() {
            continue;
        }
        let filename = field.file_name().map(str::to_string);
        let file_type = field.content_type().map(|value| value.to_string());
        let data = field.bytes().await.map_err(|err| {
            ApiError::invalid_parameter("Malformed multipart body").with_detail(err.to_string())
        })?;
        match filename {
            Some(filename) => {
                files.insert(
                    name,
                    UploadedFile {
                        filename,
                        content_type: file_type,
                        bytes: data,
                    },
                );
            }
            None => {
                fields.insert(
                    name,
                    Value::String(String::from_utf8_lossy(&data).into_owned()),
                );
            }
        }
    }
    Ok((fields, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_tokens_are_parsed_case_insensitively() {
        assert_eq!(extract_bearer("Bearer abc").unwrap(), "abc");
        assert_eq!(extract_bearer("bearer abc").unwrap(), "abc");
        assert!(extract_bearer("Basic abc").is_err());
        assert!(extract_bearer("Bearer ").is_err());
    }

    #[test]
    fn urlencoded_parsing_keeps_last_value() {
        let map = parse_urlencoded(b"a=1&b=x&a=2").unwrap();
        assert_eq!(map.get("a").unwrap(), "2");
        assert_eq!(map.get("b").unwrap(), "x");
    }
}

#[cfg(test)]
mod path_param_tests {
    use super::*;
    use crate::test_support::TestContext;
    use tower::ServiceExt;

    async fn echo(
        axum::extract::State(state): axum::extract::State<crate::core::state::AppState>,
        request: Request,
    ) -> String {
        let context = RequestContext::from_request(&state, request)
            .await
            .expect("context");
        format!(
            "{}:{}:{}",
            context.path_param("id").unwrap_or("?"),
            context.text("id").unwrap_or_default(),
            context.param("id", ParamScope::Path).is_some()
        )
    }

    #[test]
    fn path_parameters_are_extracted() {
        let fixture = TestContext::new();
        let router = axum::Router::new()
            .route("/items/{id}", axum::routing::get(echo))
            .with_state(fixture.state());
        let request = axum::http::Request::builder()
            .uri("/items/42")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = crate::test_support::block_on(router.oneshot(request)).unwrap();
        let body = crate::test_support::block_on(async {
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
        });
        assert_eq!(String::from_utf8_lossy(&body), "42:42:true");
    }
}
