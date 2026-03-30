use crate::config::{ProjectConfig, ResolvedService};
use crate::config::service::HealthCmd;
use crate::graph::DependencyGraph;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

// ─── Runtime types (used by tui::dag) ──────────────────────────

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeStatus {
    Running,
    Stopped,
    Unknown,
}

impl std::fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeStatus::Running => write!(f, "running"),
            RuntimeStatus::Stopped => write!(f, "stopped"),
            RuntimeStatus::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub status: RuntimeStatus,
    pub port: Option<u16>,
}

pub fn detect_all_runtime_info(project: &ProjectConfig) -> HashMap<String, RuntimeInfo> {
    project
        .services
        .keys()
        .map(|name| {
            let info = detect_service_runtime(project, name);
            (name.clone(), info)
        })
        .collect()
}

fn detect_service_runtime(project: &ProjectConfig, name: &str) -> RuntimeInfo {
    let safe_name = crate::process::runner::sanitize_service_name(name);
    let pid_file = project.root.join(format!(".forge/pids/{}.pid", safe_name));

    let pid_str = match std::fs::read_to_string(&pid_file) {
        Ok(s) => s,
        Err(_) => {
            return RuntimeInfo {
                status: RuntimeStatus::Unknown,
                port: None,
            }
        }
    };

    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            return RuntimeInfo {
                status: RuntimeStatus::Unknown,
                port: None,
            }
        }
    };

    if !crate::process::platform::is_process_alive(pid) {
        return RuntimeInfo {
            status: RuntimeStatus::Stopped,
            port: None,
        };
    }

    let port = crate::process::platform::detect_listening_ports(pid)
        .into_iter()
        .next();

    RuntimeInfo {
        status: RuntimeStatus::Running,
        port,
    }
}

// ─── Project-level inspect ─────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ProjectInspect {
    pub workspace: WorkspaceInfo,
    pub services: Vec<ServiceSummary>,
    pub groups: HashMap<String, GroupInfo>,
    pub commands: HashMap<String, CommandInfo>,
    pub topology: Vec<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub root: String,
}

#[derive(Debug, Serialize)]
pub struct ServiceSummary {
    pub name: String,
    pub dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub depends_on: Vec<String>,
    pub groups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthInfo>,
    pub commands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    pub interval: u64,
    pub timeout: u64,
}

#[derive(Debug, Serialize)]
pub struct GroupInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub services: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CommandInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub order: String,
    pub fail_fast: bool,
}

// ─── Service-level inspect ─────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServiceInspect {
    pub name: String,
    pub dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub up: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    pub depends_on: Vec<String>,
    pub depended_by: Vec<String>,
    pub transitive_deps: Vec<String>,
    pub groups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthInfo>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    pub commands: HashMap<String, ServiceCmdInfo>,
    pub restart: RestartInfo,
}

#[derive(Debug, Serialize)]
pub struct ServiceCmdInfo {
    pub run: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RestartInfo {
    pub autorestart: bool,
    pub max_restarts: u32,
    pub restart_delay: u64,
    pub kill_timeout: u64,
    pub treekill: bool,
}

// ─── Build functions ───────────────────────────────────────────

fn build_health_info(svc: &ResolvedService) -> Option<HealthInfo> {
    let h = svc.config.health.as_ref()?;
    let cmd_str = h.cmd.as_ref().map(|c| match c {
        HealthCmd::Shell(s) => s.clone(),
        HealthCmd::Exec(v) => v.join(" "),
    });
    Some(HealthInfo {
        http: h.http.clone(),
        cmd: cmd_str,
        interval: h.interval,
        timeout: h.timeout,
    })
}

fn relative_dir(project: &ProjectConfig, svc: &ResolvedService) -> String {
    svc.dir
        .strip_prefix(&project.root)
        .unwrap_or(&svc.dir)
        .to_string_lossy()
        .to_string()
}

pub fn build_project_inspect(project: &ProjectConfig) -> Result<ProjectInspect> {
    let dep_graph = DependencyGraph::build(project)?;
    let all_names: Vec<String> = {
        let mut names: Vec<String> = project.services.keys().cloned().collect();
        names.sort();
        names
    };
    let topology = dep_graph.topological_levels_for(&all_names)?;

    let mut services: Vec<ServiceSummary> = project
        .services
        .iter()
        .map(|(name, svc)| {
            let cmd_names: Vec<String> = {
                let mut keys: Vec<String> = svc.config.commands.keys().cloned().collect();
                keys.sort();
                keys
            };
            ServiceSummary {
                name: name.clone(),
                dir: relative_dir(project, svc),
                port: svc.config.port,
                depends_on: {
                    let mut deps = svc.config.depends_on.clone();
                    deps.sort();
                    deps
                },
                groups: {
                    let mut g = svc.config.groups.clone();
                    g.sort();
                    g
                },
                health: build_health_info(svc),
                commands: cmd_names,
            }
        })
        .collect();
    services.sort_by(|a, b| a.name.cmp(&b.name));

    let groups: HashMap<String, GroupInfo> = project
        .workspace
        .groups
        .iter()
        .map(|(name, g)| {
            (
                name.clone(),
                GroupInfo {
                    description: g.description.clone(),
                    services: {
                        let mut s = g.services.clone();
                        s.sort();
                        s
                    },
                },
            )
        })
        .collect();

    let commands: HashMap<String, CommandInfo> = project
        .workspace
        .commands
        .iter()
        .map(|(name, c)| {
            (
                name.clone(),
                CommandInfo {
                    description: c.description.clone(),
                    mode: c.mode.clone(),
                    run: c.run.clone(),
                    order: c.order.clone(),
                    fail_fast: c.fail_fast,
                },
            )
        })
        .collect();

    Ok(ProjectInspect {
        workspace: WorkspaceInfo {
            name: project.workspace.workspace.name.clone(),
            description: project.workspace.workspace.description.clone(),
            root: project.root.to_string_lossy().to_string(),
        },
        services,
        groups,
        commands,
        topology,
    })
}

pub fn build_service_inspect(project: &ProjectConfig, name: &str) -> Result<ServiceInspect> {
    let svc = project
        .services
        .get(name)
        .ok_or_else(|| {
            let mut available: Vec<String> = project.services.keys().cloned().collect();
            available.sort();
            anyhow::anyhow!(
                "Service '{}' not found. Available services: {}",
                name,
                available.join(", ")
            )
        })?;

    // Direct reverse dependencies (who depends on this service)
    let mut depended_by: Vec<String> = project
        .services
        .iter()
        .filter(|(_, s)| s.config.depends_on.contains(&name.to_string()))
        .map(|(n, _)| n.clone())
        .collect();
    depended_by.sort();

    // Transitive dependencies
    let dep_graph = DependencyGraph::build(project)?;
    let all_transitive = dep_graph.topological_order_for(&[name.to_string()])?;
    let mut transitive_deps: Vec<String> = all_transitive
        .into_iter()
        .filter(|n| n != name)
        .collect();
    transitive_deps.sort();

    let commands: HashMap<String, ServiceCmdInfo> = svc
        .config
        .commands
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                ServiceCmdInfo {
                    run: v.run.clone(),
                    description: v.description.clone(),
                },
            )
        })
        .collect();

    Ok(ServiceInspect {
        name: name.to_string(),
        dir: relative_dir(project, svc),
        port: svc.config.port,
        up: svc.config.up.clone(),
        down: svc.config.down.clone(),
        build: svc.config.build.clone(),
        dev: svc.config.dev.clone(),
        logs: svc.config.logs.clone(),
        cwd: svc.config.cwd.clone(),
        args: svc.config.args.clone(),
        depends_on: {
            let mut deps = svc.config.depends_on.clone();
            deps.sort();
            deps
        },
        depended_by,
        transitive_deps,
        groups: {
            let mut g = svc.config.groups.clone();
            g.sort();
            g
        },
        health: build_health_info(svc),
        env: svc.config.env.clone(),
        commands,
        restart: RestartInfo {
            autorestart: svc.config.autorestart,
            max_restarts: svc.config.max_restarts,
            restart_delay: svc.config.restart_delay,
            kill_timeout: svc.config.kill_timeout,
            treekill: svc.config.treekill,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        service::ServiceConfig,
        workspace::{WorkspaceConfig, WorkspaceSection},
        ResolvedService,
    };
    use std::path::PathBuf;

    fn make_svc(name: &str, deps: Vec<&str>, port: Option<u16>) -> ResolvedService {
        ResolvedService {
            name: name.to_string(),
            config: ServiceConfig {
                port,
                groups: vec!["backend".to_string()],
                depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
                health: None,
                env: HashMap::new(),
                env_file: None,
                up: Some("echo".to_string()),
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
            },
            dir: PathBuf::from("/tmp/project").join(name),
        }
    }

    fn make_project(svcs: Vec<(&str, Vec<&str>, Option<u16>)>) -> ProjectConfig {
        let services: HashMap<String, ResolvedService> = svcs
            .into_iter()
            .map(|(name, deps, port)| (name.to_string(), make_svc(name, deps, port)))
            .collect();
        ProjectConfig {
            workspace: WorkspaceConfig {
                workspace: WorkspaceSection {
                    name: "test-project".to_string(),
                    description: Some("A test project".to_string()),
                    zones: None,
                    ignore: None,
                    ignore_override: None,
                    parallel_startup: true,
                    hints: vec![],
                    env: HashMap::new(),
                },
                groups: HashMap::new(),
                commands: HashMap::new(),
            },
            services,
            root: PathBuf::from("/tmp/project"),
        }
    }

    #[test]
    fn test_build_project_inspect() {
        let project = make_project(vec![
            ("api", vec!["db"], Some(8080)),
            ("db", vec![], Some(5432)),
            ("web", vec!["api"], Some(3000)),
        ]);
        let result = build_project_inspect(&project).unwrap();

        assert_eq!(result.workspace.name, "test-project");
        assert_eq!(result.services.len(), 3);
        // Services should be sorted by name
        assert_eq!(result.services[0].name, "api");
        assert_eq!(result.services[1].name, "db");
        assert_eq!(result.services[2].name, "web");
        // Topology: db -> api -> web
        assert_eq!(result.topology.len(), 3);
        assert_eq!(result.topology[0], vec!["db"]);
        assert_eq!(result.topology[1], vec!["api"]);
        assert_eq!(result.topology[2], vec!["web"]);
    }

    #[test]
    fn test_build_service_inspect() {
        let project = make_project(vec![
            ("api", vec!["db"], Some(8080)),
            ("db", vec![], Some(5432)),
            ("web", vec!["api"], Some(3000)),
        ]);
        let result = build_service_inspect(&project, "api").unwrap();

        assert_eq!(result.name, "api");
        assert_eq!(result.port, Some(8080));
        assert_eq!(result.depends_on, vec!["db"]);
        assert_eq!(result.depended_by, vec!["web"]);
        assert_eq!(result.transitive_deps, vec!["db"]);
        assert_eq!(result.groups, vec!["backend"]);
    }

    #[test]
    fn test_build_service_inspect_not_found() {
        let project = make_project(vec![("api", vec![], None)]);
        let result = build_service_inspect(&project, "nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("api"));
    }

    #[test]
    fn test_depended_by() {
        let project = make_project(vec![
            ("db", vec![], None),
            ("api", vec!["db"], None),
            ("worker", vec!["db"], None),
            ("web", vec!["api"], None),
        ]);
        let result = build_service_inspect(&project, "db").unwrap();
        assert_eq!(result.depended_by, vec!["api", "worker"]);
    }

    #[test]
    fn test_transitive_deps() {
        let project = make_project(vec![
            ("db", vec![], None),
            ("auth", vec!["db"], None),
            ("api", vec!["auth"], None),
        ]);
        let result = build_service_inspect(&project, "api").unwrap();
        assert_eq!(result.transitive_deps, vec!["auth", "db"]);
    }

    #[test]
    fn test_relative_dir() {
        let project = make_project(vec![("api", vec![], None)]);
        let svc = project.services.get("api").unwrap();
        assert_eq!(relative_dir(&project, svc), "api");
    }
}
