//! User data access, mirrors `backend/repository/UserRepository.py`.
//!
//! Every statement uses bound parameters (`?` for MySQL/SQLite, `$1` for PostgreSQL).
//! The Python implementation built SQL with f-strings and filtered dangerous
//! keywords afterwards; that is gone (see `docs/MIGRATION.md`).

use std::sync::{Arc, OnceLock};

use chrono::NaiveDateTime;
use sqlx::FromRow;

use crate::config::Config;
use crate::datasource::{MysqlDataSource, PostgresDataSource, SqliteDataSource};
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

/// Stored credentials used by the login endpoints.
#[derive(Debug, Clone, Default)]
pub struct LoginMaterial {
    /// Argon2id hash, when the account still uses password login.
    pub password: Option<String>,
    /// Serialized OPAQUE registration record.
    pub opaque_registration: Option<Vec<u8>>,
}

/// A user about to be inserted.
#[derive(Debug, Clone)]
pub struct NewUserRecord {
    /// Pre-generated snowflake id.
    pub uid: i64,
    /// Login name.
    pub username: String,
    /// Argon2id hash; `None` for OPAQUE accounts.
    pub password_hash: Option<String>,
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

/// PostgreSQL has signed integers and reserves the table name `user`.
const SELECT_USER_POSTGRES: &str = "\
SELECT u1.uid, u1.username, u1.phone, u1.email,
       CAST(u1.user_role AS BIGINT) AS user_role,
       u2.username AS create_by, u1.create_time, u1.last_login_time,
       CAST(u1.status AS BIGINT) AS status,
       CAST(u1.identity AS BIGINT) AS identity
FROM \"user\" AS u1
LEFT JOIN \"user\" AS u2 ON u1.create_by = u2.uid";

const INSERT_USER_POSTGRES: &str = "\
INSERT INTO \"user\" (uid, username, password, phone, email, user_role, identity, create_by, status)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1)";

const INSERT_GROUP_POSTGRES: &str = "\
INSERT INTO group_user (gid, uid)
SELECT gid, $1::BIGINT FROM group_user WHERE uid = $2";

const SELECT_USER_SQLITE: &str = "\
SELECT u1.uid, u1.username, u1.phone, u1.email, u1.user_role,
       u2.username AS create_by, u1.create_time, u1.last_login_time,
       u1.status, u1.identity
FROM user AS u1
LEFT JOIN user AS u2 ON u1.create_by = u2.uid";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    Mysql,
    Postgres,
    Sqlite,
}

impl Dialect {
    fn parse(config: &Config) -> ApiResult<Self> {
        match config
            .string("datasource.driver", "mysql")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "mysql" => Ok(Self::Mysql),
            "postgres" | "postgresql" | "pgsql" => Ok(Self::Postgres),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(ApiError::internal(format!(
                "unsupported datasource.driver `{other}`; expected mysql, postgres or sqlite"
            ))),
        }
    }

    fn sql<'a>(self, mysql: &'a str, postgres: &'a str, sqlite: &'a str) -> &'a str {
        match self {
            Self::Mysql => mysql,
            Self::Postgres => postgres,
            Self::Sqlite => sqlite,
        }
    }
}

// Expand the bound query for each native SQLx driver. This keeps SQLx's typed
// decoding (including timestamps and BYTEA) and one implementation of each
// repository operation. A transaction always stays on the selected backend.
macro_rules! with_database {
    ($repository:expr, |$db:ident, $dialect:ident| $body:block) => {{
        let $dialect = $repository.dialect()?;
        match $dialect {
            Dialect::Mysql => {
                let $db = &$repository.mysql;
                $body
            }
            Dialect::Postgres => {
                let $db = &$repository.postgres;
                $body
            }
            Dialect::Sqlite => {
                let $db = &$repository.sqlite;
                $body
            }
        }
    }};
}

/// User operations use `datasource.driver` (MySQL by default).
#[stemson_macros::repository]
pub struct UserRepository {
    config: Arc<Config>,
    mysql: Arc<MysqlDataSource>,
    postgres: Arc<PostgresDataSource>,
    sqlite: Arc<SqliteDataSource>,
    #[skip]
    dialect: OnceLock<ApiResult<Dialect>>,
}

impl UserRepository {
    fn dialect(&self) -> ApiResult<Dialect> {
        self.dialect
            .get_or_init(|| Dialect::parse(&self.config))
            .clone()
    }

    /// All users, ordered by id.
    pub async fn list_all(&self) -> ApiResult<Vec<UserRow>> {
        with_database!(self, |db, dialect| {
            let select = dialect.sql(SELECT_USER, SELECT_USER_POSTGRES, SELECT_USER_SQLITE);
            let sql = format!("{select} ORDER BY u1.uid");
            db.fetch_all(sqlx::query_as::<_, UserRow>(&sql)).await
        })
    }

    /// Look a user up by id.
    pub async fn find_by_uid(&self, uid: i64) -> ApiResult<Option<UserRow>> {
        with_database!(self, |db, dialect| {
            let select = dialect.sql(SELECT_USER, SELECT_USER_POSTGRES, SELECT_USER_SQLITE);
            let filter = dialect.sql(
                " WHERE u1.uid = ?",
                " WHERE u1.uid = $1",
                " WHERE u1.uid = ?",
            );
            let sql = format!("{select}{filter}");
            db.fetch_optional(sqlx::query_as::<_, UserRow>(&sql).bind(uid))
                .await
        })
    }

    /// Look a user up by login name.
    pub async fn find_by_username(&self, username: &str) -> ApiResult<Option<UserRow>> {
        with_database!(self, |db, dialect| {
            let select = dialect.sql(SELECT_USER, SELECT_USER_POSTGRES, SELECT_USER_SQLITE);
            let filter = dialect.sql(
                " WHERE u1.username = ?",
                " WHERE u1.username = $1",
                " WHERE u1.username = ?",
            );
            let sql = format!("{select}{filter}");
            db.fetch_optional(sqlx::query_as::<_, UserRow>(&sql).bind(username))
                .await
        })
    }

    /// Stored credentials for a login name.
    pub async fn login_material(&self, username: &str) -> ApiResult<Option<LoginMaterial>> {
        #[derive(FromRow)]
        struct MaterialRow {
            password: Option<String>,
            opaque_registration: Option<Vec<u8>>,
        }
        let row = with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "SELECT password, opaque_registration FROM user WHERE username = ?",
                "SELECT password, opaque_registration FROM \"user\" WHERE username = $1",
                "SELECT password, opaque_registration FROM user WHERE username = ?",
            );
            db.fetch_optional(sqlx::query_as::<_, MaterialRow>(sql).bind(username))
                .await
        })?;
        Ok(row.map(|row| LoginMaterial {
            password: row.password,
            opaque_registration: row.opaque_registration,
        }))
    }

    /// Store a one-time registration invitation for an OPAQUE account.
    pub async fn set_invite(
        &self,
        uid: i64,
        token_hash: &str,
        expires_at: NaiveDateTime,
    ) -> ApiResult<u64> {
        with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "UPDATE user SET invite_token_hash = ?, invite_expires_at = ? WHERE uid = ?",
                "UPDATE \"user\" SET invite_token_hash = $1, invite_expires_at = $2 WHERE uid = $3",
                "UPDATE user SET invite_token_hash = ?, invite_expires_at = ? WHERE uid = ?",
            );
            let result = db
                .execute(sqlx::query(sql).bind(token_hash).bind(expires_at).bind(uid))
                .await?;
            Ok(result.rows_affected())
        })
    }

    /// Atomically consume an unexpired invitation and store the OPAQUE record.
    pub async fn consume_invite(
        &self,
        username: &str,
        token_hash: &str,
        registration: &[u8],
    ) -> ApiResult<u64> {
        with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "UPDATE user SET opaque_registration = ?, password = NULL, \
                 invite_token_hash = NULL, invite_expires_at = NULL \
                 WHERE username = ? AND invite_token_hash = ? AND invite_expires_at > NOW()",
                "UPDATE \"user\" SET opaque_registration = $1, password = NULL, \
                 invite_token_hash = NULL, invite_expires_at = NULL \
                 WHERE username = $2 AND invite_token_hash = $3 AND invite_expires_at > NOW()",
                "UPDATE user SET opaque_registration = ?, password = NULL, \
                 invite_token_hash = NULL, invite_expires_at = NULL \
                 WHERE username = ? AND invite_token_hash = ? \
                 AND julianday(invite_expires_at) > julianday('now')",
            );
            let result = db
                .execute(
                    sqlx::query(sql)
                        .bind(registration)
                        .bind(username)
                        .bind(token_hash),
                )
                .await?;
            Ok(result.rows_affected())
        })
    }

    /// Insert a user and copy the creator's groups in one transaction.
    pub async fn insert_user_and_group(&self, record: &NewUserRecord) -> ApiResult<()> {
        with_database!(self, |db, dialect| {
            let mut transaction = db.begin().await?;
            sqlx::query(dialect.sql(INSERT_USER, INSERT_USER_POSTGRES, INSERT_USER))
                .bind(record.uid)
                .bind(&record.username)
                .bind(record.password_hash.as_deref())
                .bind(&record.phone)
                .bind(&record.email)
                .bind(record.role)
                .bind(record.identity)
                .bind(record.create_by)
                .execute(&mut *transaction)
                .await
                .map_err(|err| map_unique_violation(err, &record.username))?;
            sqlx::query(dialect.sql(INSERT_GROUP, INSERT_GROUP_POSTGRES, INSERT_GROUP))
                .bind(record.uid)
                .bind(record.create_by)
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            Ok(())
        })
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
        with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "UPDATE user SET username = ?, phone = ?, email = ?, user_role = ?, status = ? WHERE uid = ?",
                "UPDATE \"user\" SET username = $1, phone = $2, email = $3, user_role = $4, status = $5 WHERE uid = $6",
                "UPDATE user SET username = ?, phone = ?, email = ?, user_role = ?, status = ? WHERE uid = ?",
            );
            let result = sqlx::query(sql)
                .bind(username)
                .bind(phone)
                .bind(email)
                .bind(role)
                .bind(status)
                .bind(uid)
                .execute(db.pool())
                .await
                .map_err(|err| map_unique_violation(err, username))?;
            Ok(result.rows_affected())
        })
    }

    /// Replace the password hash.
    pub async fn update_password(&self, uid: i64, password_hash: &str) -> ApiResult<u64> {
        with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "UPDATE user SET password = ? WHERE uid = ?",
                "UPDATE \"user\" SET password = $1 WHERE uid = $2",
                "UPDATE user SET password = ? WHERE uid = ?",
            );
            let result = db
                .execute(sqlx::query(sql).bind(password_hash).bind(uid))
                .await?;
            Ok(result.rows_affected())
        })
    }

    /// Delete a user.
    pub async fn delete(&self, uid: i64) -> ApiResult<u64> {
        with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "DELETE FROM user WHERE uid = ?",
                "DELETE FROM \"user\" WHERE uid = $1",
                "DELETE FROM user WHERE uid = ?",
            );
            let result = db.execute(sqlx::query(sql).bind(uid)).await?;
            Ok(result.rows_affected())
        })
    }

    /// Update `last_login_time`.
    pub async fn touch_login(&self, uid: i64) -> ApiResult<()> {
        with_database!(self, |db, dialect| {
            let sql = dialect.sql(
                "UPDATE user SET last_login_time = NOW() WHERE uid = ?",
                "UPDATE \"user\" SET last_login_time = NOW() WHERE uid = $1",
                "UPDATE user SET last_login_time = CURRENT_TIMESTAMP WHERE uid = ?",
            );
            db.execute(sqlx::query(sql).bind(uid)).await?;
            Ok(())
        })
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
    use crate::test_support::TestContext;

    #[test]
    fn driver_defaults_to_mysql_and_accepts_postgres_aliases() {
        let fixture = TestContext::with_config("{}");
        assert_eq!(
            Dialect::parse(fixture.ctx().config()).unwrap(),
            Dialect::Mysql
        );
        for driver in ["postgres", "postgresql", "pgsql", "PostgreSQL"] {
            let fixture = TestContext::with_config(&format!("datasource:\n  driver: {driver}\n"));
            assert_eq!(
                Dialect::parse(fixture.ctx().config()).unwrap(),
                Dialect::Postgres
            );
        }
        let fixture = TestContext::with_config("datasource:\n  driver: sqlite\n");
        assert_eq!(
            Dialect::parse(fixture.ctx().config()).unwrap(),
            Dialect::Sqlite
        );
    }

    #[tokio::test]
    async fn invalid_driver_fails_without_falling_back_to_mysql() {
        let fixture = TestContext::with_config("datasource:\n  driver: postgress\n");
        fixture.ctx().build_components().unwrap();
        let repository = fixture.ctx().resolve::<UserRepository>().unwrap();
        let error = repository.list_all().await.unwrap_err();
        assert!(error
            .detail
            .unwrap()
            .contains("unsupported datasource.driver `postgress`"));
    }

    #[test]
    fn selected_driver_stays_fixed_after_config_reload() {
        let fixture = TestContext::with_config("datasource:\n  driver: postgres\n");
        fixture.ctx().build_components().unwrap();
        let repository = fixture.ctx().resolve::<UserRepository>().unwrap();
        assert_eq!(repository.dialect().unwrap(), Dialect::Postgres);
        std::fs::write(
            fixture.ctx().config().path(),
            "datasource:\n  driver: mysql\n",
        )
        .unwrap();
        fixture.ctx().config().reload().unwrap();
        assert_eq!(repository.dialect().unwrap(), Dialect::Postgres);
    }

    #[test]
    fn queries_use_bound_parameters_only() {
        // A regression guard for the f-string SQL of the Python version.
        for statement in [
            SELECT_USER,
            INSERT_USER,
            INSERT_GROUP,
            SELECT_USER_POSTGRES,
            INSERT_USER_POSTGRES,
            INSERT_GROUP_POSTGRES,
            SELECT_USER_SQLITE,
        ] {
            assert!(
                !statement.contains('{') && !statement.contains('\''),
                "SQL must not interpolate values: {statement}"
            );
        }
        assert!(SELECT_USER.contains("u1.uid"));
    }
}
