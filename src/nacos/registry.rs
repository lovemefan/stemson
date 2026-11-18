use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use reqwest::Method;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::client::{enabled, env, Client, ConnectionSettings};
use crate::config::Config;

struct Settings {
    connection: ConnectionSettings,
    instance: Instance,
    heartbeat_interval: Duration,
}

#[derive(Clone)]
struct Instance {
    service: String,
    group: String,
    cluster: String,
    ip: IpAddr,
    port: u16,
    metadata: BTreeMap<String, String>,
}

impl Settings {
    fn from_lookup(
        lookup: impl Fn(&str) -> anyhow::Result<Option<String>>,
        default_service: &str,
        address: SocketAddr,
    ) -> anyhow::Result<Option<Self>> {
        if !enabled(lookup("NACOS_REGISTRY_ENABLED")?, "NACOS_REGISTRY_ENABLED")? {
            return Ok(None);
        }
        let connection = ConnectionSettings::from_lookup(&lookup)?;
        let service = lookup("NACOS_SERVICE_NAME")?.unwrap_or_else(|| default_service.to_owned());
        let group = lookup("NACOS_SERVICE_GROUP")?.unwrap_or_else(|| "DEFAULT_GROUP".into());
        let cluster = lookup("NACOS_CLUSTER_NAME")?.unwrap_or_else(|| "DEFAULT".into());
        if [service.as_str(), group.as_str(), cluster.as_str()]
            .iter()
            .any(|s| s.trim().is_empty() || s.contains("@@"))
        {
            bail!("NACOS_SERVICE_NAME, NACOS_SERVICE_GROUP and NACOS_CLUSTER_NAME must be nonempty and must not contain @@");
        }
        let ip = match lookup("NACOS_INSTANCE_IP")? {
            Some(ip) => ip
                .parse::<IpAddr>()
                .context("NACOS_INSTANCE_IP must be an IP address")?,
            None => address.ip(),
        };
        if ip.is_unspecified() || ip.is_multicast() {
            bail!("set NACOS_INSTANCE_IP to an address reachable by consumers; wildcard and multicast addresses cannot be registered");
        }
        let port = match lookup("NACOS_INSTANCE_PORT")? {
            Some(port) => port
                .parse::<u16>()
                .context("NACOS_INSTANCE_PORT must be a port from 1 to 65535")?,
            None => address.port(),
        };
        if port == 0 {
            bail!("NACOS_INSTANCE_PORT must be greater than zero");
        }
        let metadata = match lookup("NACOS_METADATA")? {
            Some(raw) => serde_json::from_str::<BTreeMap<String, String>>(&raw).map_err(|_| {
                anyhow::anyhow!("NACOS_METADATA must be a JSON object with string values")
            })?,
            None => BTreeMap::new(),
        };
        let interval = lookup("NACOS_HEARTBEAT_INTERVAL_MS")?
            .unwrap_or_else(|| "5000".into())
            .parse::<u64>()
            .context("NACOS_HEARTBEAT_INTERVAL_MS must be an integer from 1000 to 5000")?;
        if !(1000..=5000).contains(&interval) {
            bail!("NACOS_HEARTBEAT_INTERVAL_MS must be from 1000 to 5000");
        }
        Ok(Some(Self {
            connection,
            instance: Instance {
                service,
                group,
                cluster,
                ip,
                port,
                metadata,
            },
            heartbeat_interval: Duration::from_millis(interval),
        }))
    }
}

impl Instance {
    fn params(&self, namespace: &str) -> Vec<(String, String)> {
        let mut params = vec![
            ("serviceName".into(), self.service.clone()),
            ("groupName".into(), self.group.clone()),
            ("clusterName".into(), self.cluster.clone()),
            ("ip".into(), self.ip.to_string()),
            ("port".into(), self.port.to_string()),
            ("ephemeral".into(), "true".into()),
        ];
        if !namespace.is_empty() {
            params.push(("namespaceId".into(), namespace.to_owned()));
        }
        params
    }

    async fn change_registration(&self, client: &mut Client, method: Method) -> anyhow::Result<()> {
        let mut params = self.params(&client.settings.namespace);
        if method == Method::POST {
            params.extend([
                ("enabled".into(), "true".into()),
                ("healthy".into(), "true".into()),
                ("weight".into(), "1".into()),
                ("metadata".into(), serde_json::to_string(&self.metadata)?),
            ]);
        }
        let response = client.request(method, "v1/ns/instance", &params).await?;
        if !response.status().is_success() {
            bail!(
                "Nacos instance operation failed with HTTP {}",
                response.status()
            );
        }
        let body = response
            .text()
            .await
            .map_err(reqwest::Error::without_url)
            .context("failed to read Nacos instance response")?;
        if !body.trim().eq_ignore_ascii_case("ok") {
            bail!("Nacos instance operation returned an unexpected response");
        }
        Ok(())
    }

    async fn heartbeat(&self, client: &mut Client) -> anyhow::Result<Option<Duration>> {
        let beat = serde_json::json!({
            "serviceName": format!("{}@@{}", self.group, self.service),
            "ip": self.ip.to_string(), "port": self.port, "cluster": self.cluster,
            "weight": 1.0, "metadata": self.metadata, "scheduled": false,
        });
        let mut params = self.params(&client.settings.namespace);
        params.push(("beat".into(), beat.to_string()));
        let response = client
            .request(Method::PUT, "v1/ns/instance/beat", &params)
            .await?;
        if !response.status().is_success() {
            bail!("Nacos heartbeat failed with HTTP {}", response.status());
        }
        #[derive(Deserialize)]
        struct BeatResponse {
            code: u32,
            #[serde(rename = "clientBeatInterval")]
            interval: Option<u64>,
        }
        let beat: BeatResponse = response
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("invalid Nacos heartbeat response")?;
        match beat.code {
            10200 => Ok(beat
                .interval
                .filter(|ms| *ms > 0)
                .map(|ms| Duration::from_millis(ms.clamp(1000, 5000)))),
            20404 => {
                tracing::warn!(service = %self.service, group = %self.group, "Nacos instance missing; registering again");
                self.change_registration(client, Method::POST).await?;
                tracing::info!(service = %self.service, group = %self.group, ip = %self.ip,
                    port = self.port, "Nacos service registered again");
                Ok(None)
            }
            code => bail!("Nacos heartbeat rejected with code {code}"),
        }
    }
}

/// Keeps an ephemeral instance registered with Nacos. Call `shutdown` before
/// stopping HTTP to deregister immediately. Dropping only stops heartbeats;
/// Nacos then expires the instance according to its server-side timeout.
pub struct Registration {
    client: Arc<Mutex<Client>>,
    instance: Instance,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Registration {
    /// Register using `NACOS_REGISTRY_ENABLED` and `NACOS_*` environment variables.
    /// `address` must be the bound listener's address (including its actual port).
    /// Returns `None` when disabled, and fails startup if enabled registration fails.
    pub async fn start_from_env(
        config: &Config,
        address: SocketAddr,
    ) -> anyhow::Result<Option<Self>> {
        let settings = Settings::from_lookup(
            env,
            &config.string("server.name", "stemson_backend"),
            address,
        )?;
        match settings {
            Some(settings) => Self::start(settings).await.map(Some),
            None => {
                tracing::info!("Nacos service registration is disabled");
                Ok(None)
            }
        }
    }

    async fn start(settings: Settings) -> anyhow::Result<Self> {
        let mut client = Client::new(settings.connection)?;
        let instance = settings.instance;
        instance.change_registration(&mut client, Method::POST).await.with_context(|| format!(
            "Nacos registration failed: server_addr={} service={:?} group={:?} namespace={:?} instance={}:{}",
            client.settings.base_url, instance.service, instance.group, client.settings.namespace_label(), instance.ip, instance.port
        ))?;
        tracing::info!(server_addr = %client.settings.base_url, service = %instance.service,
            group = %instance.group, namespace = %client.settings.namespace_label(), cluster = %instance.cluster,
            ip = %instance.ip, port = instance.port, ephemeral = true, "Nacos service registered");
        let client = Arc::new(Mutex::new(client));
        let task_client = Arc::clone(&client);
        let task_instance = instance.clone();
        let task = tokio::spawn(async move {
            let mut interval = settings.heartbeat_interval;
            let mut failed = false;
            loop {
                tokio::time::sleep(interval).await;
                match task_instance
                    .heartbeat(&mut *task_client.lock().await)
                    .await
                {
                    Ok(server_interval) => {
                        if let Some(server_interval) = server_interval {
                            interval = server_interval;
                        }
                        if failed {
                            tracing::info!(service = %task_instance.service, group = %task_instance.group,
                                ip = %task_instance.ip, port = task_instance.port, "Nacos heartbeat recovered");
                        } else {
                            tracing::debug!(service = %task_instance.service, "Nacos heartbeat sent");
                        }
                        failed = false;
                    }
                    Err(err) => {
                        tracing::warn!(service = %task_instance.service, group = %task_instance.group,
                            ip = %task_instance.ip, port = task_instance.port, error = %err,
                            "Nacos heartbeat failed; retrying on next interval");
                        failed = true;
                    }
                }
            }
        });
        Ok(Self {
            client,
            instance,
            task: Some(task),
        })
    }

    /// Stop heartbeats and deregister. Safe to call repeatedly.
    pub async fn shutdown(&mut self) {
        let Some(task) = self.task.take() else { return };
        task.abort();
        let _ = task.await;
        let mut client = self.client.lock().await;
        match self
            .instance
            .change_registration(&mut client, Method::DELETE)
            .await
        {
            Ok(()) => {
                tracing::info!(service = %self.instance.service, group = %self.instance.group,
                ip = %self.instance.ip, port = self.instance.port, "Nacos service deregistered")
            }
            Err(err) => {
                tracing::warn!(service = %self.instance.service, group = %self.instance.group,
                ip = %self.instance.ip, port = self.instance.port, error = %err,
                "Nacos deregistration failed; ephemeral instance will expire")
            }
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
