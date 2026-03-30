use super::ProjectConfig;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

const VALID_COMMAND_MODES: &[&str] = &["direct", "service"];
const VALID_COMMAND_ORDERS: &[&str] = &["topological", "parallel", "sequential"];

// ── Known field sets for unknown-field detection ──────────────────────────────

const WORKSPACE_TOP_LEVEL: &[&str] = &["workspace", "groups", "commands"];
const WORKSPACE_SECTION_FIELDS: &[&str] = &[
    "name", "description", "zones", "ignore", "ignore_override",
    "parallel_startup", "hints", "env",
];
const WORKSPACE_HINT_FIELDS: &[&str] = &["title", "items"];
const WORKSPACE_HINT_ITEM_FIELDS: &[&str] = &["label", "value"];
const GROUP_FIELDS: &[&str] = &["description", "includes", "services"];
const COMMAND_FIELDS: &[&str] = &["description", "mode", "run", "order", "fail_fast"];

const SERVICE_TOP_LEVEL: &[&str] = &["service", "lib"];
const SERVICE_FIELDS: &[&str] = &[
    "port", "groups", "depends_on", "health", "env", "env_file",
    "up", "down", "build", "dev", "logs", "cwd", "args",
    "autorestart", "max_restarts", "restart_delay", "kill_timeout",
    "treekill", "attach", "max_memory", "mode", "commands",
];
const HEALTH_FIELDS: &[&str] = &["http", "cmd", "interval", "timeout"];
const SERVICE_COMMAND_FIELDS: &[&str] = &["run", "description", "inputs", "outputs"];

// ── Validation issue types ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum IssueLevel {
    Error,
    Warning,
}

impl std::fmt::Display for IssueLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueLevel::Error => write!(f, "error"),
            IssueLevel::Warning => write!(f, "warning"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub level: IssueLevel,
    /// TOML path where the issue was found, e.g. "service.health.command"
    pub path: String,
    pub message: String,
}

#[derive(Debug)]
pub struct FileValidationResult {
    /// Path relative to workspace root
    pub relative_path: PathBuf,
    pub issues: Vec<ValidationIssue>,
}

impl FileValidationResult {
    pub fn errors(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues.iter().filter(|i| i.level == IssueLevel::Error)
    }
    pub fn warnings(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues.iter().filter(|i| i.level == IssueLevel::Warning)
    }
}

// ── Levenshtein "did you mean?" ───────────────────────────────────────────────

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m {
        dp[i][0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1])
            };
        }
    }
    dp[m][n]
}

fn did_you_mean(unknown: &str, known: &[&str]) -> Option<String> {
    let threshold = (unknown.len() / 2 + 1).max(2);
    let mut best: Option<(&str, usize)> = None;
    for &k in known {
        let dist = levenshtein(unknown, k);
        if dist <= threshold {
            if best.map_or(true, |(_, d)| dist < d) {
                best = Some((k, dist));
            }
        }
    }
    best.map(|(k, _)| k.to_string())
}

// ── Unknown field checkers ────────────────────────────────────────────────────

fn check_unknown_keys(
    table: &toml::Table,
    known: &[&str],
    path_prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            let path = if path_prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", path_prefix, key)
            };
            let suggestion = did_you_mean(key, known);
            let message = match suggestion {
                Some(s) => format!("unknown field '{}' — did you mean '{}'?", key, s),
                None => format!("unknown field '{}'", key),
            };
            issues.push(ValidationIssue {
                level: IssueLevel::Error,
                path,
                message,
            });
        }
    }
}

fn check_service_config(
    svc: &toml::Table,
    path_prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    check_unknown_keys(svc, SERVICE_FIELDS, path_prefix, issues);

    if let Some(toml::Value::Table(health)) = svc.get("health") {
        let prefix = format!("{}.health", path_prefix);
        check_unknown_keys(health, HEALTH_FIELDS, &prefix, issues);
    }

    if let Some(toml::Value::Table(commands)) = svc.get("commands") {
        for (name, val) in commands {
            if let toml::Value::Table(c) = val {
                let prefix = format!("{}.commands.{}", path_prefix, name);
                check_unknown_keys(c, SERVICE_COMMAND_FIELDS, &prefix, issues);
            }
        }
    }
}

/// Detect unknown fields in a workspace-level forge.toml.
pub fn detect_unknown_workspace_fields(content: &str) -> Vec<ValidationIssue> {
    let table = match content.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let mut issues = vec![];

    check_unknown_keys(&table, WORKSPACE_TOP_LEVEL, "", &mut issues);

    if let Some(toml::Value::Table(ws)) = table.get("workspace") {
        check_unknown_keys(ws, WORKSPACE_SECTION_FIELDS, "workspace", &mut issues);

        if let Some(toml::Value::Array(hints)) = ws.get("hints") {
            for (i, hint) in hints.iter().enumerate() {
                if let toml::Value::Table(h) = hint {
                    let prefix = format!("workspace.hints[{}]", i);
                    check_unknown_keys(h, WORKSPACE_HINT_FIELDS, &prefix, &mut issues);
                    if let Some(toml::Value::Array(items)) = h.get("items") {
                        for (j, item) in items.iter().enumerate() {
                            if let toml::Value::Table(it) = item {
                                let prefix =
                                    format!("workspace.hints[{}].items[{}]", i, j);
                                check_unknown_keys(
                                    it,
                                    WORKSPACE_HINT_ITEM_FIELDS,
                                    &prefix,
                                    &mut issues,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(toml::Value::Table(groups)) = table.get("groups") {
        for (name, val) in groups {
            if let toml::Value::Table(g) = val {
                let prefix = format!("groups.{}", name);
                check_unknown_keys(g, GROUP_FIELDS, &prefix, &mut issues);
            }
        }
    }

    if let Some(toml::Value::Table(commands)) = table.get("commands") {
        for (name, val) in commands {
            if let toml::Value::Table(c) = val {
                let prefix = format!("commands.{}", name);
                check_unknown_keys(c, COMMAND_FIELDS, &prefix, &mut issues);
            }
        }
    }

    issues
}

/// Detect unknown fields in a service-level forge.toml.
pub fn detect_unknown_service_fields(content: &str) -> Vec<ValidationIssue> {
    let table = match content.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let mut issues = vec![];

    check_unknown_keys(&table, SERVICE_TOP_LEVEL, "", &mut issues);

    if let Some(toml::Value::Table(svc)) = table.get("service") {
        // Same heuristic as service.rs: presence of 'port' or 'up' → single service
        let is_single = svc.contains_key("port") || svc.contains_key("up");
        if is_single {
            check_service_config(svc, "service", &mut issues);
        } else {
            for (name, val) in svc {
                if let toml::Value::Table(sub) = val {
                    let prefix = format!("service.{}", name);
                    check_service_config(sub, &prefix, &mut issues);
                }
            }
        }
    }

    issues
}

/// Check for service-level warnings: env_file not found.
pub fn check_service_warnings(
    service_name: &str,
    service_dir: &Path,
    env_file: Option<&str>,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(env_file_path) = env_file {
        let full_path = if Path::new(env_file_path).is_absolute() {
            PathBuf::from(env_file_path)
        } else {
            service_dir.join(env_file_path)
        };
        if !full_path.exists() {
            issues.push(ValidationIssue {
                level: IssueLevel::Warning,
                path: "service.env_file".to_string(),
                message: format!(
                    "service '{}': env_file '{}' not found",
                    service_name, env_file_path
                ),
            });
        }
    }
}

/// Check for port conflicts across all services.
pub fn check_port_conflicts(project: &ProjectConfig) -> Vec<ValidationIssue> {
    use std::collections::HashMap;
    let mut port_map: HashMap<u16, Vec<&str>> = HashMap::new();
    for (name, svc) in &project.services {
        if let Some(port) = svc.config.port {
            port_map.entry(port).or_default().push(name.as_str());
        }
    }
    let mut issues = vec![];
    for (port, names) in &port_map {
        if names.len() > 1 {
            let mut sorted = names.to_vec();
            sorted.sort();
            issues.push(ValidationIssue {
                level: IssueLevel::Warning,
                path: "service.port".to_string(),
                message: format!(
                    "port {} is used by multiple services: {}",
                    port,
                    sorted.join(", ")
                ),
            });
        }
    }
    issues
}

/// Validate a loaded project configuration for semantic correctness.
/// Called after parsing, before any operations.
pub fn validate(project: &ProjectConfig) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    validate_workspace(&project.workspace, &mut errors);
    validate_services(project, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "Configuration validation failed:\n  - {}",
            errors.join("\n  - ")
        );
    }
}

fn validate_workspace(ws: &super::workspace::WorkspaceConfig, errors: &mut Vec<String>) {
    // Workspace name must not be empty
    if ws.workspace.name.trim().is_empty() {
        errors.push("workspace.name must not be empty".to_string());
    }

    // Validate workspace-level commands
    for (name, cmd) in &ws.commands {
        if !VALID_COMMAND_MODES.contains(&cmd.mode.as_str()) {
            errors.push(format!(
                "commands.{}.mode: invalid value '{}', must be one of: {}",
                name,
                cmd.mode,
                VALID_COMMAND_MODES.join(", ")
            ));
        }
        if !VALID_COMMAND_ORDERS.contains(&cmd.order.as_str()) {
            errors.push(format!(
                "commands.{}.order: invalid value '{}', must be one of: {}",
                name,
                cmd.order,
                VALID_COMMAND_ORDERS.join(", ")
            ));
        }
        if cmd.mode == "direct" && cmd.run.is_none() {
            errors.push(format!(
                "commands.{}: mode=direct requires 'run' field",
                name
            ));
        }
    }
}

fn validate_services(project: &ProjectConfig, errors: &mut Vec<String>) {
    for (name, svc) in &project.services {
        let cfg = &svc.config;

        // up is required
        if cfg.up.is_none() {
            errors.push(format!(
                "service '{}': 'up' field is required",
                name
            ));
        }

        // Port
        if cfg.port == Some(0) {
            errors.push(format!("service '{}': port must not be 0", name));
        }

        // depends_on validation
        for dep in &cfg.depends_on {
            if dep.trim().is_empty() {
                errors.push(format!(
                    "service '{}': depends_on contains empty string",
                    name
                ));
            }
            if dep == name {
                errors.push(format!("service '{}': depends_on contains self-reference", name));
            }
            if !project.services.contains_key(dep) && !dep.trim().is_empty() {
                errors.push(format!(
                    "service '{}': depends_on references undefined service '{}'",
                    name, dep
                ));
            }
        }

        // Check for duplicate depends_on entries
        let mut seen_deps = std::collections::HashSet::new();
        for dep in &cfg.depends_on {
            if !seen_deps.insert(dep) {
                errors.push(format!(
                    "service '{}': duplicate depends_on entry '{}'",
                    name, dep
                ));
            }
        }

        // Health config validation
        if let Some(health) = &cfg.health {
            if health.http.is_some() && health.cmd.is_some() {
                errors.push(format!(
                    "service '{}': health.http and health.cmd are mutually exclusive",
                    name
                ));
            }
            if health.http.is_none() && health.cmd.is_none() {
                errors.push(format!(
                    "service '{}': health config present but neither http nor cmd is set",
                    name
                ));
            }
            if let Some(http_path) = &health.http
                && !http_path.starts_with('/') {
                    errors.push(format!(
                        "service '{}': health.http path must start with '/', got '{}'",
                        name, http_path
                    ));
                }
            if let Some(cmd) = &health.cmd {
                use crate::config::service::HealthCmd;
                let is_empty = match cmd {
                    HealthCmd::Shell(s) => s.trim().is_empty(),
                    HealthCmd::Exec(argv) => argv.is_empty(),
                };
                if is_empty {
                    errors.push(format!("service '{}': health.cmd must not be empty", name));
                }
            }
            if health.interval == 0 {
                errors.push(format!(
                    "service '{}': health.interval must be >= 1",
                    name
                ));
            }
            if health.timeout == 0 {
                errors.push(format!(
                    "service '{}': health.timeout must be >= 1",
                    name
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        service::{HealthCmd, HealthConfig, ServiceConfig},
        workspace::{CommandConfig, WorkspaceConfig, WorkspaceSection},
        ProjectConfig, ResolvedService,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_workspace(name: &str) -> WorkspaceConfig {
        WorkspaceConfig {
            workspace: WorkspaceSection {
                name: name.to_string(),
                description: None,
                zones: None,
                ignore: None,
                ignore_override: None,
                parallel_startup: true,
                hints: vec![],
                env: HashMap::new(),
            },
            groups: HashMap::new(),
            commands: HashMap::new(),
        }
    }

    fn make_service(name: &str) -> ResolvedService {
        ResolvedService {
            name: name.to_string(),
            config: ServiceConfig {
                port: Some(8080),
                groups: vec![],
                depends_on: vec![],
                health: None,
                env: HashMap::new(),
                env_file: None,
                up: Some("echo hi".to_string()),
                down: None,
                build: None,
                dev: None,
                logs: None,
                cwd: None,
                args: None,
                autorestart: true,
                max_restarts: 10,
                restart_delay: 3,
                kill_timeout: 10,
                treekill: true,
                attach: false,
                max_memory: None,
                mode: crate::config::ServiceMode::Service,
                commands: HashMap::new(),
            },            dir: PathBuf::from("/tmp/test"),
        }
    }

    fn make_project(services: Vec<(&str, ResolvedService)>) -> ProjectConfig {
        ProjectConfig {
            workspace: make_workspace("test"),
            services: services.into_iter().map(|(n, s)| (n.to_string(), s)).collect(),
            root: PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn test_valid_project() {
        let project = make_project(vec![("api", make_service("api"))]);
        assert!(validate(&project).is_ok());
    }

    #[test]
    fn test_empty_workspace_name() {
        let mut project = make_project(vec![("api", make_service("api"))]);
        project.workspace.workspace.name = "".to_string();
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("workspace.name must not be empty"));
    }

    #[test]
    fn test_whitespace_workspace_name() {
        let mut project = make_project(vec![("api", make_service("api"))]);
        project.workspace.workspace.name = "   ".to_string();
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("workspace.name must not be empty"));
    }

    #[test]
    fn test_missing_up() {
        let mut svc = make_service("api");
        svc.config.up = None;
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("'up' field is required"));
    }

    #[test]
    fn test_port_zero() {
        let mut svc = make_service("api");
        svc.config.port = Some(0);
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("port must not be 0"));
    }

    #[test]
    fn test_port_none_is_valid() {
        let mut svc = make_service("api");
        svc.config.port = None;
        let project = make_project(vec![("api", svc)]);
        assert!(validate(&project).is_ok());
    }

    #[test]
    fn test_depends_on_empty_string() {
        let mut svc = make_service("api");
        svc.config.depends_on = vec!["".to_string()];
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("empty string"));
    }

    #[test]
    fn test_depends_on_self_reference() {
        let mut svc = make_service("api");
        svc.config.depends_on = vec!["api".to_string()];
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("self-reference"));
    }

    #[test]
    fn test_depends_on_undefined_service() {
        let mut svc = make_service("api");
        svc.config.depends_on = vec!["nonexistent".to_string()];
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("undefined service 'nonexistent'"));
    }

    #[test]
    fn test_depends_on_duplicate() {
        let mut api = make_service("api");
        api.config.depends_on = vec!["db".to_string(), "db".to_string()];
        let db = make_service("db");
        let project = make_project(vec![("api", api), ("db", db)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("duplicate depends_on"));
    }

    #[test]
    fn test_health_http_and_cmd_mutually_exclusive() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: Some("/health".to_string()),
            cmd: Some(HealthCmd::Shell("curl localhost".to_string())),

            interval: 2,
            timeout: 60,
        });
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn test_health_neither_http_nor_cmd() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: None,
            cmd: None,

            interval: 2,
            timeout: 60,
        });
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("neither http nor cmd"));
    }

    #[test]
    fn test_health_http_no_leading_slash() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: Some("health".to_string()),
            cmd: None,

            interval: 2,
            timeout: 60,
        });
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("must start with '/'"));
    }

    #[test]
    fn test_health_empty_cmd() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: None,
            cmd: Some(HealthCmd::Shell("  ".to_string())),

            interval: 2,
            timeout: 60,
        });
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_health_interval_zero() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: Some("/health".to_string()),
            cmd: None,

            interval: 0,
            timeout: 60,
        });
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("interval must be >= 1"));
    }

    #[test]
    fn test_health_timeout_zero() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: Some("/health".to_string()),
            cmd: None,

            interval: 2,
            timeout: 0,
        });
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("timeout must be >= 1"));
    }

    #[test]
    fn test_valid_health_config() {
        let mut svc = make_service("api");
        svc.config.health = Some(HealthConfig {
            http: Some("/healthz".to_string()),
            cmd: None,

            interval: 2,
            timeout: 60,
        });
        let project = make_project(vec![("api", svc)]);
        assert!(validate(&project).is_ok());
    }

    #[test]
    fn test_invalid_command_mode() {
        let mut project = make_project(vec![("api", make_service("api"))]);
        project.workspace.commands.insert(
            "deploy".to_string(),
            CommandConfig {
                description: None,
                mode: "foobar".to_string(),
                run: Some("echo deploy".to_string()),
                order: "topological".to_string(),
                fail_fast: true,
            },
        );
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("invalid value 'foobar'"));
    }

    #[test]
    fn test_direct_command_without_run() {
        let mut project = make_project(vec![("api", make_service("api"))]);
        project.workspace.commands.insert(
            "deploy".to_string(),
            CommandConfig {
                description: None,
                mode: "direct".to_string(),
                run: None,
                order: "topological".to_string(),
                fail_fast: true,
            },
        );
        let err = validate(&project).unwrap_err();
        assert!(err.to_string().contains("mode=direct requires 'run'"));
    }

    #[test]
    fn test_multiple_errors_collected() {
        let mut svc = make_service("api");
        svc.config.up = None;
        svc.config.port = Some(0);
        let project = make_project(vec![("api", svc)]);
        let err = validate(&project).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'up' field is required"));
        assert!(msg.contains("port must not be 0"));
    }

    // ── unknown field detection tests ─────────────────────────────────────────

    #[test]
    fn test_detect_unknown_workspace_field() {
        let content = r#"
[workspace]
name = "test"
type = "monorepo"
"#;
        let issues = detect_unknown_workspace_fields(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "workspace.type");
        assert!(issues[0].message.contains("unknown field 'type'"));
    }

    #[test]
    fn test_detect_unknown_top_level_workspace_key() {
        let content = r#"
[workspace]
name = "test"

[something]
foo = "bar"
"#;
        let issues = detect_unknown_workspace_fields(content);
        let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"something"));
    }

    #[test]
    fn test_detect_no_issues_valid_workspace() {
        let content = r#"
[workspace]
name = "test"
parallel_startup = true

[commands.migrate]
mode = "service"
order = "topological"

[groups.backend]
services = ["api"]
"#;
        let issues = detect_unknown_workspace_fields(content);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_detect_unknown_service_field() {
        let content = r#"
[service]
port = 8080
up = "cargo run"
type = "command"
"#;
        let issues = detect_unknown_service_fields(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "service.type");
        assert!(issues[0].message.contains("unknown field 'type'"));
    }

    #[test]
    fn test_detect_unknown_health_field() {
        let content = r#"
[service]
port = 8080
up = "cargo run"

[service.health]
command = "curl /health"
"#;
        let issues = detect_unknown_service_fields(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "service.health.command");
        assert!(issues[0].message.contains("did you mean 'cmd'"));
    }

    #[test]
    fn test_detect_no_issues_valid_service() {
        let content = r#"
[service]
port = 8080
up = "cargo run"
depends_on = ["postgres"]
autorestart = true

[service.health]
http = "/healthz"
interval = 2

[service.commands.migrate]
run = "sqlx migrate run"
inputs = ["migrations/**/*.sql"]
"#;
        let issues = detect_unknown_service_fields(content);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_detect_unknown_multi_service_field() {
        let content = r#"
[service.api]
port = 8080
up = "cargo run"
type = "command"

[service.worker]
up = "cargo run --bin worker"
"#;
        let issues = detect_unknown_service_fields(content);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "service.api.type");
    }

    #[test]
    fn test_levenshtein_did_you_mean() {
        // "command" should suggest "cmd"
        let suggestion = did_you_mean("command", HEALTH_FIELDS);
        assert_eq!(suggestion.as_deref(), Some("cmd"));
    }

    #[test]
    fn test_did_you_mean_no_suggestion_far_field() {
        // "xyz_totally_different_field" should not suggest anything
        let suggestion = did_you_mean("xyz_totally_different_field", SERVICE_FIELDS);
        assert!(suggestion.is_none());
    }

    #[test]
    fn test_port_conflict_detection() {
        let mut api = make_service("api");
        api.config.port = Some(8080);
        let mut web = make_service("web");
        web.config.port = Some(8080);
        let project = make_project(vec![("api", api), ("web", web)]);
        let issues = check_port_conflicts(&project);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].level, IssueLevel::Warning);
        assert!(issues[0].message.contains("8080"));
        assert!(issues[0].message.contains("api"));
        assert!(issues[0].message.contains("web"));
    }

    #[test]
    fn test_no_port_conflict_different_ports() {
        let mut api = make_service("api");
        api.config.port = Some(8080);
        let mut db = make_service("db");
        db.config.port = Some(5432);
        let project = make_project(vec![("api", api), ("db", db)]);
        let issues = check_port_conflicts(&project);
        assert!(issues.is_empty());
    }
}
