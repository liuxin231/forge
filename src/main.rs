mod cli;
mod commands;
mod config;
mod graph;
mod init;
mod inspect;
mod log;
mod output;
mod process;
mod resolver;
mod supervisor;
mod tui;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = cli::Cli::parse();

    if let Some(dir) = &cli.directory {
        std::env::set_current_dir(dir)
            .map_err(|e| anyhow::anyhow!("Failed to change directory to {}: {}", dir.display(), e))?;
    }

    match cli.command {
        cli::Command::Up {
            targets,
            attach,
            json,
        } => {
            cmd_up(targets, attach, json).await?;
        }
        cli::Command::Down { targets, json } => {
            cmd_down(targets, json).await?;
        }
        cli::Command::Restart { targets, json } => {
            cmd_restart(targets, json).await?;
        }
        cli::Command::Ps { targets, json } => {
            cmd_ps(targets, json).await?;
        }
        cli::Command::Logs {
            targets,
            tail,
            follow,
            json,
        } => {
            cmd_logs(targets, tail, follow, json).await?;
        }
        cli::Command::Run {
            name,
            targets,
            parallel,
            json,
        } => {
            cmd_run(&name, targets, parallel, json).await?;
        }
        cli::Command::Graph { targets } => {
            cmd_graph(targets)?;
        }
        cli::Command::Init { path } => {
            init::run(path)?;
        }
        cli::Command::Supervisor { workspace_root } => {
            supervisor::daemon::run_as_daemon(&workspace_root).await?;
        }
        cli::Command::External(args) => {
            let name = &args[0];
            let mut targets = vec![];
            let mut json = false;
            let mut parallel = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "--json" => json = true,
                    "--parallel" => parallel = true,
                    _ if !arg.starts_with('-') => targets.push(arg.clone()),
                    _ => anyhow::bail!("Unknown flag '{}' for command '{}'", arg, name),
                }
            }
            cmd_run(name, targets, parallel, json).await?;
        }
    }

    Ok(())
}

async fn cmd_up(targets: Vec<String>, attach: Option<Vec<String>>, json: bool) -> Result<()> {
    use supervisor::protocol::{Request, Response};

    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;
    let resolved = resolver::resolve_targets(&project, &targets)?;
    let dep_graph = graph::DependencyGraph::build(&project)?;
    let levels = dep_graph.topological_levels_for(&resolved)?;
    let start_order: Vec<String> = levels.iter().flatten().cloned().collect();
    let parallel = project.workspace.workspace.parallel_startup;

    process::check_port_conflicts(&project, &start_order)?;

    match attach {
        None => {
            let mut client = supervisor::ensure_supervisor(&workspace_root, &project).await?;

            if json {
                let response = client.send(Request::Up(start_order)).await?;
                output::print_up_result(&response, json)?;
            } else {
                let mut list = output::LiveList::new(start_order.clone());
                list.render();

                for level in &levels {
                    for name in level {
                        list.set_starting(name);
                    }
                    list.render();

                    let response = client.send(Request::Up(level.clone())).await?;

                    match response {
                        Response::Services(statuses) => {
                            for s in &statuses {
                                match s.health {
                                    supervisor::protocol::HealthStatus::Healthy => {
                                        list.set_healthy(&s.name, s.port);
                                    }
                                    _ => list.set_unhealthy(&s.name),
                                }
                            }
                            list.render();
                        }
                        Response::Error(e) => {
                            for name in level {
                                list.set_failed(name);
                            }
                            list.render();
                            list.print_summary("up");
                            anyhow::bail!("Failed to start services: {}", e);
                        }
                        _ => {}
                    }
                }

                // Clear live table, query full status, print final rich table
                list.clear();
                let full_response = client.send(Request::Status(vec![])).await?;
                if let Response::Services(svcs) = full_response {
                    let statuses_map: std::collections::HashMap<_, _> =
                        svcs.into_iter().map(|s| (s.name.clone(), s)).collect();
                    output::print_up_final_table(
                        &start_order,
                        &statuses_map,
                        &list.elapsed_secs(),
                        &project,
                    )?;
                }

                list.print_summary("up");
                output::print_hints(&project.workspace.workspace.hints);
            }
        }
        Some(attach_targets) => {
            let attach_set = resolve_attach_set(&project, &start_order, &attach_targets)?;
            supervisor::run_mixed_mode(&project, &levels, &attach_set, parallel, json).await?;
        }
    }

    Ok(())
}

/// Resolve which services should be attached to terminal
fn resolve_attach_set(
    project: &config::ProjectConfig,
    start_order: &[String],
    attach_targets: &[String],
) -> Result<std::collections::HashSet<String>> {
    use std::collections::HashSet;

    if !attach_targets.is_empty() {
        // Explicit: --attach gateway/api iam/api
        let resolved = resolver::resolve_targets(project, attach_targets)?;
        return Ok(resolved.into_iter().collect());
    }

    // No explicit targets: use services with attach=true in config
    let from_config: HashSet<String> = start_order
        .iter()
        .filter(|name| {
            project
                .services
                .get(*name)
                .map(|s| s.config.attach)
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    if !from_config.is_empty() {
        return Ok(from_config);
    }

    // No config either: attach all
    Ok(start_order.iter().cloned().collect())
}

async fn cmd_down(targets: Vec<String>, json: bool) -> Result<()> {
    use supervisor::protocol::{Request, Response};

    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;

    let resolved = if targets.is_empty() {
        project.services.keys().cloned().collect::<Vec<_>>()
    } else {
        resolver::resolve_targets(&project, &targets)?
    };

    let mut client = supervisor::connect_supervisor(&workspace_root).await?;

    if json {
        let to_stop = if targets.is_empty() { vec![] } else { resolved };
        let response = client.send(Request::Down(to_stop)).await?;
        output::print_down_result(&response, json)?;
    } else {
        // Reverse topo levels: dependents stop first, dependencies last
        let dep_graph = graph::DependencyGraph::build(&project)?;
        let forward_levels = dep_graph.topological_levels_for(&resolved)?;
        let reverse_levels: Vec<Vec<String>> = forward_levels.into_iter().rev().collect();

        // Display order follows reverse topo so the list shows stop order visually
        let stop_order: Vec<String> = reverse_levels.iter().flatten().cloned().collect();
        let mut list = output::LiveList::new(stop_order);
        list.render();

        for level in &reverse_levels {
            for name in level {
                list.set_stopping(name);
            }
            list.render();

            let response = client.send(Request::Down(level.clone())).await?;

            match response {
                Response::Ok => {
                    for name in level {
                        list.set_stopped(name);
                    }
                    list.render();
                }
                Response::Error(e) => {
                    for name in level {
                        list.set_failed(name);
                    }
                    list.render();
                    list.print_summary("down");
                    anyhow::bail!("Failed to stop services: {}", e);
                }
                _ => {}
            }
        }

        list.print_summary("down");
    }

    Ok(())
}

async fn cmd_restart(targets: Vec<String>, json: bool) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;
    let resolved = resolver::resolve_targets(&project, &targets)?;

    let mut client = supervisor::connect_supervisor(&workspace_root).await?;
    let response = client
        .send(supervisor::protocol::Request::Restart(resolved))
        .await?;
    output::print_restart_result(&response, json)?;

    Ok(())
}

async fn cmd_ps(targets: Vec<String>, json: bool) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;

    let resolved = if targets.is_empty() {
        vec![]
    } else {
        resolver::resolve_targets(&project, &targets)?
    };

    let mut client = supervisor::connect_supervisor(&workspace_root).await?;
    let response = client
        .send(supervisor::protocol::Request::Status(resolved))
        .await?;
    output::print_ps_result(&response, json)?;
    if !json {
        output::print_hints(&project.workspace.workspace.hints);
    }

    Ok(())
}

async fn cmd_logs(
    targets: Vec<String>,
    tail: usize,
    follow: bool,
    json: bool,
) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;
    let resolved = resolver::resolve_targets(&project, &targets)?;

    let mut client = supervisor::connect_supervisor(&workspace_root).await?;
    let response = client
        .send(supervisor::protocol::Request::Logs {
            services: resolved.clone(),
            tail,
            follow,
        })
        .await?;

    match response {
        supervisor::protocol::Response::LogLines(lines) => {
            output::print_log_lines(&lines, json)?;
        }
        supervisor::protocol::Response::LogStream => {
            let label = if resolved.is_empty() {
                "all services".to_string()
            } else {
                resolved.join(", ")
            };
            client.stream_logs(json, if json { None } else { Some(label) }).await?;
            if !json {
                eprintln!();
            }
        }
        supervisor::protocol::Response::Error(e) => {
            anyhow::bail!("{}", e);
        }
        _ => {}
    }

    Ok(())
}

async fn cmd_run(name: &str, targets: Vec<String>, parallel: bool, json: bool) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;
    commands::execute_command(&project, name, &targets, parallel, json).await
}

fn cmd_graph(targets: Vec<String>) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;
    let filtered = if targets.is_empty() {
        project.clone()
    } else {
        let seed: std::collections::HashSet<String> =
            resolver::resolve_targets(&project, &targets)?.into_iter().collect();
        let mut closure = seed.clone();
        let mut frontier: Vec<String> = seed.into_iter().collect();
        while let Some(name) = frontier.pop() {
            if let Some(svc) = project.services.get(&name) {
                for dep in &svc.config.depends_on {
                    if closure.insert(dep.clone()) {
                        frontier.push(dep.clone());
                    }
                }
            }
        }
        let mut p = project.clone();
        p.services.retain(|name, _| closure.contains(name));
        p
    };
    let runtime = inspect::detect_all_runtime_info(&filtered);
    let levels = {
        let all: Vec<String> = filtered.services.keys().cloned().collect();
        graph::DependencyGraph::build(&filtered)?
            .topological_levels_for(&all)?
    };
    let avail_w = terminal_width();
    let output = tui::dag::render_dag_ansi(&filtered, &runtime, &levels, avail_w);
    print!("{}", output);
    Ok(())
}

fn terminal_width() -> usize {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        if let Ok((w, _)) = crossterm::terminal::size() {
            return (w as usize).saturating_sub(2);
        }
    }
    78
}

/// Find workspace root by searching for forge.toml upward.
/// Uses TOML parsing to verify it's a workspace config, not just string matching.
fn find_workspace_root() -> Result<std::path::PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        let candidate = dir.join("forge.toml");
        if candidate.is_file()
            && is_workspace_forge_toml(&candidate) {
                return Ok(dir);
            }
        if !dir.pop() {
            anyhow::bail!(
                "No workspace forge.toml found. Run this command from a forge workspace directory."
            );
        }
    }
}

/// Check if a forge.toml file contains a [workspace] section by parsing it as TOML.
fn is_workspace_forge_toml(path: &std::path::Path) -> bool {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    match content.parse::<toml::Table>() {
        Ok(table) => table.contains_key("workspace"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_workspace_forge_toml_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forge.toml");
        std::fs::write(&path, "[workspace]\nname = \"test\"").unwrap();
        assert!(is_workspace_forge_toml(&path));
    }

    #[test]
    fn test_is_workspace_forge_toml_service_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forge.toml");
        std::fs::write(&path, "[service]\ntype = \"command\"\nup = \"echo\"").unwrap();
        assert!(!is_workspace_forge_toml(&path));
    }

    #[test]
    fn test_is_workspace_forge_toml_commented() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forge.toml");
        std::fs::write(&path, "# [workspace]\n[service]\ntype = \"command\"\nup = \"echo\"").unwrap();
        assert!(!is_workspace_forge_toml(&path));
    }

    #[test]
    fn test_is_workspace_forge_toml_nonexistent() {
        assert!(!is_workspace_forge_toml(std::path::Path::new("/nonexistent/forge.toml")));
    }
}
