//! User endpoints, mirrors `backend/controller/user/UserController.py`.
//!
//! Each Python controller class becomes one struct here, so the automatic route
//! derivation produces the same paths:
//!
//! | struct | route |
//! |--------|-------|
//! | `ValidatePassword` | `POST /user/user_controller/validate_password` |
//! | `AddUser` | `POST /user/user_controller/add_user` |
//! | `GetUserInfo` | `POST /user/user_controller/get_user_info` |
//! | `DeleteUser` | `POST /user/user_controller/delete_user` |
//! | `ModifyUser` | `POST /user/user_controller/modify_user` |
//! | `GetAllInfo` | `POST /getall` (explicit path) |

use std::sync::Arc;

use serde::Deserialize;

use crate::exception::{ApiError, ApiResult};
use crate::model::identity::UserIdentity;
use crate::model::response::ApiResponse;
use crate::model::status_code::StatusCode;
use crate::request::context::RequestContext;
use crate::service::{CreateUserCommand, UpdateUserCommand, UserService};

/// Email pattern used by `ModifyUser`.
///
/// The original Python pattern was
/// `^([a-z0-9A-Z]+[-|\.]?)+[a-z0-9A-Z]@([a-z0-9A-Z]+(-[a-z0-9A-Z]+)?\.)+[a-zA-Z]{2,}$`,
/// which rejected ordinary addresses such as `first_last@example.com` or
/// `user+tag@example.com` (and allowed a literal `|` in the local part). This
/// permissive pattern keeps the same intent without the false negatives; see
/// `docs/MIGRATION.md`.
const EMAIL_PATTERN: &str = r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$";

/// Login payload for the password check endpoint.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ValidatePasswordRequest {
    /// Login name.
    pub username: String,
    /// Plain text password.
    pub password: String,
}

/// Payload of `add_user`.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateUserRequest {
    /// Login name.
    pub username: Option<String>,
    /// Plain text password.
    pub password: Option<String>,
    /// Optional phone number.
    pub phone: Option<String>,
    /// Optional email.
    pub email: Option<String>,
    /// 0 = normal user, 1 = administrator.
    pub role: Option<i32>,
}

/// Payload of `get_user_information`.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
pub struct GetUserInfoRequest {
    /// Look up by id.
    pub user_id: Option<i64>,
    /// Look up by login name.
    pub username: Option<String>,
}

/// Payload of `delete_user`.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
pub struct DeleteUserRequest {
    /// User to delete.
    pub user_id: Option<i64>,
    /// Alternative lookup by login name.
    pub username: Option<String>,
}

/// Payload of `modify_user`.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ModifyUserRequest {
    /// User to modify.
    pub user_id: i64,
    /// New login name.
    pub username: String,
    /// New email.
    pub email: String,
    /// New plain text password.
    pub password: String,
    /// New role (0 or 1).
    pub user_role: i32,
    /// New status (0 or 1).
    pub status: i32,
}

/// `POST /user/user_controller/validate_password`
#[derive(sanicboot_macros::Autowired)]
pub struct ValidatePassword {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller]
impl ValidatePassword {
    /// Check a username/password pair without issuing a token.
    #[post]
    #[validate(
        json,
        not_empty(["username", "password"]),
        length("password", 5, 20, "Length of password must be 5 to 20"),
    )]
    async fn post(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let payload: ValidatePasswordRequest = ctx.json_as()?;
        let valid = self
            .user_service
            .verify_credentials(&payload.username, &payload.password)
            .await?;
        if valid {
            Ok(ApiResponse::success_with_message(
                "username and password are valid",
                serde_json::json!({ "valid": true }),
            ))
        } else {
            Err(ApiError::bad_credentials())
        }
    }
}

/// `POST /user/user_controller/add_user`
#[derive(sanicboot_macros::Autowired)]
pub struct AddUser {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller]
impl AddUser {
    /// Create a user. Requires an administrator.
    #[post]
    #[protected]
    async fn post(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let claims = ctx.require_user()?;
        ctx.require_admin("add user information")?;

        let payload: CreateUserRequest = ctx.json_as()?;
        let username = payload
            .username
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ApiError::miss_parameters("username is empty"))?;
        let password = payload
            .password
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::miss_parameters("password is empty"))?;
        let role = payload
            .role
            .ok_or_else(|| ApiError::miss_parameters("user role is empty"))?;
        if role != 0 && role != 1 {
            return Err(ApiError::new(
                StatusCode::AddUserFailed,
                "role id is not 0 or 1",
            ));
        }

        let created = self
            .user_service
            .create_user(
                CreateUserCommand {
                    username,
                    password,
                    phone: payload.phone,
                    email: payload.email,
                    role,
                },
                claims.user_id,
            )
            .await?;

        Ok(ApiResponse::success_with_message(
            format!("Add user {} success", created.username),
            created.to_dict(),
        ))
    }
}

/// `POST /user/user_controller/get_user_info`
#[derive(sanicboot_macros::Autowired)]
pub struct GetUserInfo {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller]
impl GetUserInfo {
    /// Look up a single user. Requires an administrator.
    #[post]
    #[protected]
    async fn post(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        ctx.require_admin("get user information")?;
        let payload: GetUserInfoRequest = ctx.json_as()?;

        let user = match (payload.user_id, payload.username) {
            (Some(uid), _) => self.user_service.get_user_by_uid(uid).await?,
            (None, Some(username)) => self.user_service.get_user_by_username(&username).await?,
            (None, None) => {
                return Err(ApiError::miss_parameters("username is empty"));
            }
        };
        Ok(ApiResponse::success(user.to_dict()))
    }
}

/// `POST /user/user_controller/delete_user`
#[derive(sanicboot_macros::Autowired)]
pub struct DeleteUser {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller]
impl DeleteUser {
    /// Delete a user. Requires an administrator.
    #[post]
    #[protected]
    async fn post(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        ctx.require_admin("delete user information")?;
        let payload: DeleteUserRequest = ctx.json_as()?;

        let uid = payload
            .user_id
            .ok_or_else(|| ApiError::miss_parameters("user_id is empty"))?;
        let removed = self.user_service.delete_user(uid).await?;
        Ok(ApiResponse::success_with_message(
            format!("delete {} Success", removed.username),
            removed.to_dict(),
        ))
    }
}

/// `POST /user/user_controller/modify_user`
#[derive(sanicboot_macros::Autowired)]
pub struct ModifyUser {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller]
impl ModifyUser {
    /// Modify a user. Requires an administrator.
    #[post]
    #[protected]
    #[validate(
        json,
        not_empty([
            "user_id",
            "username",
            "email",
            "password",
            "user_role",
            "status"
        ]),
        pattern("email", EMAIL_PATTERN, match, "Email format invalid"),
    )]
    async fn post(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        ctx.require_admin("modify user information")?;
        let payload: ModifyUserRequest = ctx.json_as()?;

        let updated = self
            .user_service
            .update_user(UpdateUserCommand {
                uid: payload.user_id,
                username: Some(payload.username),
                password: Some(payload.password),
                phone: None,
                email: Some(payload.email),
                role: Some(payload.user_role),
                status: Some(payload.status),
            })
            .await?;

        Ok(ApiResponse::success_with_message(
            format!("Modify {} Success: ", updated.username),
            updated.to_dict(),
        ))
    }
}

/// `POST /getall` — the explicit path of the original
/// `@Controller("/getall")` declaration.
#[derive(sanicboot_macros::Autowired)]
pub struct GetAllInfo {
    user_service: Arc<UserService>,
}

#[sanicboot_macros::controller("/getall")]
impl GetAllInfo {
    /// List every user. Requires an administrator identity.
    #[post]
    #[protected]
    #[authority([UserIdentity::Admin])]
    async fn post(&self, _ctx: RequestContext) -> ApiResult<ApiResponse> {
        let users = self.user_service.list_users().await?;
        let payload: Vec<serde_json::Value> = users.iter().map(|user| user.to_dict()).collect();
        Ok(ApiResponse::success(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::EMAIL_PATTERN;

    fn matches(value: &str) -> bool {
        let regex = regex::Regex::new(EMAIL_PATTERN).expect("valid email pattern");
        // the validator uses `PatternMode::Match`, i.e. anchored at the start
        regex
            .find(value)
            .map(|found| found.start() == 0)
            .unwrap_or(false)
    }

    #[test]
    fn email_pattern_accepts_ordinary_addresses() {
        for value in [
            "lovemefan@outlook.com",
            "first_last@example.com",
            "user+tag@example.co.uk",
            "a.b-c@sub.example.org",
        ] {
            assert!(matches(value), "{value} should be accepted");
        }
    }

    #[test]
    fn email_pattern_rejects_invalid_addresses() {
        for value in ["not-an-email", "@example.com", "user@", "user@example"] {
            assert!(!matches(value), "{value} should be rejected");
        }
    }
}
