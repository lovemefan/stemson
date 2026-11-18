//! Shared connection settings and authentication for configuration and naming.

use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use reqwest::{Method, Response, StatusCode, Url};
use serde::Deserialize;

pub(crate) fn env(key: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => bail!("{key} must contain valid Unicode"),
    }
}

pub(crate) fn enabled(value: Option<String>, key: &str) -> anyhow::Result<bool> {
    match value
        .as_deref()
        .unwrap_or("false")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "false" | "0" | "no" | "off" => Ok(false),
        "true" | "1" | "yes" | "on" => Ok(true),
        _ => bail!("{key} must be a boolean (true/false, 1/0, yes/no, on/off)"),
    }
}

pub(crate) struct ConnectionSettings {
    pub base_url: Url,
    pub namespace: String,
    pub credentials: Option<(String, String)>,
    pub timeout: Duration,
}

impl ConnectionSettings {
    pub fn from_lookup(
        lookup: &impl Fn(&str) -> anyhow::Result<Option<String>>,
    ) -> anyhow::Result<Self> {
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
        Ok(Self {
            base_url,
            namespace,
            credentials,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    pub fn namespace_label(&self) -> &str {
        if self.namespace.is_empty() {
            "public"
        } else {
            &self.namespace
        }
    }
}

pub(crate) struct Client {
    pub settings: ConnectionSettings,
    http: reqwest::Client,
    token: Option<(String, Instant)>,
}

impl Client {
    pub fn new(settings: ConnectionSettings) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(settings.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build Nacos HTTP client")?;
        Ok(Self {
            settings,
            http,
            token: None,
        })
    }

    async fn authenticate(&mut self) -> anyhow::Result<()> {
        let Some((username, password)) = &self.settings.credentials else {
            return Ok(());
        };
        if self
            .token
            .as_ref()
            .is_some_and(|(_, expires)| Instant::now() < *expires)
        {
            return Ok(());
        }
        let response = self
            .http
            .post(self.settings.base_url.join("v1/auth/login")?)
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
            #[serde(rename = "tokenTtl", default = "default_ttl")]
            token_ttl: u64,
        }
        fn default_ttl() -> u64 {
            18000
        }
        let login: Login = response
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("invalid Nacos login response")?;
        if login.access_token.is_empty() {
            bail!("Nacos login returned an empty access token");
        }
        // Refresh early, and bound untrusted TTLs to avoid overflowing Instant.
        let ttl = Duration::from_secs(login.token_ttl.min(86400).saturating_sub(30));
        self.token = Some((login.access_token, Instant::now() + ttl));
        Ok(())
    }

    pub async fn request(
        &mut self,
        method: Method,
        path: &str,
        params: &[(String, String)],
    ) -> anyhow::Result<Response> {
        for attempt in 0..2 {
            self.authenticate().await?;
            let mut request = self
                .http
                .request(method.clone(), self.settings.base_url.join(path)?);
            if let Some((token, _)) = &self.token {
                request = request.query(&[("accessToken", token)]);
            }
            // Servlet containers parse POST forms natively. Use query parameters
            // for PUT/DELETE so v1 naming also works without a PUT form filter.
            request = if method == Method::POST {
                request.form(params)
            } else {
                request.query(params)
            };
            let response = request
                .send()
                .await
                .map_err(reqwest::Error::without_url)
                .context("Nacos request failed")?;
            if matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) && self.settings.credentials.is_some()
                && attempt == 0
            {
                self.token = None;
                continue;
            }
            return Ok(response);
        }
        unreachable!("last request attempt returns its response")
    }
}
