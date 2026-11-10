//! Configuration management.
//!
//! Rust port of `backend/config/Config.py` and `backend/config/BaseConfig.py`.
//!
//! * YAML file, addressed with dotted keys (`server.http.port`).
//! * `get(key, default)` semantics identical to the Python implementation.
//! * Hot reload: the file is watched and atomically swapped on change
//!   (`notify` + `arc-swap`), so readers never block.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use arc_swap::ArcSwap;
use notify::{Event, RecursiveMode, Watcher};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A live view over the YAML configuration file.
#[derive(Debug)]
pub struct Config {
    path: PathBuf,
    data: ArcSwap<Value>,
}

impl Config {
    /// Load a configuration file.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Arc<Self>> {
        let path = path.as_ref().to_path_buf();
        let data = Self::read_file(&path)?;
        Ok(Arc::new(Self {
            path,
            data: ArcSwap::from_pointee(data),
        }))
    }

    /// Load the configuration path from `STEMSON_CONFIG` or the default
    /// `config/config.yaml`.
    pub fn load_default() -> anyhow::Result<Arc<Self>> {
        let path =
            std::env::var("STEMSON_CONFIG").unwrap_or_else(|_| "config/config.yaml".to_string());
        Self::load(path)
    }

    fn read_file(path: &Path) -> anyhow::Result<Value> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let value: Value = serde_yaml_ng::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        if !value.is_object() {
            return Err(anyhow!(
                "config file {} must contain a YAML mapping at the top level",
                path.display()
            ));
        }
        Ok(value)
    }

    /// Path of the loaded configuration file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Immutable snapshot of the whole configuration tree.
    pub fn snapshot(&self) -> Arc<Value> {
        self.data.load_full()
    }

    /// Re-read the configuration file from disk.
    pub fn reload(&self) -> anyhow::Result<()> {
        let data = Self::read_file(&self.path)?;
        self.data.store(Arc::new(data));
        Ok(())
    }

    /// Get a value by dotted key, e.g. `server.http.port`.
    ///
    /// Mirrors `Config.get(key, default)` of the Python implementation,
    /// including "return the node itself when the path descends into a scalar".
    pub fn get(&self, key: &str) -> Option<Value> {
        let root = self.data.load();
        let mut current: &Value = &root;
        for segment in key.split('.') {
            match current {
                Value::Object(map) => current = map.get(segment)?,
                _ => return Some(current.clone()),
            }
        }
        Some(current.clone())
    }

    /// Get a value or a JSON default.
    pub fn get_or(&self, key: &str, default: Value) -> Value {
        self.get(key).unwrap_or(default)
    }

    /// Get a string value, falling back to `default`.
    pub fn string(&self, key: &str, default: &str) -> String {
        match self.get(key) {
            Some(Value::String(s)) => s,
            Some(Value::Null) | None => default.to_string(),
            Some(other) => other.to_string(),
        }
    }

    /// Get an integer value, falling back to `default`.
    pub fn i64(&self, key: &str, default: i64) -> i64 {
        match self.get(key) {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
            Some(Value::String(s)) => s.parse().unwrap_or(default),
            _ => default,
        }
    }

    /// Get an unsigned 16 bit value (ports), falling back to `default`.
    pub fn u16(&self, key: &str, default: u16) -> u16 {
        let value = self.i64(key, default as i64);
        u16::try_from(value).unwrap_or(default)
    }

    /// Get a boolean value, falling back to `default`.
    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(Value::Bool(b)) => b,
            Some(Value::String(s)) => {
                matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "yes")
            }
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
            _ => default,
        }
    }

    /// Deserialize a configuration subtree into `T`.
    pub fn get_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(|v| serde_json::from_value(v).ok())
    }

    /// Like [`Config::get_as`] but fails hard when the key is missing or invalid.
    /// Intended for `#[value("...")]` injection at startup.
    pub fn require<T: DeserializeOwned>(&self, key: &str) -> anyhow::Result<T> {
        let raw = self
            .get(key)
            .ok_or_else(|| anyhow!("missing config key `{key}` in {}", self.path.display()))?;
        serde_json::from_value(raw)
            .map_err(|e| anyhow!("config key `{key}` has an invalid value: {e}"))
    }

    /// Start watching the configuration file for changes.
    ///
    /// Returns a guard that keeps the watcher alive; drop it to stop watching.
    pub fn watch(self: &Arc<Self>) -> anyhow::Result<ConfigWatcher> {
        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })?;

        let dir = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;

        let config = Arc::clone(self);
        let file_name = self
            .path
            .file_name()
            .map(|n| n.to_os_string())
            .ok_or_else(|| anyhow!("config path has no file name"))?;

        let handle = std::thread::Builder::new()
            .name("stemson-config-watch".to_string())
            .spawn(move || {
                let mut last_reload = std::time::Instant::now();
                for event in rx {
                    let Ok(event) = event else { continue };
                    let relevant = event.paths.iter().any(|p| {
                        p.file_name()
                            .map(|n| n == file_name.as_os_str())
                            .unwrap_or(false)
                    });
                    if !relevant {
                        continue;
                    }
                    // debounce: editors write several events per save
                    if last_reload.elapsed() < Duration::from_millis(200) {
                        continue;
                    }
                    last_reload = std::time::Instant::now();
                    match config.reload() {
                        Ok(()) => tracing::info!(
                            file = %config.path.display(),
                            "configuration reloaded"
                        ),
                        Err(err) => tracing::error!(
                            file = %config.path.display(),
                            error = %err,
                            "configuration reload failed, keeping the previous version"
                        ),
                    }
                }
            })?;

        Ok(ConfigWatcher {
            _watcher: watcher,
            _handle: Some(handle),
        })
    }
}

/// Keeps a configuration watcher alive. Dropping it stops hot reload.
pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
    _handle: Option<std::thread::JoinHandle<()>>,
}

/// Trait mirroring `backend/config/BaseConfig.py`: anything holding a
/// [`Config`] can read dotted keys.
pub trait BaseConfig {
    /// The configuration this object reads from.
    fn config(&self) -> &Arc<Config>;

    /// Get a dotted key, e.g. `self.get("datasource.mysql.host")`.
    fn get(&self, key: &str) -> Option<Value> {
        self.config().get(key)
    }

    /// Get a dotted key with a default.
    fn get_or(&self, key: &str, default: Value) -> Value {
        self.config().get_or(key, default)
    }

    /// Get a string with a default.
    fn get_string(&self, key: &str, default: &str) -> String {
        self.config().string(key, default)
    }

    /// Get an integer with a default.
    fn get_i64(&self, key: &str, default: i64) -> i64 {
        self.config().i64(key, default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(content: &str) -> (tempdir::TempDir, Arc<Config>) {
        let dir = tempdir::TempDir::new();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, content).unwrap();
        (dir, Config::load(&path).unwrap())
    }

    /// Minimal temporary directory helper (avoids a dev-dependency).
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new() -> Self {
                let mut path = std::env::temp_dir();
                let unique = format!(
                    "stemson-test-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                );
                path.push(unique);
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    const SAMPLE: &str = r#"
server:
  name: stemson_backend
  http:
    port: 8080
  log:
    debug: true
datasource:
  mysql:
    host: localhost
    port: 3306
    max_usage: 1000
"#;

    #[test]
    fn dotted_lookup_matches_python_semantics() {
        let (_dir, config) = write_config(SAMPLE);
        assert_eq!(config.i64("server.http.port", 0), 8080);
        assert_eq!(config.string("server.name", ""), "stemson_backend");
        assert_eq!(config.string("server.missing", "fallback"), "fallback");
        assert!(config.bool("server.log.debug", false));
        assert_eq!(config.i64("datasource.mysql.port", 0), 3306);
    }

    #[test]
    fn scalar_nodes_are_returned_as_is() {
        let (_dir, config) = write_config(SAMPLE);
        // descending into a scalar returns the scalar, like Config.get in Python
        assert_eq!(config.i64("server.name.anything", 7), 7);
        assert_eq!(
            config.get("server.http").unwrap(),
            serde_json::json!({"port": 8080})
        );
    }

    #[test]
    fn typed_requirement_reports_missing_keys() {
        let (_dir, config) = write_config(SAMPLE);
        let port: u16 = config.require("server.http.port").unwrap();
        assert_eq!(port, 8080);
        let err = config.require::<String>("nope.nothing").unwrap_err();
        assert!(err.to_string().contains("missing config key"));
    }

    #[test]
    fn reload_picks_up_changes() {
        let (dir, config) = write_config(SAMPLE);
        std::fs::write(
            dir.path().join("config.yaml"),
            SAMPLE.replace("port: 8080", "port: 9090"),
        )
        .unwrap();
        config.reload().unwrap();
        assert_eq!(config.i64("server.http.port", 0), 9090);
    }
}
