//! Startup-only Nacos HTTP OpenAPI client. Connection settings never come from YAML.

use std::time::Duration;

use anyhow::{bail, Context};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;

pub(super) struct Settings {
    base_url: Url,
    data_id: String,
    group: String,
    namespace: String,
    credentials: Option<(String, String)>,
    timeout: Duration,
}

/// Configuration identity only; credentials and response contents are never retained here.
#[derive(Debug)]
pub(super) struct Source {
    pub server_addr: String,
    pub data_id: String,
    pub group: String,
    pub namespace: String,
}

impl Settings {
    pub(super) fn source(&self) -> Source {
        Source {
            server_addr: self.base_url.to_string(),
            data_id: self.data_id.clone(),
            group: self.group.clone(),
            namespace: if self.namespace.is_empty() {
                "public".to_owned()
            } else {
                self.namespace.clone()
            },
        }
    }

    pub(super) fn from_env() -> anyhow::Result<Option<Self>> {
        Self::from_lookup(|key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => bail!("{key} must contain valid Unicode"),
        })
    }

    fn from_lookup(
        lookup: impl Fn(&str) -> anyhow::Result<Option<String>>,
    ) -> anyhow::Result<Option<Self>> {
        let enabled = lookup("NACOS_ENABLED")?.unwrap_or_else(|| "false".into());
        match enabled.trim().to_ascii_lowercase().as_str() {
            "false" | "0" | "no" | "off" => return Ok(None),
            "true" | "1" | "yes" | "on" => {}
            _ => bail!("NACOS_ENABLED must be a boolean (true/false, 1/0, yes/no, on/off)"),
        }

        let address =
            lookup("NACOS_SERVER_ADDR")?.unwrap_or_else(|| "http://127.0.0.1:8848".into());
        let address = address.trim();
        let address = if address.contains("://") {
            address.to_owned()
        } else {
            format!("http://{address}")
        };
        let mut base_url = Url::parse(&address).context("invalid NACOS_SERVER_ADDR")?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            bail!("NACOS_SERVER_ADDR must be an HTTP(S) address without credentials, query or fragment");
        }
        let path = if base_url.path().trim_matches('/').is_empty() {
            "/nacos/".to_owned()
        } else {
            format!("{}/", base_url.path().trim_end_matches('/'))
        };
        base_url.set_path(&path);

        let data_id = lookup("NACOS_DATA_ID")?.unwrap_or_else(|| "config.yaml".into());
        let group = lookup("NACOS_GROUP")?.unwrap_or_else(|| "DEFAULT_GROUP".into());
        if data_id.trim().is_empty() || group.trim().is_empty() {
            bail!("NACOS_DATA_ID and NACOS_GROUP must not be empty");
        }
        let namespace = lookup("NACOS_NAMESPACE")?.unwrap_or_default();
        let username = lookup("NACOS_USERNAME")?.filter(|v| !v.is_empty());
        let password = lookup("NACOS_PASSWORD")?.filter(|v| !v.is_empty());
        let credentials = match (username, password) {
            (None, None) => None,
            (Some(username), Some(password)) => Some((username, password)),
            _ => bail!("NACOS_USERNAME and NACOS_PASSWORD must be set together"),
        };
        let timeout_ms = lookup("NACOS_TIMEOUT_MS")?
            .unwrap_or_else(|| "5000".into())
            .parse::<u64>()
            .context("NACOS_TIMEOUT_MS must be a positive integer")?;
        if timeout_ms == 0 {
            bail!("NACOS_TIMEOUT_MS must be greater than zero");
        }
        Ok(Some(Self {
            base_url,
            data_id,
            group,
            namespace,
            credentials,
            timeout: Duration::from_millis(timeout_ms),
        }))
    }

    pub(super) async fn fetch(&self) -> anyhow::Result<Option<Value>> {
        let client = Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build Nacos HTTP client")?;

        let token = if let Some((username, password)) = &self.credentials {
            let response = client
                .post(self.base_url.join("v1/auth/login")?)
                .form(&[("username", username), ("password", password)])
                .send()
                .await
                .map_err(reqwest::Error::without_url)
                .context("Nacos login request failed")?;
            if !response.status().is_success() {
                bail!("Nacos login failed with HTTP {}", response.status());
            }
            #[derive(Deserialize)]
            struct Login {
                #[serde(rename = "accessToken")]
                access_token: String,
            }
            let login: Login = response
                .json()
                .await
                .map_err(reqwest::Error::without_url)
                .context("invalid Nacos login response")?;
            if login.access_token.is_empty() {
                bail!("Nacos login returned an empty access token");
            }
            Some(login.access_token)
        } else {
            None
        };

        let mut request = client
            .get(self.base_url.join("v1/cs/configs")?)
            .query(&[("dataId", &self.data_id), ("group", &self.group)]);
        // Nacos uses the namespace ID as `tenant`; public is represented by omission.
        if !self.namespace.is_empty() {
            request = request.query(&[("tenant", &self.namespace)]);
        }
        if let Some(token) = token {
            request = request.query(&[("accessToken", token)]);
        }
        let response = request
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Nacos configuration request failed")?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!(
                "Nacos configuration request failed with HTTP {}",
                response.status()
            );
        }
        let content = response
            .text()
            .await
            .map_err(reqwest::Error::without_url)
            .context("failed to read Nacos configuration")?;
        if content.trim().is_empty() {
            return Ok(None);
        }
        // Do not include YAML parser diagnostics: they can contain configuration secrets.
        let value: Value = serde_yaml_ng::from_str(&content)
            .map_err(|_| anyhow::anyhow!("Nacos configuration must contain valid YAML"))?;
        if !value.is_object() {
            bail!("Nacos configuration must contain a YAML mapping at the top level");
        }
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Form, Query};
    use axum::routing::{get, post};
    use axum::Router;
    use std::collections::HashMap;

    use crate::config::Config;

    fn settings(vars: &[(&str, &str)]) -> anyhow::Result<Option<Settings>> {
        Settings::from_lookup(|key| {
            Ok(vars
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, v)| (*v).into()))
        })
    }

    struct Server {
        address: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Server {
        async fn start(router: Router) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            Self { address, task }
        }

        fn settings(&self) -> Settings {
            settings(&[
                ("NACOS_ENABLED", "true"),
                ("NACOS_SERVER_ADDR", &self.address),
            ])
            .unwrap()
            .unwrap()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    #[test]
    fn environment_defaults_and_validation() {
        assert!(settings(&[]).unwrap().is_none());
        assert!(settings(&[("NACOS_SERVER_ADDR", "invalid")])
            .unwrap()
            .is_none());
        let defaults = settings(&[("NACOS_ENABLED", "ON")]).unwrap().unwrap();
        assert_eq!(defaults.base_url.as_str(), "http://127.0.0.1:8848/nacos/");
        assert_eq!(defaults.data_id, "config.yaml");
        assert_eq!(defaults.group, "DEFAULT_GROUP");
        assert!(defaults.namespace.is_empty());
        assert_eq!(defaults.timeout, Duration::from_secs(5));
        let custom = settings(&[
            ("NACOS_ENABLED", "1"),
            ("NACOS_SERVER_ADDR", "localhost:8848/custom/"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(custom.base_url.as_str(), "http://localhost:8848/custom/");
        assert!(settings(&[("NACOS_ENABLED", "typo")]).is_err());
        for (key, value) in [
            ("NACOS_TIMEOUT_MS", "0"),
            ("NACOS_TIMEOUT_MS", "oops"),
            ("NACOS_DATA_ID", ""),
            ("NACOS_GROUP", ""),
            ("NACOS_SERVER_ADDR", "ftp://localhost"),
            ("NACOS_SERVER_ADDR", "http://localhost?accessToken=secret"),
            ("NACOS_USERNAME", "user"),
            ("NACOS_PASSWORD", "secret"),
        ] {
            assert!(
                settings(&[("NACOS_ENABLED", "true"), (key, value)]).is_err(),
                "{key}"
            );
        }
    }

    #[tokio::test]
    async fn authenticated_fetch_encodes_identifiers_and_namespace() {
        let server = Server::start(
            Router::new()
                .route(
                    "/custom/v1/auth/login",
                    post(|Form(form): Form<HashMap<String, String>>| async move {
                        assert_eq!(form.get("username").unwrap(), "reader");
                        assert_eq!(form.get("password").unwrap(), "p&a=ss+");
                        axum::Json(serde_json::json!({"accessToken": "token&+secret"}))
                    }),
                )
                .route(
                    "/custom/v1/cs/configs",
                    get(|Query(query): Query<HashMap<String, String>>| async move {
                        assert_eq!(query.get("dataId").unwrap(), "服务 & config.yaml");
                        assert_eq!(query.get("group").unwrap(), "custom+group");
                        assert_eq!(query.get("tenant").unwrap(), "namespace-id");
                        assert_eq!(query.get("accessToken").unwrap(), "token&+secret");
                        "server:\n  http:\n    port: 9090\n"
                    }),
                ),
        )
        .await;
        let settings = settings(&[
            ("NACOS_ENABLED", "true"),
            ("NACOS_SERVER_ADDR", &format!("{}/custom", server.address)),
            ("NACOS_USERNAME", "reader"),
            ("NACOS_PASSWORD", "p&a=ss+"),
            ("NACOS_DATA_ID", "服务 & config.yaml"),
            ("NACOS_GROUP", "custom+group"),
            ("NACOS_NAMESPACE", "namespace-id"),
        ])
        .unwrap()
        .unwrap();
        let data = settings.fetch().await.unwrap().unwrap();
        assert_eq!(data["server"]["http"]["port"], 9090);
    }

    #[tokio::test]
    async fn overlay_survives_local_reload_and_replaces_arrays_scalars_and_null() {
        let server = Server::start(Router::new().route("/nacos/v1/cs/configs", get(
            |Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query.get("dataId").unwrap(), "config.yaml");
                assert_eq!(query.get("group").unwrap(), "DEFAULT_GROUP");
                assert!(!query.contains_key("tenant"));
                assert!(!query.contains_key("accessToken"));
                "server:\n  http:\n    port: 9090\nlist: [remote]\nscalar: {nested: true}\nremoved: null\nadded: 42\n"
            }
        ))).await;
        let directory =
            std::env::temp_dir().join(format!("stemson-nacos-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("config.yaml");
        let local = "server:\n  name: local\n  http:\n    host: 0.0.0.0\n    port: 8080\nlist: [a, b]\nscalar: old\nremoved: old\n";
        std::fs::write(&path, local).unwrap();
        let config = Config::load_with_nacos_settings(&path, Some(server.settings()))
            .await
            .unwrap();
        assert_eq!(config.u16("server.http.port", 0), 9090);
        assert_eq!(config.string("server.http.host", ""), "0.0.0.0");
        assert_eq!(config.string("server.name", ""), "local");
        assert_eq!(config.get("list").unwrap(), serde_json::json!(["remote"]));
        assert!(config.bool("scalar.nested", false));
        assert_eq!(config.get("removed"), Some(Value::Null));
        assert_eq!(config.i64("added", 0), 42);
        let snapshot = config.snapshot();
        std::fs::write(
            &path,
            local.replace("local", "updated").replace("8080", "7070"),
        )
        .unwrap();
        config.reload().unwrap();
        assert_eq!(config.u16("server.http.port", 0), 9090);
        assert_eq!(config.string("server.name", ""), "updated");
        assert_eq!(snapshot["server"]["name"], "local");
        std::fs::write(&path, "[]").unwrap();
        assert!(config.reload().is_err());
        assert_eq!(config.string("server.name", ""), "updated");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn missing_and_empty_configs_fall_back_but_errors_are_reported() {
        for (status, content, missing) in [
            (StatusCode::NOT_FOUND, "config data not exist", true),
            (StatusCode::OK, "  \n", true),
            (StatusCode::OK, "{}", false),
            (StatusCode::FORBIDDEN, "secret", false),
            (StatusCode::INTERNAL_SERVER_ERROR, "secret", false),
            (StatusCode::OK, "[array]", false),
            (StatusCode::OK, "password: [secret", false),
        ] {
            let server = Server::start(Router::new().route(
                "/nacos/v1/cs/configs",
                get(move || async move { (status, content) }),
            ))
            .await;
            let result = server.settings().fetch().await;
            if missing {
                assert!(result.unwrap().is_none());
            } else if content == "{}" {
                assert_eq!(result.unwrap().unwrap(), serde_json::json!({}));
            } else {
                let error = format!("{:#}", result.unwrap_err());
                assert!(!error.contains("secret"));
            }
        }
    }

    #[tokio::test]
    async fn timeout_and_login_failures_are_reported_without_secrets() {
        let server = Server::start(
            Router::new()
                .route(
                    "/nacos/v1/cs/configs",
                    get(|| async {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        "{}"
                    }),
                )
                .route(
                    "/nacos/v1/auth/login",
                    post(|| async { (StatusCode::UNAUTHORIZED, "secret") }),
                ),
        )
        .await;
        let mut settings = server.settings();
        settings.timeout = Duration::from_millis(20);
        assert!(settings.fetch().await.is_err());
        settings.credentials = Some(("reader".into(), "secret".into()));
        settings.timeout = Duration::from_secs(5);
        let error = format!("{:#}", settings.fetch().await.unwrap_err());
        assert!(error.contains("401"));
        assert!(!error.contains("secret"));
    }
}
