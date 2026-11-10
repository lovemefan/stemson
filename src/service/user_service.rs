//! User business logic, mirrors `backend/service/userService/UserService.py`.

use std::sync::Arc;

use crate::exception::{ApiError, ApiResult};
use crate::model::identity::UserIdentity;
use crate::model::user::{naive_to_local, User};
use crate::repository::user_repository::{LoginMaterial, NewUserRecord, UserRepository, UserRow};
use crate::utils::hash::{hash_password, verify_password};
use crate::utils::snowflake;

/// Command used to create a user.
#[derive(Debug, Clone)]
pub struct CreateUserCommand {
    /// Login name.
    pub username: String,
    /// Plain text password. `None` creates an OPAQUE account and returns an
    /// invitation token instead.
    pub password: Option<String>,
    /// Optional phone number.
    pub phone: Option<String>,
    /// Optional email.
    pub email: Option<String>,
    /// 0 = normal user, 1 = administrator.
    pub role: i32,
}

/// Command used to modify a user. `None` means "leave unchanged".
#[derive(Debug, Clone)]
pub struct UpdateUserCommand {
    /// Target user id.
    pub uid: i64,
    /// New login name.
    pub username: Option<String>,
    /// New plain text password.
    pub password: Option<String>,
    /// New phone number.
    pub phone: Option<String>,
    /// New email.
    pub email: Option<String>,
    /// New role (0 or 1).
    pub role: Option<i32>,
    /// New status (0 or 1).
    pub status: Option<i32>,
}

/// `@Service`
#[stemson_macros::service]
pub struct UserService {
    user_repository: Arc<UserRepository>,
}

impl UserService {
    /// Every user, ordered by id.
    pub async fn list_users(&self) -> ApiResult<Vec<User>> {
        let rows = self.user_repository.list_all().await?;
        Ok(rows.into_iter().map(row_to_user).collect())
    }

    /// Look a user up by id.
    pub async fn get_user_by_uid(&self, uid: i64) -> ApiResult<User> {
        let row = self
            .user_repository
            .find_by_uid(uid)
            .await?
            .ok_or_else(|| missing_user(uid))?;
        Ok(row_to_user(row))
    }

    /// Look a user up by login name.
    pub async fn get_user_by_username(&self, username: &str) -> ApiResult<User> {
        let row = self
            .user_repository
            .find_by_username(username)
            .await?
            .ok_or_else(|| ApiError::user_not_exist(username))?;
        Ok(row_to_user(row))
    }

    /// Record a successful login.
    pub async fn touch_login(&self, uid: i64) -> ApiResult<()> {
        self.user_repository.touch_login(uid).await
    }

    /// Stored credentials for a login name.
    pub async fn login_material(&self, username: &str) -> ApiResult<Option<LoginMaterial>> {
        self.user_repository.login_material(username).await
    }

    /// Verify a username/password pair without touching the login timestamp.
    ///
    /// Accounts that only carry an OPAQUE registration record cannot log in
    /// through the password path.
    pub async fn verify_credentials(&self, username: &str, password: &str) -> ApiResult<bool> {
        let Some(material) = self.user_repository.login_material(username).await? else {
            return Err(ApiError::user_not_exist(username));
        };
        let Some(stored) = material.password else {
            return Ok(false);
        };
        Ok(verify_password(password, &stored))
    }

    /// Authenticate a user and record the login time.
    pub async fn authenticate(&self, username: &str, password: &str) -> ApiResult<User> {
        let user = self.get_user_by_username(username).await?;
        if !self.verify_credentials(username, password).await? {
            return Err(ApiError::bad_credentials());
        }
        if user.status == 0 {
            return Err(ApiError::forbidden("This account has been disabled."));
        }
        self.user_repository.touch_login(user.uid).await?;
        self.get_user_by_uid(user.uid).await
    }

    /// Create a user on behalf of an administrator.
    ///
    /// With a password the account uses password login (Argon2id). Without one
    /// the account is OPAQUE-only and the returned invitation token must be
    /// handed to the user, who registers through `/v1/api/auth/register/*`.
    pub async fn create_user(
        &self,
        command: CreateUserCommand,
        creator_uid: i64,
    ) -> ApiResult<(User, Option<String>)> {
        if command.username.trim().is_empty() {
            return Err(ApiError::miss_parameters("username is empty"));
        }
        if command.role != 0 && command.role != 1 {
            return Err(ApiError::new(
                crate::model::status_code::StatusCode::AddUserFailed,
                "role id is not 0 or 1",
            ));
        }

        let password = command
            .password
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let password_hash = match password {
            Some(password) => Some(hash_password(password)?),
            None => None,
        };
        let invitation = match password {
            Some(_) => None,
            None => Some(new_invitation()),
        };

        let uid = snowflake::next_id()?;
        let identity = if command.role == 1 {
            UserIdentity::Admin
        } else {
            UserIdentity::User
        };
        let record = NewUserRecord {
            uid,
            username: command.username.trim().to_string(),
            password_hash,
            phone: normalize_optional(command.phone),
            email: normalize_optional(command.email),
            role: command.role,
            identity: identity.as_i32(),
            create_by: creator_uid,
        };
        self.user_repository.insert_user_and_group(&record).await?;

        let invite_token = match invitation {
            Some(invitation) => {
                self.user_repository
                    .set_invite(uid, &invitation.token_hash, invitation.expires_at)
                    .await?;
                Some(invitation.token)
            }
            None => None,
        };

        Ok((self.get_user_by_uid(uid).await?, invite_token))
    }

    /// Finish an OPAQUE registration with the invitation issued by
    /// [`UserService::create_user`].
    pub async fn register_opaque(
        &self,
        username: &str,
        invite_token: &str,
        registration: Vec<u8>,
    ) -> ApiResult<()> {
        let affected = self
            .user_repository
            .consume_invite(username, &sha256_hex(invite_token.trim()), &registration)
            .await?;
        if affected == 0 {
            return Err(ApiError::invalid_parameter(
                "invalid or expired registration invitation",
            ));
        }
        Ok(())
    }

    /// Modify the mutable fields of a user.
    pub async fn update_user(&self, command: UpdateUserCommand) -> ApiResult<User> {
        let current = self.get_user_by_uid(command.uid).await?;

        if let Some(role) = command.role {
            if role != 0 && role != 1 {
                return Err(ApiError::new(
                    crate::model::status_code::StatusCode::ModifyUserFailed,
                    "role id is not 0 or 1",
                ));
            }
        }
        if let Some(status) = command.status {
            if status != 0 && status != 1 {
                return Err(ApiError::new(
                    crate::model::status_code::StatusCode::ModifyUserFailed,
                    "status is not 0 or 1",
                ));
            }
        }

        let username = command
            .username
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(current.username.clone());
        let phone = command.phone.or_else(|| current.phone.clone());
        let email = command.email.or_else(|| current.email.clone());
        let role = command.role.unwrap_or(current.role);
        let status = command.status.unwrap_or(current.status);

        self.user_repository
            .update_profile(
                command.uid,
                username.trim(),
                phone.as_deref(),
                email.as_deref(),
                role,
                status,
            )
            .await?;

        if let Some(password) = command.password.filter(|value| !value.is_empty()) {
            self.user_repository
                .update_password(command.uid, &hash_password(&password)?)
                .await?;
        }

        self.get_user_by_uid(command.uid).await
    }

    /// Delete a user, returning the removed record.
    pub async fn delete_user(&self, uid: i64) -> ApiResult<User> {
        let user = self.get_user_by_uid(uid).await?;
        let affected = self.user_repository.delete(uid).await?;
        if affected == 0 {
            return Err(missing_user(uid));
        }
        Ok(user)
    }
}

fn missing_user(uid: i64) -> ApiError {
    ApiError::new(
        crate::model::status_code::StatusCode::UserNotExist,
        format!("User '{uid}' is not exist"),
    )
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// A freshly generated one-time registration invitation.
struct Invitation {
    token: String,
    token_hash: String,
    expires_at: chrono::NaiveDateTime,
}

/// Invitations are valid for 24 hours and can be used exactly once.
fn new_invitation() -> Invitation {
    use base64::Engine;
    use opaque_ke::rand::RngCore;

    let mut raw = [0u8; 32];
    opaque_ke::rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    Invitation {
        token_hash: sha256_hex(&token),
        token,
        expires_at: chrono::Utc::now().naive_utc() + chrono::Duration::hours(24),
    }
}

fn sha256_hex(value: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn row_to_user(row: UserRow) -> User {
    User {
        uid: row.uid,
        username: row.username,
        phone: row.phone,
        email: row.email,
        password: None,
        role: row.user_role as i32,
        user_identity: UserIdentity::from_i32(row.identity as i32).unwrap_or_default(),
        create_by: row.create_by,
        create_time: row.create_time.map(naive_to_local),
        last_login_time: row.last_login_time.map(naive_to_local),
        status: row.status as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_values_are_trimmed_and_emptied() {
        assert_eq!(normalize_optional(Some("  ".into())), None);
        assert_eq!(
            normalize_optional(Some("  a@b.co ".into())),
            Some("a@b.co".into())
        );
        assert_eq!(normalize_optional(None), None);
    }

    #[test]
    fn rows_map_into_the_public_model() {
        let row = UserRow {
            uid: 1342014866998829056,
            username: "lovemefan".into(),
            phone: None,
            email: Some("lovemefan@outlook.com".into()),
            user_role: 1,
            create_by: Some("admin".into()),
            create_time: None,
            last_login_time: None,
            status: 1,
            identity: 2,
        };
        let user = row_to_user(row);
        assert_eq!(user.user_identity, UserIdentity::Admin);
        assert_eq!(user.create_by.as_deref(), Some("admin"));
        assert!(user.password.is_none());
    }
}
