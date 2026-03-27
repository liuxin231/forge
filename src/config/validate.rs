use super::ProjectConfig;
use anyhow::{bail, Result};

const VALID_COMMAND_MODES: &[&str] = &["direct", "service"];
const VALID_COMMAND_ORDERS: &[&str] = &["topological", "parallel", "sequential"];

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
                commands: HashMap::new(),
            },
            dir: PathBuf::from("/tmp/test"),
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
}
