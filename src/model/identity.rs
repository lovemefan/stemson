//! User identities, mirrors `backend/model/UserIdentity.py`.
//!
//! ```text
//! 1 = SUPER_ADMIN, 2 = ADMIN, 3 = USER
//! ```

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The identity of a user, used for authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UserIdentity {
    /// Can do everything.
    SuperAdmin = 1,
    /// Can manage users.
    Admin = 2,
    /// Regular user.
    User = 3,
}

impl UserIdentity {
    /// Numeric value stored in the database and the JWT.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Parse the numeric value.
    pub const fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(UserIdentity::SuperAdmin),
            2 => Some(UserIdentity::Admin),
            3 => Some(UserIdentity::User),
            _ => None,
        }
    }

    /// Uppercase name used in scopes and logs.
    pub const fn name(self) -> &'static str {
        match self {
            UserIdentity::SuperAdmin => "SUPER_ADMIN",
            UserIdentity::Admin => "ADMIN",
            UserIdentity::User => "USER",
        }
    }

    /// Whether this identity outranks `required`.
    pub const fn satisfies(self, required: UserIdentity) -> bool {
        (self as i32) <= (required as i32)
    }
}

impl Default for UserIdentity {
    /// Defaults to `USER`, like `User(identity=3)` in the Python model.
    fn default() -> Self {
        UserIdentity::User
    }
}

impl std::fmt::Display for UserIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl From<UserIdentity> for i32 {
    fn from(identity: UserIdentity) -> Self {
        identity.as_i32()
    }
}

impl Serialize for UserIdentity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i32(self.as_i32())
    }
}

impl<'de> Deserialize<'de> for UserIdentity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = i32::deserialize(deserializer)?;
        UserIdentity::from_i32(raw).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown user identity `{raw}` (expected 1, 2 or 3)"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_values_match_the_python_enum() {
        assert_eq!(UserIdentity::SuperAdmin.as_i32(), 1);
        assert_eq!(UserIdentity::Admin.as_i32(), 2);
        assert_eq!(UserIdentity::User.as_i32(), 3);
        assert_eq!(UserIdentity::default(), UserIdentity::User);
    }

    #[test]
    fn serde_uses_numbers() {
        assert_eq!(serde_json::to_string(&UserIdentity::Admin).unwrap(), "2");
        assert_eq!(
            serde_json::from_str::<UserIdentity>("3").unwrap(),
            UserIdentity::User
        );
        assert!(serde_json::from_str::<UserIdentity>("9").is_err());
    }

    #[test]
    fn identity_ordering_is_least_privileged_first() {
        assert!(UserIdentity::SuperAdmin.satisfies(UserIdentity::Admin));
        assert!(UserIdentity::Admin.satisfies(UserIdentity::Admin));
        assert!(!UserIdentity::User.satisfies(UserIdentity::Admin));
    }
}
