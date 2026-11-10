//! Configuration injection, mirrors `backend/core/component/value.py`.
//!
//! Field injection is the preferred form:
//!
//! ```ignore
//! #[derive(Autowired)]
//! struct Mailer {
//!     #[value("smtp.port")]
//!     port: u16,
//! }
//! ```
//!
//! [`Value::get`] is the imperative equivalent for code that has no
//! `AppContext` field, and behaves like the Python `@Value` decorator: a
//! missing key is an error at startup.

use serde::de::DeserializeOwned;

use crate::core::context::AppContext;

/// Read a configuration value by dotted key.
pub struct Value;

impl Value {
    /// Read `key` and deserialize it into `T`.
    ///
    /// # Errors
    ///
    /// Fails when the key is missing or its value has the wrong type, exactly
    /// like the Python decorator raising `ValueError(f"{self.key} is invalid")`.
    pub fn get<T: DeserializeOwned>(ctx: &AppContext, key: &str) -> anyhow::Result<T> {
        ctx.inject_value::<T>(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestContext;

    #[test]
    fn reads_typed_values() {
        let context = TestContext::new();
        let port: u16 = Value::get(context.ctx().as_ref(), "server.http.request_timeout")
            .expect("timeout is configured");
        assert_eq!(port, 30);

        let secret: String =
            Value::get(context.ctx().as_ref(), "server.jwt.secret").expect("secret is configured");
        assert_eq!(secret, "test-secret");
    }

    #[test]
    fn missing_keys_are_errors() {
        let context = TestContext::new();
        assert!(Value::get::<String>(context.ctx().as_ref(), "nope.nothing").is_err());
    }
}
