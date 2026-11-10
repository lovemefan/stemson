//! Parameter validation, the Rust port of
//! `backend/core/decorator/validateParameters.py`.
//!
//! The Python project used decorators:
//!
//! ```python
//! @NotEmpty(required=["username", "password"], parameter_type="json")
//! @Length("password", min=5, max=20, message="...", parameter_type="json")
//! async def post(self, request): ...
//! ```
//!
//! In Rust the `#[controller]` macro turns the declarative `#[validate(...)]`
//! attribute into code equivalent to:
//!
//! ```ignore
//! let mut validator = ctx.validator(ParamScope::Json);
//! validator.not_empty(&["username", "password"]);
//! validator.length("password", 5, 20, "...");
//! validator.check()?;
//! ```
//!
//! Validators are order preserving and stop at the first failure, exactly like
//! the Python decorator chain.

use serde_json::Value;

use crate::exception::{ApiError, ApiResult};
use crate::request::context::{ParamScope, RequestContext, UploadedFile};

/// How `pattern` matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternMode {
    /// Anchored at the start of the value, like Python's `re.match`.
    Match,
    /// Anywhere in the value, like Python's `re.search`.
    Search,
}

/// Collects validation failures for one request.
pub struct Validator<'a> {
    ctx: &'a RequestContext,
    scope: ParamScope,
    error: Option<ApiError>,
}

impl<'a> Validator<'a> {
    /// Create a validator bound to a parameter scope.
    pub fn new(ctx: &'a RequestContext, scope: ParamScope) -> Self {
        Self {
            ctx,
            scope,
            error: None,
        }
    }

    fn fail(&mut self, error: ApiError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn failed(&self) -> bool {
        self.error.is_some()
    }

    /// `@NotEmpty`: every key must be present and not null.
    pub fn not_empty(&mut self, keys: &[&str]) -> &mut Self {
        if self.failed() {
            return self;
        }
        if self.scope == ParamScope::Json && self.ctx.json.is_none() && !keys.is_empty() {
            self.fail(ApiError::miss_parameters(
                "Please make sure your header has `Content-Type: application/json`",
            ));
            return self;
        }
        for key in keys {
            let present = self
                .ctx
                .param(key, self.scope)
                .map(|value| !value.is_null())
                .unwrap_or(false);
            if !present {
                self.fail(ApiError::miss_parameters(format!(
                    "Missing partial parameter, {key} is empty"
                )));
                return self;
            }
        }
        self
    }

    /// `@Length`: string length (in characters) must be within `min..=max`.
    pub fn length(&mut self, key: &str, min: usize, max: usize, message: &str) -> &mut Self {
        if self.failed() {
            return self;
        }
        let Some(value) = self.ctx.param(key, self.scope) else {
            self.fail(ApiError::invalid_parameter(format!(
                "Missing partial parameter, {key} is empty"
            )));
            return self;
        };
        let length = match value {
            Value::String(text) => text.chars().count(),
            Value::Array(items) => items.len(),
            _ => {
                self.fail(ApiError::invalid_parameter(format!(
                    "The data {key} should be a string"
                )));
                return self;
            }
        };
        if length < min || length > max {
            self.fail(ApiError::invalid_parameter(text_or_default(
                message, key, "length",
            )));
        }
        self
    }

    /// `@Range`: numeric value must be within `min..=max`.
    pub fn range(&mut self, key: &str, min: f64, max: f64, message: &str) -> &mut Self {
        if self.failed() {
            return self;
        }
        let Some(value) = self.ctx.param(key, self.scope) else {
            self.fail(ApiError::invalid_parameter(format!(
                "Missing partial parameter, {key} is empty"
            )));
            return self;
        };
        let number = match value {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().parse::<f64>().ok(),
            _ => None,
        };
        let Some(number) = number else {
            self.fail(ApiError::invalid_parameter(format!(
                "could not convert {key} to number"
            )));
            return self;
        };
        if number < min || number > max {
            self.fail(ApiError::invalid_parameter(text_or_default(
                message, key, "range",
            )));
        }
        self
    }

    /// `@Pattern`: regular expression check.
    pub fn pattern(
        &mut self,
        key: &str,
        pattern: &str,
        mode: PatternMode,
        message: &str,
    ) -> &mut Self {
        if self.failed() {
            return self;
        }
        let Some(value) = self.ctx.param(key, self.scope) else {
            self.fail(ApiError::invalid_parameter(format!(
                "Missing partial parameter, {key} is empty"
            )));
            return self;
        };
        let Some(text) = value_as_text(value) else {
            self.fail(ApiError::invalid_parameter(format!(
                "The data {key} should be a string"
            )));
            return self;
        };
        let regex = match regex::Regex::new(pattern) {
            Ok(regex) => regex,
            Err(err) => {
                // A broken pattern is a programming error, not a client error.
                self.fail(
                    ApiError::internal(format!("invalid validation pattern for {key}: {err}"))
                        .with_detail(pattern.to_string()),
                );
                return self;
            }
        };
        let matched = match mode {
            PatternMode::Match => regex
                .find(&text)
                .map(|found| found.start() == 0)
                .unwrap_or(false),
            PatternMode::Search => regex.is_match(&text),
        };
        if !matched {
            self.fail(ApiError::invalid_parameter(text_or_default(
                message, key, "format",
            )));
        }
        self
    }

    /// `@EnumString`: value must be one of `values`.
    pub fn enum_string(&mut self, key: &str, values: &[&str], message: &str) -> &mut Self {
        if self.failed() {
            return self;
        }
        let Some(value) = self.ctx.param(key, self.scope) else {
            self.fail(ApiError::invalid_parameter(format!(
                "Missing partial parameter, {key} is empty"
            )));
            return self;
        };
        let matched = match value {
            Value::String(text) => values.contains(&text.as_str()),
            other => values.contains(&other.to_string().as_str()),
        };
        if !matched {
            self.fail(ApiError::invalid_parameter(text_or_default(
                message, key, "value",
            )));
        }
        self
    }

    /// `@Assert`: arbitrary predicate over the raw JSON value.
    pub fn assert(&mut self, key: &str, predicate: fn(&Value) -> bool, message: &str) -> &mut Self {
        if self.failed() {
            return self;
        }
        let Some(value) = self.ctx.param(key, self.scope) else {
            self.fail(ApiError::invalid_parameter(format!(
                "Missing partial parameter, {key} is empty"
            )));
            return self;
        };
        if !predicate(value) {
            self.fail(ApiError::invalid_parameter(text_or_default(
                message, key, "value",
            )));
        }
        self
    }

    /// `@Assert` for uploaded files (the Python version duck-typed `len(file.body)`).
    pub fn assert_file(
        &mut self,
        key: &str,
        predicate: fn(&UploadedFile) -> bool,
        message: &str,
    ) -> &mut Self {
        if self.failed() {
            return self;
        }
        let Some(file) = self.ctx.file(key) else {
            self.fail(ApiError::miss_parameters(format!(
                "Missing partial parameter, {key} is empty"
            )));
            return self;
        };
        if !predicate(file) {
            self.fail(ApiError::invalid_parameter(text_or_default(
                message, key, "file",
            )));
        }
        self
    }

    /// Return the first failure, if any.
    pub fn check(self) -> ApiResult<()> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn text_or_default(message: &str, key: &str, kind: &str) -> String {
    if message.is_empty() {
        format!("invalid {kind} for parameter `{key}`")
    } else {
        message.to_string()
    }
}

fn value_as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestContext;

    fn with_json(payload: Value) -> (TestContext, RequestContext) {
        let ctx = TestContext::new();
        let request = ctx.json_request(&payload);
        (ctx, request)
    }

    #[test]
    fn not_empty_reports_missing_parameters() {
        let (_ctx, request) = with_json(serde_json::json!({"username": "alice"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.not_empty(&["username", "password"]);
        let err = validator.check().unwrap_err();
        assert_eq!(
            err.code,
            crate::model::status_code::StatusCode::MissParameters
        );
        assert!(err.message.contains("password"));
    }

    #[test]
    fn not_empty_reports_a_missing_json_content_type() {
        let ctx = TestContext::new();
        let request = ctx.empty_request();
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.not_empty(&["username"]);
        let err = validator.check().unwrap_err();
        assert!(err.message.contains("Content-Type: application/json"));
    }

    #[test]
    fn length_uses_character_count() {
        let (_ctx, request) = with_json(serde_json::json!({"password": "密码密码"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.length("password", 3, 20, "bad length");
        assert!(validator.check().is_ok());

        let (_ctx, request) = with_json(serde_json::json!({"password": "1234"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.length("password", 5, 20, "Length of password must be 5 to 20");
        let err = validator.check().unwrap_err();
        assert_eq!(err.message, "Length of password must be 5 to 20");
    }

    #[test]
    fn range_accepts_numeric_strings_and_rejects_outliers() {
        let (_ctx, request) = with_json(serde_json::json!({"age": "18"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.range("age", 18.0, 80.0, "Age must in range 18 to 80");
        assert!(validator.check().is_ok());

        let (_ctx, request) = with_json(serde_json::json!({"age": 81}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.range("age", 18.0, 80.0, "Age must in range 18 to 80");
        assert!(validator.check().is_err());
    }

    #[test]
    fn pattern_supports_match_and_search() {
        let (_ctx, request) = with_json(serde_json::json!({"email": "a@b.co"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.pattern(
            "email",
            r"^[^@]+@[^@]+\.[a-z]{2,}$",
            PatternMode::Match,
            "bad",
        );
        assert!(validator.check().is_ok());

        let (_ctx, request) = with_json(serde_json::json!({"name": "xx-tom-xx"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.pattern("name", "tom", PatternMode::Match, "must start with tom");
        assert!(validator.check().is_err());

        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.pattern("name", "tom", PatternMode::Search, "must contain tom");
        assert!(validator.check().is_ok());
    }

    #[test]
    fn enum_string_checks_membership() {
        let (_ctx, request) = with_json(serde_json::json!({"gender": "male"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.enum_string("gender", &["male", "female"], "bad gender");
        assert!(validator.check().is_ok());

        let (_ctx, request) = with_json(serde_json::json!({"gender": "other"}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.enum_string("gender", &["male", "female"], "gender must male or female");
        let err = validator.check().unwrap_err();
        assert_eq!(err.message, "gender must male or female");
    }

    #[test]
    fn assert_runs_arbitrary_predicates() {
        fn is_odd(value: &Value) -> bool {
            value.as_i64().map(|n| n % 2 != 0).unwrap_or(false)
        }
        let (_ctx, request) = with_json(serde_json::json!({"number": 3}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.assert("number", is_odd, "The number is not a odd");
        assert!(validator.check().is_ok());

        let (_ctx, request) = with_json(serde_json::json!({"number": 4}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.assert("number", is_odd, "The number is not a odd");
        assert!(validator.check().is_err());
    }

    #[test]
    fn first_failure_wins() {
        let (_ctx, request) = with_json(serde_json::json!({}));
        let mut validator = Validator::new(&request, ParamScope::Json);
        validator.not_empty(&["username"]);
        validator.not_empty(&["password"]);
        let err = validator.check().unwrap_err();
        assert!(err.message.contains("username"));
    }
}
