use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

/// How the supervisor treats process exit.
///
/// - `service` (default): long-running process; restart on exit.
/// - `oneshot`: launcher that exits 0 immediately after starting an external
///   daemon (e.g. `docker compose up -d`). Forge keeps the service marked
///   as *running* and relies on the health check for liveness; the process
///   is never restarted on successful exit.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceMode {
    #[default]
    Service,
    Oneshot,
}

/// Service-level forge.toml — supports both single [service] and multi [service.xxx]
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceFile {
    #[serde(default)]
    pub service: Option<toml::Value>,
    /// Lib section — if present, this is a library, not a service
    #[serde(default)]
    pub lib: Option<toml::Value>,
}

impl ServiceFile {
    /// Parse the service field into either a single config or multi config
    pub fn parse_services(&self) -> Option<ServiceConfigOrMulti> {
        let value = self.service.as_ref()?;

        // Try to parse as single service first by checking for known fields
        if let Some(table) = value.as_table() {
            // If it has known service fields at top level, it's a single service
            if table.contains_key("port") || table.contains_key("up") {
                if let Ok(config) = value.clone().try_into::<ServiceConfig>() {
                    return Some(ServiceConfigOrMulti::Single(Box::new(config)));
                }
                // Log parse failure instead of silently ignoring
                tracing::warn!(
                    "Service section has known fields but failed to parse as ServiceConfig"
                );
            }

            // Otherwise try as multi-service (map of name -> config)
            let mut map = HashMap::new();
            for (key, val) in table {
                if let Some(sub_table) = val.as_table() {
                    // Only treat as a service if it has known service fields
                    let has_service_fields = sub_table.contains_key("port")
                        || sub_table.contains_key("up");
                    if has_service_fields {
                        match val.clone().try_into::<ServiceConfig>() {
                            Ok(config) => {
                                map.insert(key.clone(), config);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse sub-service '{}': {}",
                                    key,
                                    e
                                );
                            }
                        }
                    }
                }
            }
            if !map.is_empty() {
                return Some(ServiceConfigOrMulti::Multi(map));
            }
        }

        None
    }
}

/// Represents either a single service or multiple named services
#[derive(Debug, Clone)]
pub enum ServiceConfigOrMulti {
    Single(Box<ServiceConfig>),
    Multi(HashMap<String, ServiceConfig>),
}

#[allow(dead_code)]
/// Configuration for a single service
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    #[serde(default)]
    pub port: Option<u16>,

    #[serde(default)]
    pub groups: Vec<String>,

    #[serde(default)]
    pub depends_on: Vec<String>,

    #[serde(default)]
    pub health: Option<HealthConfig>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Path to a .env file to load (relative to service dir or absolute)
    #[serde(default)]
    pub env_file: Option<String>,

    #[serde(default)]
    pub up: Option<String>,

    #[serde(default)]
    pub down: Option<String>,

    #[serde(default)]
    pub build: Option<String>,

    #[serde(default)]
    pub dev: Option<String>,

    #[serde(default)]
    pub logs: Option<String>,

    #[serde(default)]
    pub cwd: Option<String>,

    #[serde(default)]
    pub args: Option<String>,

    #[serde(default = "default_autorestart")]
    pub autorestart: bool,

    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,

    #[serde(default = "default_restart_delay")]
    pub restart_delay: u64,

    #[serde(default = "default_kill_timeout")]
    pub kill_timeout: u64,

    #[serde(default = "default_treekill")]
    pub treekill: bool,

    /// Whether this service should run in foreground (attach to terminal) by default
    #[serde(default)]
    pub attach: bool,

    #[serde(default)]
    pub max_memory: Option<String>,

    /// How forge treats process exit: `service` (default) = long-running + restart;
    /// `oneshot` = launcher exits immediately, daemon managed externally.
    #[serde(default)]
    pub mode: ServiceMode,

    /// Custom commands defined on this service
    #[serde(default)]
    pub commands: HashMap<String, ServiceCommandConfig>,
}

#[allow(dead_code)]
/// A custom command defined on a service
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceCommandConfig {
    pub run: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Glob patterns (relative to service dir) whose file contents determine the cache key.
    /// If empty, caching is disabled for this command.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Paths/globs produced by this command (informational, reserved for future artifact management).
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// Health check command: either a shell string or an argv array.
///
/// ```toml
/// cmd = "pg_isready -h localhost"          # sh -c "..."
/// cmd = ["pg_isready", "-h", "localhost"]  # exec directly, no shell
/// ```
#[derive(Debug, Clone)]
pub enum HealthCmd {
    /// Passed to `sh -c`. Supports shell syntax, pipes, env vars.
    Shell(String),
    /// Executed directly via execvp. argv[0] is the binary.
    Exec(Vec<String>),
}

impl<'de> Deserialize<'de> for HealthCmd {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = HealthCmd;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a string or array of strings")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<HealthCmd, E> {
                Ok(HealthCmd::Shell(v.to_string()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<HealthCmd, E> {
                Ok(HealthCmd::Shell(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<HealthCmd, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<String>()? {
                    items.push(item);
                }
                Ok(HealthCmd::Exec(items))
            }
        }
        d.deserialize_any(Visitor)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    #[serde(default)]
    pub http: Option<String>,
    #[serde(default)]
    pub cmd: Option<HealthCmd>,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_autorestart() -> bool {
    true
}
fn default_max_restarts() -> u32 {
    10
}
fn default_restart_delay() -> u64 {
    3
}
fn default_kill_timeout() -> u64 {
    10
}
fn default_treekill() -> bool {
    true
}
fn default_interval() -> u64 {
    2
}
fn default_timeout() -> u64 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_service() {
        let toml_str = r#"
[service]
port = 8080
up = "cargo run"
depends_on = ["postgres"]
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        let result = file.parse_services().unwrap();
        match result {
            ServiceConfigOrMulti::Single(cfg) => {
                assert_eq!(cfg.port, Some(8080));
                assert_eq!(cfg.depends_on, vec!["postgres"]);
                assert_eq!(cfg.up, Some("cargo run".to_string()));
            }
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_parse_multi_service() {
        let toml_str = r#"
[service.postgres]
up = "docker compose up postgres"
port = 5432

[service.redis]
up = "docker compose up redis"
port = 6379
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        let result = file.parse_services().unwrap();
        match result {
            ServiceConfigOrMulti::Multi(map) => {
                assert!(map.contains_key("postgres"));
                assert!(map.contains_key("redis"));
                assert_eq!(map["postgres"].port, Some(5432));
                assert_eq!(map["redis"].port, Some(6379));
            }
            _ => panic!("Expected Multi"),
        }
    }

    #[test]
    fn test_parse_no_service_section() {
        let toml_str = r#"
[lib]
path = "src/lib.rs"
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        assert!(file.parse_services().is_none());
    }

    #[test]
    fn test_service_defaults() {
        let toml_str = r#"
[service]
up = "echo hi"
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        let result = file.parse_services().unwrap();
        match result {
            ServiceConfigOrMulti::Single(cfg) => {
                assert!(cfg.autorestart);
                assert_eq!(cfg.max_restarts, 10);
                assert_eq!(cfg.restart_delay, 3);
                assert_eq!(cfg.kill_timeout, 10);
                assert!(cfg.treekill);
                assert!(!cfg.attach);
                assert!(cfg.port.is_none());
            }
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_service_with_health_config() {
        let toml_str = r#"
[service]
port = 8080
up = "cargo run"

[service.health]
http = "/healthz"
interval = 5
timeout = 30
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        match file.parse_services().unwrap() {
            ServiceConfigOrMulti::Single(cfg) => {
                let health = cfg.health.unwrap();
                assert_eq!(health.http, Some("/healthz".to_string()));
                assert!(health.cmd.is_none());
                assert_eq!(health.interval, 5);
                assert_eq!(health.timeout, 30);
            }
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_service_with_custom_commands() {
        let toml_str = r#"
[service]
port = 8080
up = "cargo run"

[service.commands.migrate]
run = "sqlx migrate run"
description = "Run database migrations"
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        match file.parse_services().unwrap() {
            ServiceConfigOrMulti::Single(cfg) => {
                assert!(cfg.commands.contains_key("migrate"));
                assert_eq!(cfg.commands["migrate"].run, "sqlx migrate run");
            }
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_service_with_env() {
        let toml_str = r#"
[service]
port = 8080
up = "cargo run"

[service.env]
RUST_LOG = "info"
DATABASE_URL = "postgres://localhost/db"
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        match file.parse_services().unwrap() {
            ServiceConfigOrMulti::Single(cfg) => {
                assert_eq!(cfg.env["RUST_LOG"], "info");
                assert_eq!(cfg.env["DATABASE_URL"], "postgres://localhost/db");
            }
            _ => panic!("Expected Single"),
        }
    }

    #[test]
    fn test_empty_service_section() {
        let toml_str = r#"
[service]
"#;
        let file: ServiceFile = toml::from_str(toml_str).unwrap();
        // Empty service table -> no known fields -> returns None
        assert!(file.parse_services().is_none());
    }
}
