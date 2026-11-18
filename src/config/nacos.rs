//! Startup-only Nacos HTTP OpenAPI client. Connection settings never come from YAML.

use anyhow::{bail, Context};
use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::nacos::client::{enabled, env, Client, ConnectionSettings};

pub(super) struct Settings {
    connection: ConnectionSettings,
    data_id: String,
    group: String,
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
            server_addr: self.connection.base_url.to_string(),
            data_id: self.data_id.clone(),
            group: self.group.clone(),
            namespace: self.connection.namespace_label().to_owned(),
        }
    }

    pub(super) fn from_env() -> anyhow::Result<Option<Self>> {
        Self::from_lookup(env)
    }

    fn from_lookup(
        lookup: impl Fn(&str) -> anyhow::Result<Option<String>>,
    ) -> anyhow::Result<Option<Self>> {
        if !enabled(lookup("NACOS_ENABLED")?, "NACOS_ENABLED")? {
            return Ok(None);
        }
        let connection = ConnectionSettings::from_lookup(&lookup)?;

        let data_id = lookup("NACOS_DATA_ID")?.unwrap_or_else(|| "config.yaml".into());
        let group = lookup("NACOS_GROUP")?.unwrap_or_else(|| "DEFAULT_GROUP".into());
        if data_id.trim().is_empty() || group.trim().is_empty() {
            bail!("NACOS_DATA_ID and NACOS_GROUP must not be empty");
        }
        Ok(Some(Self {
            connection,
            data_id,
            group,
        }))
    }

    pub(super) async fn fetch(self) -> anyhow::Result<Option<Value>> {
        let mut params = vec![
            ("dataId".into(), self.data_id),
            ("group".into(), self.group),
        ];
        if !self.connection.namespace.is_empty() {
            params.push(("tenant".into(), self.connection.namespace.clone()));
        }
        let mut client = Client::new(self.connection)?;
        let response = client
            .request(Method::GET, "v1/cs/configs", &params)
            .await
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
    use std::time::Duration;

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
        assert_eq!(
            defaults.connection.base_url.as_str(),
            "http://127.0.0.1:8848/nacos/"
        );
        assert_eq!(defaults.data_id, "config.yaml");
        assert_eq!(defaults.group, "DEFAULT_GROUP");
        assert!(defaults.connection.namespace.is_empty());
        assert_eq!(defaults.connection.timeout, Duration::from_secs(5));
        let custom = settings(&[
            ("NACOS_ENABLED", "1"),
            ("NACOS_SERVER_ADDR", "localhost:8848/custom/"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            custom.connection.base_url.as_str(),
            "http://localhost:8848/custom/"
        );
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
        settings.connection.timeout = Duration::from_millis(20);
        assert!(settings.fetch().await.is_err());
        let mut settings = server.settings();
        settings.connection.credentials = Some(("reader".into(), "secret".into()));
        settings.connection.timeout = Duration::from_secs(5);
        let error = format!("{:#}", settings.fetch().await.unwrap_err());
        assert!(error.contains("401"));
        assert!(!error.contains("secret"));
    }
}
