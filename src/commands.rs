use crate::config::{ProjectConfig, ResolvedService};
use crate::graph::DependencyGraph;
use crate::resolver;
use anyhow::{bail, Result};
use colored::Colorize;
use std::process::Stdio;

/// Execute a custom command
pub async fn execute_command(
    project: &ProjectConfig,
    name: &str,
    targets: &[String],
    parallel_override: bool,
    json: bool,
) -> Result<()> {
    // Check if it's a workspace-level direct command
    if let Some(cmd_config) = project.workspace.commands.get(name)
        && cmd_config.mode == "direct" {
            return execute_direct_command(project, name, cmd_config, json).await;
        }

    // Service-delegated command
    execute_service_command(project, name, targets, parallel_override, json).await
}

/// Execute a direct (workspace-level) command
async fn execute_direct_command(
    project: &ProjectConfig,
    name: &str,
    config: &crate::config::workspace::CommandConfig,
    json: bool,
) -> Result<()> {
    let run_cmd = config
        .run
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Command '{}' (mode=direct) has no 'run' field", name))?;

    if !json {
        eprintln!("{} {}", "Running:".bold(), run_cmd);
    }

    let status = tokio::process::Command::new("sh")
        .args(["-c", run_cmd])
        .current_dir(&project.root)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        bail!("Command '{}' failed with exit code {:?}", name, status.code());
    }

    if json {
        let result = DirectCommandResult {
            command: name.to_string(),
            status: "ok".to_string(),
        };
        println!("{}", serde_json::to_string(&result)?);
    }

    Ok(())
}

/// Execute a command delegated to matching services
async fn execute_service_command(
    project: &ProjectConfig,
    name: &str,
    targets: &[String],
    parallel_override: bool,
    json: bool,
) -> Result<()> {
    let resolved = resolver::resolve_targets(project, targets)?;

    // Determine execution order
    let cmd_config = project.workspace.commands.get(name);
    let order = if parallel_override {
        "parallel"
    } else {
        cmd_config
            .map(|c| c.order.as_str())
            .unwrap_or("topological")
    };

    // Validate order value
    if !["topological", "parallel", "sequential"].contains(&order) {
        bail!(
            "Invalid command order '{}' for command '{}'. Must be: topological, parallel, or sequential",
            order,
            name
        );
    }

    let fail_fast = cmd_config.map(|c| c.fail_fast).unwrap_or(true);

    // Filter to services that have this command defined (or have a built-in default)
    let services_with_cmd: Vec<&str> = resolved
        .iter()
        .filter(|svc_name| service_has_command(project, svc_name, name))
        .map(|s| s.as_str())
        .collect();

    if services_with_cmd.is_empty() {
        bail!(
            "No services have command '{}' defined. Define it in [service.commands.{}] in the service's forge.toml.",
            name, name
        );
    }

    // Order services
    let ordered = match order {
        "topological" => {
            let graph = DependencyGraph::build(project)?;
            let all_names: Vec<String> = services_with_cmd.iter().map(|s| s.to_string()).collect();
            let topo = graph.topological_order_for(&all_names)?;
            // Filter back to only services that have the command (topological expansion adds dependencies)
            let cmd_set: std::collections::HashSet<&str> =
                services_with_cmd.iter().copied().collect();
            topo.into_iter().filter(|s| cmd_set.contains(s.as_str())).collect::<Vec<_>>()
        }
        _ => services_with_cmd.iter().map(|s| s.to_string()).collect(),
    };

    let mut results: Vec<CommandResult> = Vec::new();

    if order == "parallel" {
        // Parallel execution
        let mut handles = Vec::new();
        for svc_name in &ordered {
            let svc = match project.services.get(svc_name) {
                Some(s) => s.clone(),
                None => continue,
            };
            let cmd_name = name.to_string();
            let root = project.root.clone();
            handles.push(tokio::spawn(async move {
                run_service_command(&svc, &cmd_name, &root).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await?;
            let svc_name = &ordered[i];
            let success = result.is_ok();
            let message = match &result {
                Ok(_) => "ok".to_string(),
                Err(e) => e.to_string(),
            };
            if !json {
                print_service_result(svc_name, name, success);
            }
            results.push(CommandResult {
                service: svc_name.clone(),
                command: name.to_string(),
                success,
                message,
            });
        }
    } else {
        // Sequential / topological execution
        for svc_name in &ordered {
            let svc = match project.services.get(svc_name) {
                Some(s) => s,
                None => continue,
            };
            if !json {
                eprint!("  {} {}...", name.bold(), svc_name);
            }

            let result = run_service_command(svc, name, &project.root).await;
            let success = result.is_ok();
            let message = match &result {
                Ok(_) => "ok".to_string(),
                Err(e) => e.to_string(),
            };

            if !json {
                print_service_result(svc_name, name, success);
            }

            results.push(CommandResult {
                service: svc_name.clone(),
                command: name.to_string(),
                success,
                message,
            });

            if !success && fail_fast {
                break;
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    let failed: Vec<_> = results.iter().filter(|r| !r.success).collect();
    if !failed.is_empty() {
        bail!(
            "{} service(s) failed for command '{}'",
            failed.len(),
            name
        );
    }

    Ok(())
}

/// Check if a service has a given command (custom or built-in default)
fn service_has_command(project: &ProjectConfig, svc_name: &str, cmd_name: &str) -> bool {
    let svc = match project.services.get(svc_name) {
        Some(s) => s,
        None => return false,
    };

    // Check custom commands first
    if svc.config.commands.contains_key(cmd_name) {
        return true;
    }

    // Check explicit config fields
    match cmd_name {
        "build" => svc.config.build.is_some(),
        _ => false,
    }
}

/// Run a command for a single service
async fn run_service_command(
    svc: &ResolvedService,
    cmd_name: &str,
    _workspace_root: &std::path::Path,
) -> Result<()> {
    let cmd_str = get_command_string(svc, cmd_name)?;
    let cwd = if let Some(ref custom_cwd) = svc.config.cwd {
        if std::path::Path::new(custom_cwd).is_absolute() {
            std::path::PathBuf::from(custom_cwd)
        } else {
            svc.dir.join(custom_cwd)
        }
    } else {
        svc.dir.clone()
    };

    let status = tokio::process::Command::new("sh")
        .args(["-c", &cmd_str])
        .current_dir(&cwd)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .envs(&svc.config.env)
        .status()
        .await?;

    if !status.success() {
        bail!(
            "Command '{}' failed for '{}' (exit code {:?})",
            cmd_name,
            svc.name,
            status.code()
        );
    }

    Ok(())
}

/// Resolve the actual shell command to run for a service + command name
fn get_command_string(svc: &ResolvedService, cmd_name: &str) -> Result<String> {
    // 1. Check custom commands
    if let Some(cmd) = svc.config.commands.get(cmd_name) {
        return Ok(cmd.run.clone());
    }

    // 2. Check explicit override fields (build, test via config fields)
    if cmd_name == "build"
        && let Some(ref build_cmd) = svc.config.build {
            return Ok(build_cmd.clone());
        }

    bail!(
        "No command '{}' defined for service '{}'. Add [service.commands.{}] to its forge.toml.",
        cmd_name,
        svc.name,
        cmd_name
    )
}

fn print_service_result(svc_name: &str, cmd_name: &str, success: bool) {
    if success {
        eprintln!("\r  {} {} {}", "+".green(), cmd_name.bold(), svc_name);
    } else {
        eprintln!("\r  {} {} {}", "x".red(), cmd_name.bold(), svc_name);
    }
}

#[derive(Debug, serde::Serialize)]
struct DirectCommandResult {
    command: String,
    status: String,
}

#[derive(Debug, serde::Serialize)]
struct CommandResult {
    service: String,
    command: String,
    success: bool,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        service::{ServiceCommandConfig, ServiceConfig},
        workspace::{WorkspaceConfig, WorkspaceSection},
        ProjectConfig, ResolvedService,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_svc(name: &str, commands: Vec<(&str, &str)>) -> ResolvedService {
        let mut cmd_map = HashMap::new();
        for (k, v) in commands {
            cmd_map.insert(
                k.to_string(),
                ServiceCommandConfig {
                    run: v.to_string(),
                    description: None,
                },
            );
        }
        ResolvedService {
            name: name.to_string(),
            config: ServiceConfig {
                port: None,
                groups: vec![],
                depends_on: vec![],
                health: None,
                env: HashMap::new(),
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
                commands: cmd_map,
            },
            dir: PathBuf::from("/tmp"),
        }
    }

    fn make_project_with(svcs: Vec<ResolvedService>) -> ProjectConfig {
        let services: HashMap<String, ResolvedService> = svcs
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();
        ProjectConfig {
            workspace: WorkspaceConfig {
                workspace: WorkspaceSection {
                    name: "test".to_string(),
                    description: None,
                    zones: None,
                    ignore: None,
                    ignore_override: None,
                    parallel_startup: true,
                    hints: vec![],
                },
                groups: HashMap::new(),
                commands: HashMap::new(),
            },
            services,
            root: PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn test_service_has_command_custom() {
        let svc = make_svc("api", vec![("migrate", "sqlx migrate run")]);
        let project = make_project_with(vec![svc]);
        assert!(service_has_command(&project, "api", "migrate"));
        assert!(!service_has_command(&project, "api", "nonexistent"));
    }

    #[test]
    fn test_service_has_command_build_with_config() {
        let mut svc = make_svc("api", vec![]);
        svc.config.build = Some("cargo build".to_string());
        let project = make_project_with(vec![svc]);
        assert!(service_has_command(&project, "api", "build"));
    }

    #[test]
    fn test_service_has_command_no_build() {
        let svc = make_svc("db", vec![]);
        let project = make_project_with(vec![svc]);
        assert!(!service_has_command(&project, "db", "build"));
    }

    #[test]
    fn test_service_has_command_nonexistent_service() {
        let project = make_project_with(vec![]);
        assert!(!service_has_command(&project, "nonexistent", "build"));
    }

    #[test]
    fn test_get_command_string_custom() {
        let svc = make_svc("api", vec![("migrate", "sqlx migrate run")]);
        assert_eq!(
            get_command_string(&svc, "migrate").unwrap(),
            "sqlx migrate run"
        );
    }

    #[test]
    fn test_get_command_string_build_field() {
        let mut svc = make_svc("api", vec![]);
        svc.config.build = Some("cargo build --release".to_string());
        assert_eq!(
            get_command_string(&svc, "build").unwrap(),
            "cargo build --release"
        );
    }

    #[test]
    fn test_get_command_string_undefined() {
        let svc = make_svc("db", vec![]);
        let result = get_command_string(&svc, "deploy");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No command 'deploy'"));
    }
}
