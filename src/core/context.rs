//! The application context: configuration, component container, JWT, routes.
//!
//! This is the object the `#[service]`, `#[repository]`, `#[datasource]` and
//! `#[controller]` macros inject from; it plays the role of the module level
//! registries (`DATASOURCE_REGISTRY`, `REPOSITORY_REGISTRY`,
//! `SERVICES_REGISTRY`, `CONTROLLERS_REGISTRY`) of the Python project.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use anyhow::anyhow;

use crate::auth::opaque::{NonceStore, OpaqueAuth};
use crate::auth::{AuthMode, JwtService};
use crate::config::{Config, ConfigWatcher};
use crate::core::registry::{ComponentKind, ComponentRegistration, ComponentValue, Container};
use crate::core::route::{RouteEntry, RouteTable};
use crate::exception::{ApiError, ApiResult};
use crate::model::controller::ControllerBase;

/// Shared application context.
pub struct AppContext {
    config: Arc<Config>,
    container: RwLock<Container>,
    jwt: Arc<JwtService>,
    auth_mode: AuthMode,
    opaque: Option<Arc<OpaqueAuth>>,
    routes: RwLock<RouteTable>,
    controllers: RwLock<Vec<Arc<dyn ControllerBase>>>,
    config_watcher: RwLock<Option<ConfigWatcher>>,
    started_at: Instant,
}

impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppContext")
            .field("config", &self.config.path())
            .field("container", &self.container)
            .finish()
    }
}

impl AppContext {
    /// Create the context for a configuration.
    pub fn new(config: Arc<Config>) -> Arc<Self> {
        let jwt = Arc::new(JwtService::from_config(&config));
        let auth_mode = AuthMode::parse(&config.string("server.auth.mode", "password"));
        let opaque = if auth_mode.allows_opaque() {
            let path = config.string(
                "server.auth.opaque.setup_file",
                "config/opaque_server_setup.bin",
            );
            let ttl = config.i64("server.auth.opaque.session_ttl", 120);
            let nonces = if config.bool("server.auth.opaque.single_use", true) {
                match config
                    .string("server.auth.opaque.nonce_store", "memory")
                    .trim()
                {
                    "redis" => NonceStore::Redis(std::sync::Arc::new(
                        crate::datasource::RedisDataSource::from_config(config.clone()),
                    )),
                    _ => NonceStore::memory(),
                }
            } else {
                NonceStore::Disabled
            };
            match OpaqueAuth::load_or_create(path, ttl, nonces) {
                Ok(auth) => Some(Arc::new(auth)),
                Err(err) => {
                    tracing::error!(error = %err, "cannot initialise OPAQUE authentication");
                    None
                }
            }
        } else {
            None
        };
        Arc::new(Self {
            config,
            container: RwLock::new(Container::default()),
            jwt,
            auth_mode,
            opaque,
            routes: RwLock::new(RouteTable::default()),
            controllers: RwLock::new(Vec::new()),
            config_watcher: RwLock::new(None),
            started_at: Instant::now(),
        })
    }

    /// The live configuration.
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    /// The JWT service.
    pub fn jwt(&self) -> &Arc<JwtService> {
        &self.jwt
    }

    /// Configured authentication mode.
    pub fn auth_mode(&self) -> AuthMode {
        self.auth_mode
    }

    /// The OPAQUE service, if the mode enables it.
    pub fn opaque(&self) -> ApiResult<&Arc<OpaqueAuth>> {
        self.opaque.as_ref().ok_or_else(|| {
            crate::exception::ApiError::invalid_parameter(
                "OPAQUE authentication is not enabled on this server",
            )
        })
    }

    /// Time since the context was created.
    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Enable hot reload for the configuration file.
    pub fn watch_config(self: &Arc<Self>) -> anyhow::Result<()> {
        let watcher = self.config.watch()?;
        *self
            .config_watcher
            .write()
            .map_err(|_| anyhow!("config watcher lock poisoned"))? = Some(watcher);
        Ok(())
    }

    /// Build every registered component, layer by layer.
    ///
    /// Order matters and mirrors the Python injection chain: datasource →
    /// repository → service → controller. Within a layer, components are built
    /// in name order so startup is deterministic.
    pub fn build_components(&self) -> anyhow::Result<()> {
        let mut registrations: Vec<&'static ComponentRegistration> =
            inventory::iter::<ComponentRegistration>().collect();
        registrations.sort_by(|a, b| {
            a.kind
                .order()
                .cmp(&b.kind.order())
                .then_with(|| a.name.cmp(b.name))
        });

        for registration in registrations {
            let value = (registration.build)(self).map_err(|err| {
                anyhow!(
                    "failed to build {} `{}` ({}): {err:#}",
                    registration.kind.as_str(),
                    registration.name,
                    registration.type_name
                )
            })?;
            if let Some(cast) = registration.as_controller {
                if let Some(controller) = cast(Arc::clone(&value)) {
                    self.controllers
                        .write()
                        .map_err(|_| anyhow!("controller registry lock poisoned"))?
                        .push(controller);
                }
            }
            self.container
                .write()
                .map_err(|_| anyhow!("component container lock poisoned"))?
                .insert(registration.name, registration.kind, value)
                .map_err(|err| {
                    anyhow!(
                        "failed to register {} `{}`: {err}",
                        registration.kind.as_str(),
                        registration.name
                    )
                })?;
            tracing::debug!(
                kind = registration.kind.as_str(),
                name = registration.name,
                "registered component"
            );
        }
        Ok(())
    }

    /// Resolve a component by type, failing with a startup error message.
    ///
    /// Used by the generated `Autowire` implementations.
    pub fn resolve_component<T: std::any::Any + Send + Sync>(&self) -> anyhow::Result<Arc<T>> {
        self.try_resolve::<T>().ok_or_else(|| {
            anyhow!(
                "component `{}` is not registered; it must be built in an earlier layer \
                 (datasource -> repository -> service -> controller). Did you add                  #[service] / #[repository] / #[datasource]?",
                std::any::type_name::<T>()
            )
        })
    }

    /// Resolve a component by type, or fail with a 500.
    pub fn resolve<T: std::any::Any + Send + Sync>(&self) -> ApiResult<Arc<T>> {
        self.try_resolve::<T>().ok_or_else(|| {
            ApiError::internal(format!(
                "component `{}` is not registered; did you forget #[service] / #[repository] / #[controller]?",
                std::any::type_name::<T>()
            ))
        })
    }

    /// Resolve a component by type.
    pub fn try_resolve<T: std::any::Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.container.read().ok()?.get::<T>()
    }

    /// Resolve a component by its registry name.
    pub fn resolve_by_name(&self, name: &str) -> Option<ComponentValue> {
        self.container.read().ok()?.get_by_name(name)
    }

    /// Every registered component, as `(kind, name)` pairs.
    pub fn component_names(&self) -> Vec<(ComponentKind, &'static str)> {
        self.container
            .read()
            .map(|container| container.names())
            .unwrap_or_default()
    }

    /// Number of registered components.
    pub fn component_count(&self) -> usize {
        self.container.read().map(|c| c.len()).unwrap_or_default()
    }

    /// Every built controller, in registration order.
    pub fn controllers(&self) -> Vec<Arc<dyn ControllerBase>> {
        self.controllers
            .read()
            .map(|controllers| controllers.clone())
            .unwrap_or_default()
    }

    /// Record routes discovered while assembling the router.
    pub fn register_routes(&self, entries: impl IntoIterator<Item = RouteEntry>) {
        if let Ok(mut routes) = self.routes.write() {
            routes.extend(entries);
        }
    }

    /// Snapshot of the registered routes.
    pub fn routes(&self) -> Vec<RouteEntry> {
        self.routes
            .read()
            .map(|routes| routes.entries().to_vec())
            .unwrap_or_default()
    }

    /// Read a configuration value for `#[value("...")]` injection.
    pub fn inject_value<T: serde::de::DeserializeOwned>(&self, key: &str) -> anyhow::Result<T> {
        self.config
            .require(key)
            .map_err(|err| anyhow!("cannot inject configuration value for `{key}`: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_jwt_are_exposed() {
        let dir = std::env::temp_dir().join(format!("stemson-ctx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "server:\n  jwt:\n    expires_in: 600\n").unwrap();
        let config = Config::load(&path).unwrap();
        let ctx = AppContext::new(config);
        assert_eq!(ctx.jwt().ttl_seconds(), 600);
        assert_eq!(ctx.component_count(), 0);
        assert!(ctx.routes().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
