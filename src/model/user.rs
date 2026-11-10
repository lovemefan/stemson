//! User model, mirrors `backend/model/User.py`.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::Serialize;

use crate::model::identity::UserIdentity;

/// A user row.
#[derive(Debug, Clone, Default)]
pub struct User {
    /// Snowflake primary key.
    pub uid: i64,
    /// Login name (unique).
    pub username: String,
    /// Phone number.
    pub phone: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Argon2id password hash. Never serialized.
    pub password: Option<String>,
    /// 0 = normal user, 1 = administrator.
    pub role: i32,
    /// Identity used for authorization.
    pub user_identity: UserIdentity,
    /// Username of the administrator who created this user.
    pub create_by: Option<String>,
    /// Creation timestamp.
    pub create_time: Option<DateTime<Local>>,
    /// Last successful login.
    pub last_login_time: Option<DateTime<Local>>,
    /// 1 = enabled, 0 = disabled.
    pub status: i32,
}

impl User {
    /// Public representation, mirrors `User.to_dict()`.
    ///
    /// The password hash is never exposed.
    pub fn to_dict(&self) -> serde_json::Value {
        #[derive(Serialize)]
        struct UserView<'a> {
            user_id: i64,
            username: &'a str,
            phone: &'a Option<String>,
            email: &'a Option<String>,
            role: i32,
            create_by: &'a Option<String>,
            create_time: Option<String>,
            last_login_time: Option<String>,
            status: i32,
        }

        serde_json::to_value(UserView {
            user_id: self.uid,
            username: &self.username,
            phone: &self.phone,
            email: &self.email,
            role: self.role,
            create_by: &self.create_by,
            create_time: self.create_time.map(|t| format_timestamp(&t)),
            last_login_time: self.last_login_time.map(|t| format_timestamp(&t)),
            status: self.status,
        })
        .unwrap_or(serde_json::Value::Null)
    }
}

/// Format like the Python model: `%Y-%m-%d %H:%M:%S`.
pub fn format_timestamp(value: &DateTime<Local>) -> String {
    value.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Convert a naive MySQL `DATETIME` to local time the same way the Python
/// driver did (naive values are interpreted as local time).
pub fn naive_to_local(value: NaiveDateTime) -> DateTime<Local> {
    Local
        .from_local_datetime(&value)
        .single()
        .unwrap_or_else(|| Local.from_utc_datetime(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_dict_never_leaks_the_password() {
        let user = User {
            uid: 1342014866998829056,
            username: "lovemefan".into(),
            password: Some("$argon2id$secret".into()),
            role: 1,
            email: Some("lovemefan@outlook.com".into()),
            status: 1,
            ..Default::default()
        };
        let json = user.to_dict();
        assert_eq!(json["user_id"], 1342014866998829056_i64);
        assert_eq!(json["username"], "lovemefan");
        assert_eq!(json["role"], 1);
        assert!(json.get("password").is_none());
    }

    #[test]
    fn naive_datetimes_are_rendered_like_python() {
        let naive =
            NaiveDateTime::parse_from_str("2020-12-24 13:37:38", "%Y-%m-%d %H:%M:%S").unwrap();
        let formatted = format_timestamp(&naive_to_local(naive));
        assert_eq!(formatted, "2020-12-24 13:37:38");
    }
}
