//! User data access, mirrors `backend/repository/UserRepository.py`.
//!
//! Every statement uses bound parameters (`?`). The Python implementation built
//! SQL with f-strings and filtered dangerous keywords afterwards; that is gone
//! (see `docs/MIGRATION.md`).

use std::sync::Arc;

use chrono::NaiveDateTime;
use sqlx::FromRow;

use crate::datasource::MysqlDataSource;
use crate::exception::{ApiError, ApiResult};

/// A user row joined with the name of its creator.
#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    /// Snowflake id.
    pub uid: i64,
    /// Login name.
    pub username: String,
    /// Phone number.
    pub phone: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// 0 = normal user, 1 = administrator.
    pub user_role: i64,
    /// Username of the administrator that created this user.
    pub create_by: Option<String>,
    /// Creation time.
    pub create_time: Option<NaiveDateTime>,
    /// Last login time.
    pub last_login_time: Option<NaiveDateTime>,
    /// 1 = enabled.
    pub status: i64,
    /// 1 = super admin, 2 = admin, 3 = user.
    pub identity: i64,
}

/// A user about to be inserted.
#[derive(Debug, Clone)]
pub struct NewUserRecord {
    /// Pre-generated snowflake id.
    pub uid: i64,
    /// Login name.
    pub username: String,
    /// Argon2id hash.
    pub password_hash: String,
    /// Phone number.
    pub phone: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// 0 = normal user, 1 = administrator.
    pub role: i32,
    /// Identity used for authorization.
    pub identity: i32,
    /// UID of the creating administrator.
    pub create_by: i64,
}

/// The schema uses `BIGINT UNSIGNED` / `TINYINT UNSIGNED` (see
/// `mysql/create_database.sql`); the casts keep the Rust side on signed
/// integers, which is what the rest of the application and the JWT use.
const SELECT_USER: &str = "\
SELECT CAST(u1.uid AS SIGNED) AS uid,
       u1.username,
       u1.phone,
       u1.email,
       CAST(u1.user_role AS SIGNED) AS user_role,
       u2.username AS create_by,
       u1.create_time,
       u1.last_login_time,
       CAST(u1.status AS SIGNED) AS status,
       CAST(u1.identity AS SIGNED) AS identity
FROM user AS u1
LEFT JOIN user AS u2 ON u1.create_by = u2.uid";

const INSERT_USER: &str = "\
INSERT INTO user (uid, username, password, phone, email, user_role, identity, create_by, status)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)";

/// Copy the creator's group membership to the new user, like
/// `add_user_into_group` in the Python repository.
const INSERT_GROUP: &str = "\
INSERT INTO group_user (gid, uid)
SELECT gid, ? FROM group_user WHERE uid = ?";

/// `@Repository`
#[stemson_macros::repository]
pub struct UserRepository {
    mysql: Arc<MysqlDataSource>,
}

impl UserRepository {
    /// All users, ordered by id.
    pub async fn list_all(&self) -> ApiResult<Vec<UserRow>> {
        let sql = format!("{SELECT_USER} ORDER BY u1.uid");
        self.mysql
            .fetch_all(sqlx::query_as::<_, UserRow>(&sql))
            .await
    }

    /// Look a user up by id.
    pub async fn find_by_uid(&self, uid: i64) -> ApiResult<Option<UserRow>> {
        let sql = format!("{SELECT_USER} WHERE u1.uid = ?");
        self.mysql
            .fetch_optional(sqlx::query_as::<_, UserRow>(&sql).bind(uid))
            .await
    }

    /// Look a user up by login name.
    pub async fn find_by_username(&self, username: &str) -> ApiResult<Option<UserRow>> {
        let sql = format!("{SELECT_USER} WHERE u1.username = ?");
        self.mysql
            .fetch_optional(sqlx::query_as::<_, UserRow>(&sql).bind(username))
            .await
    }

    /// Stored Argon2id hash for a login name.
    pub async fn password_hash(&self, username: &str) -> ApiResult<Option<String>> {
        #[derive(FromRow)]
        struct PasswordRow {
            password: String,
        }
        let row = self
            .mysql
            .fetch_optional(
                sqlx::query_as::<_, PasswordRow>("SELECT password FROM user WHERE username = ?")
                    .bind(username),
            )
            .await?;
        Ok(row.map(|row| row.password))
    }

    /// Insert a user and copy the creator's groups in one transaction.
    pub async fn insert_user_and_group(&self, record: &NewUserRecord) -> ApiResult<()> {
        let mut transaction = self.mysql.begin().await?;

        sqlx::query(INSERT_USER)
            .bind(record.uid)
            .bind(&record.username)
            .bind(&record.password_hash)
            .bind(&record.phone)
            .bind(&record.email)
            .bind(record.role)
            .bind(record.identity)
            .bind(record.create_by)
            .execute(&mut *transaction)
            .await
            .map_err(|err| map_unique_violation(err, &record.username))?;

        sqlx::query(INSERT_GROUP)
            .bind(record.uid)
            .bind(record.create_by)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(())
    }

    /// Update the mutable profile fields.
    pub async fn update_profile(
        &self,
        uid: i64,
        username: &str,
        phone: Option<&str>,
        email: Option<&str>,
        role: i32,
        status: i32,
    ) -> ApiResult<u64> {
        let result = sqlx::query(
            "UPDATE user SET username = ?, phone = ?, email = ?, user_role = ?, status = ? \
             WHERE uid = ?",
        )
        .bind(username)
        .bind(phone)
        .bind(email)
        .bind(role)
        .bind(status)
        .bind(uid)
        .execute(self.mysql.pool())
        .await
        .map_err(|err| map_unique_violation(err, username))?;
        Ok(result.rows_affected())
    }

    /// Replace the password hash.
    pub async fn update_password(&self, uid: i64, password_hash: &str) -> ApiResult<u64> {
        let result = sqlx::query("UPDATE user SET password = ? WHERE uid = ?")
            .bind(password_hash)
            .bind(uid)
            .execute(self.mysql.pool())
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete a user.
    pub async fn delete(&self, uid: i64) -> ApiResult<u64> {
        let result = sqlx::query("DELETE FROM user WHERE uid = ?")
            .bind(uid)
            .execute(self.mysql.pool())
            .await?;
        Ok(result.rows_affected())
    }

    /// Update `last_login_time`, like `UserRepository.login`.
    pub async fn touch_login(&self, uid: i64) -> ApiResult<()> {
        sqlx::query("UPDATE user SET last_login_time = NOW() WHERE uid = ?")
            .bind(uid)
            .execute(self.mysql.pool())
            .await?;
        Ok(())
    }
}

fn map_unique_violation(err: sqlx::Error, username: &str) -> ApiError {
    if let sqlx::Error::Database(database_error) = &err {
        if database_error.is_unique_violation() {
            return ApiError::user_already_exist(username);
        }
    }
    ApiError::from(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_use_bound_parameters_only() {
        // A regression guard for the f-string SQL of the Python version.
        for statement in [SELECT_USER, INSERT_USER, INSERT_GROUP] {
            assert!(
                !statement.contains('{') && !statement.contains('\''),
                "SQL must not interpolate values: {statement}"
            );
        }
        assert!(SELECT_USER.contains("u1.uid"));
    }
}
