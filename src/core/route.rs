//! HTTP route metadata and automatic path derivation.

use http::Method;

use crate::utils::text::{normalize_path, to_snake_case};

/// One registered HTTP route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// Full path, e.g. `/user/user_controller/add_user`.
    pub path: String,
    /// HTTP method.
    pub method: Method,
    /// Controller that owns the route.
    pub controller: &'static str,
}

impl RouteEntry {
    /// Build an entry.
    pub fn new(path: impl Into<String>, method: Method, controller: &'static str) -> Self {
        Self {
            path: normalize_path(&path.into()),
            method,
            controller,
        }
    }

    /// `POST /user/user_controller/add_user`
    pub fn label(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// All routes registered by the controllers, in registration order.
#[derive(Debug, Default)]
pub struct RouteTable {
    entries: Vec<RouteEntry>,
}

impl RouteTable {
    /// Record a route.
    pub fn insert(&mut self, entry: RouteEntry) {
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
    }

    /// Record several routes.
    pub fn extend(&mut self, entries: impl IntoIterator<Item = RouteEntry>) {
        for entry in entries {
            self.insert(entry);
        }
    }

    /// Registered routes.
    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }

    /// Whether `path` is served for `method`.
    pub fn contains(&self, path: &str, method: &Method) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.path == path && entry.method == *method)
    }

    /// All distinct paths.
    pub fn paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.entries.iter().map(|e| e.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }
}

/// Join a controller base path with an optional sub-path.
///
/// `join_path("/v1/api/auth", Some("/me"))` → `/v1/api/auth/me`
pub fn join_path(base: &str, suffix: Option<&str>) -> String {
    let base = normalize_path(base);
    match suffix {
        None => base,
        Some(suffix) => {
            let suffix = suffix.trim().trim_matches('/');
            if suffix.is_empty() {
                base
            } else {
                normalize_path(&format!("{}/{suffix}", base.trim_end_matches('/')))
            }
        }
    }
}

/// Derive the base path of a controller.
///
/// Mirrors `backend/core/component/controller.py`:
///
/// ```text
/// uri = f"{cls.__module__}.{cls.__name__}".replace(namespace, "").replace(".", "/")
/// uri = "/".join(snake_case(segment) for segment in uri.split("/") if segment)
/// ```
///
/// In Rust the Python module path corresponds to `module_path!()`; the
/// configured namespace `backend.controller` corresponds to
/// `<crate>::controller`.
///
/// ```text
/// module_path!() = "stemson::controller::user::user_controller"
/// crate_name     = "stemson"
/// struct         = "AddUser"
/// => "/user/user_controller/add_user"
/// ```
pub fn derive_path(
    module_path: &str,
    crate_name: &str,
    struct_name: &str,
    explicit: Option<&str>,
) -> String {
    if let Some(path) = explicit {
        return normalize_path(path);
    }

    let namespace = format!("{crate_name}::controller");
    let crate_prefix = format!("{crate_name}::");
    let relative = module_path
        .strip_prefix(&namespace)
        .map(|rest| rest.trim_start_matches("::"))
        .or_else(|| module_path.strip_prefix(&crate_prefix))
        .unwrap_or(module_path);

    let mut segments: Vec<String> = relative
        .split("::")
        .filter(|segment| !segment.is_empty())
        .map(to_snake_case)
        .collect();
    segments.push(to_snake_case(struct_name));
    normalize_path(&format!("/{}", segments.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_same_paths_as_the_python_prototype() {
        let module = "stemson::controller::user::user_controller";
        for (class, expected) in [
            (
                "ValidatePassword",
                "/user/user_controller/validate_password",
            ),
            ("AddUser", "/user/user_controller/add_user"),
            ("GetUserInfo", "/user/user_controller/get_user_info"),
            ("DeleteUser", "/user/user_controller/delete_user"),
            ("ModifyUser", "/user/user_controller/modify_user"),
            ("GetAllInfo", "/user/user_controller/get_all_info"),
        ] {
            assert_eq!(derive_path(module, "stemson", class, None), expected);
        }
    }

    #[test]
    fn sub_paths_are_joined() {
        assert_eq!(join_path("/v1/api/auth", None), "/v1/api/auth");
        assert_eq!(join_path("/v1/api/auth", Some("/me")), "/v1/api/auth/me");
        assert_eq!(join_path("/v1/api/auth/", Some("me")), "/v1/api/auth/me");
    }

    #[test]
    fn explicit_paths_win() {
        let module = "stemson::controller::user::user_controller";
        assert_eq!(
            derive_path(module, "stemson", "GetAllInfo", Some("/getall")),
            "/getall"
        );
        assert_eq!(
            derive_path(module, "stemson", "GetAllInfo", Some("getall/")),
            "/getall"
        );
    }

    #[test]
    fn paths_outside_the_controller_namespace_keep_their_segments() {
        assert_eq!(
            derive_path("my_app::handlers::user", "my_app", "Profile", None),
            "/handlers/user/profile"
        );
    }
}
