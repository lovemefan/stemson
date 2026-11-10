//! Authorization helpers, mirrors `backend/core/decorator/authority.py`.
//!
//! In the Python project this was a decorator (`@authority([...])`) because
//! Python had no better way to run a check before the handler. In Rust the
//! `#[authority(...)]` attribute on a controller method expands into a call to
//! [`require_identity`] before the handler body runs.

use crate::auth::AuthUser;
use crate::exception::{ApiError, ApiResult};
use crate::model::identity::UserIdentity;

/// Require the caller to hold at least one of the given identities.
///
/// The comparison is a privilege comparison: `SUPER_ADMIN` satisfies
/// `[ADMIN]`, and `ADMIN` satisfies `[USER]`.
pub fn require_identity(user: &AuthUser, allowed: &[UserIdentity]) -> ApiResult<()> {
    let identity = user
        .identity()
        .ok_or_else(|| ApiError::unauthorized("Invalid user identity."))?;
    if allowed.iter().any(|required| identity.satisfies(*required)) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "Invalid user identity: {identity} is not allowed here"
        )))
    }
}

/// Require an administrator (`role != 0`), which is what the original
/// controllers checked with `if not user.role`.
pub fn require_admin(user: &AuthUser, action: &str) -> ApiResult<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "You have no authorized to {action}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Claims;

    fn user_with(identity: UserIdentity, role: i32) -> AuthUser {
        AuthUser::new(Claims::new(1, "tester", role, identity, 3600))
    }

    #[test]
    fn privilege_comparison_is_inclusive() {
        let super_admin = user_with(UserIdentity::SuperAdmin, 1);
        assert!(require_identity(&super_admin, &[UserIdentity::Admin]).is_ok());
        assert!(require_identity(&super_admin, &[UserIdentity::User]).is_ok());

        let user = user_with(UserIdentity::User, 0);
        assert!(require_identity(&user, &[UserIdentity::Admin]).is_err());
        assert!(require_identity(&user, &[UserIdentity::User]).is_ok());
    }

    #[test]
    fn admin_requires_role_not_zero() {
        assert!(require_admin(&user_with(UserIdentity::Admin, 1), "add user").is_ok());
        let err = require_admin(&user_with(UserIdentity::User, 0), "add user").unwrap_err();
        assert_eq!(err.status, http::StatusCode::FORBIDDEN);
        assert!(err.message.contains("You have no authorized to add user"));
    }
}
