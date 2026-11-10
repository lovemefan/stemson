//! Text helpers.
//!
//! Rust port of `backend/utils/textProcess.py`, minus the SQL injection
//! blacklist, which was replaced by parameterized queries (see
//! `docs/MIGRATION.md`).

/// Convert `CamelCase` to `snake_case`, mirroring
/// `backend.utils.textProcess.name_convert_to_snake`.
///
/// ```text
/// AddUser          -> add_user
/// GetAllInfo       -> get_all_info
/// MysqlDataSource  -> mysql_data_source
/// ```
pub fn to_snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in chars.iter().enumerate() {
        // Python: re.sub(r"([a-z])([A-Z])", r"\1_\2", name).lower()
        if ch.is_uppercase() && i > 0 && chars[i - 1].is_lowercase() {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Normalize an HTTP path: leading slash, no trailing slash (except root).
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let with_leading = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let trimmed_end = with_leading.trim_end_matches('/');
    if trimmed_end.is_empty() {
        "/".to_string()
    } else {
        trimmed_end.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_matches_python_helper() {
        assert_eq!(to_snake_case("AddUser"), "add_user");
        assert_eq!(to_snake_case("GetAllInfo"), "get_all_info");
        assert_eq!(to_snake_case("UserService"), "user_service");
        assert_eq!(to_snake_case("MysqlDataSource"), "mysql_data_source");
        assert_eq!(to_snake_case("User"), "user");
        assert_eq!(to_snake_case("OAuth2Token"), "oauth2token");
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(normalize_path("getall"), "/getall");
        assert_eq!(normalize_path("/getall/"), "/getall");
        assert_eq!(normalize_path(" /a/b "), "/a/b");
        assert_eq!(normalize_path(""), "/");
    }
}
