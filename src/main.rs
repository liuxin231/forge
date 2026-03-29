mod cache;
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
mod upgrade;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = cli::Cli::parse();
    let verbose = cli.verbose;

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
            dry_run,
            concurrency,
            since,
            json,
        } => {
            cmd_run(&name, targets, parallel, dry_run, concurrency, since, json, verbose).await?;
        }
        cli::Command::Exec { service, cmd } => {
            cmd_exec(&service, cmd).await?;
        }
        cli::Command::Inspect { target, json } => {
            cmd_inspect(target, json)?;
        }
        cli::Command::Graph { targets, json } => {
            cmd_graph(targets, json)?;
        }
        cli::Command::Init { path, name, description, parallel } => {
            init::run(init::InitOptions { path, name, description, parallel })?;
        }
        cli::Command::Uninstall => {
            cmd_uninstall()?;
        }
        cli::Command::Validate { json } => {
            cmd_validate(json)?;
        }
        cli::Command::Upgrade { check } => {
            upgrade::run(check).await?;
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
            cmd_run(name, targets, parallel, false, None, None, json, verbose).await?;
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

    let mut client = match supervisor::connect_supervisor(&workspace_root).await {
        Ok(c) => c,
        Err(_) => {
            if json {
                println!("{}", serde_json::to_string(&serde_json::json!({"status": "ok", "message": "No services running"}))?);
            } else {
                eprintln!("No services are running.");
            }
            return Ok(());
        }
    };

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

    let mut client = match supervisor::connect_supervisor(&workspace_root).await {
        Ok(c) => c,
        Err(_) => {
            anyhow::bail!("No services are running. Start services with 'fr up' first.");
        }
    };
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

    let response = match supervisor::connect_supervisor(&workspace_root).await {
        Ok(mut client) => client
            .send(supervisor::protocol::Request::Status(resolved))
            .await?,
        Err(_) => {
            // No supervisor running — build a static "stopped" list from config
            let service_names: Vec<&String> = if resolved.is_empty() {
                let mut names: Vec<&String> = project.services.keys().collect();
                names.sort();
                names
            } else {
                resolved.iter().collect()
            };
            let statuses: Vec<supervisor::protocol::ServiceStatus> = service_names
                .into_iter()
                .map(|name| {
                    let port = project
                        .services
                        .get(name)
                        .and_then(|s| s.config.port);
                    supervisor::protocol::ServiceStatus {
                        name: name.clone(),
                        port,
                        status: supervisor::protocol::ProcessStatus::Stopped,
                        health: supervisor::protocol::HealthStatus::None,
                        pid: None,
                        restarts: 0,
                    }
                })
                .collect();
            supervisor::protocol::Response::Services(statuses)
        }
    };

    output::print_ps_result(&response, json, &project)?;
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

    let mut client = match supervisor::connect_supervisor(&workspace_root).await {
        Ok(c) => c,
        Err(_) => {
            anyhow::bail!("No services are running. Start services with 'fr up' first.");
        }
    };
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

async fn cmd_run(
    name: &str,
    targets: Vec<String>,
    parallel: bool,
    dry_run: bool,
    concurrency: Option<usize>,
    since: Option<String>,
    json: bool,
    verbose: u8,
) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;
    commands::execute_command(
        &project,
        name,
        &targets,
        commands::RunOptions {
            parallel,
            dry_run,
            concurrency,
            since,
            verbose,
            json,
        },
    )
    .await
}

fn cmd_uninstall() -> Result<()> {
    use colored::Colorize;

    let current_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot determine current executable path: {}", e))?;

    let forge_home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .join(".forge");

    eprintln!("{}", "forge uninstaller".bold());
    eprintln!();

    // Remove binary
    if current_exe.exists() {
        std::fs::remove_file(&current_exe)
            .map_err(|e| anyhow::anyhow!("Failed to remove {}: {} (try sudo?)", current_exe.display(), e))?;
        eprintln!("{} Removed: {}", "✓".green().bold(), current_exe.display());
    }

    // Remove backups
    let backup_dir = forge_home.join("backup");
    if backup_dir.is_dir() {
        let _ = std::fs::remove_dir_all(&backup_dir);
        eprintln!("{} Removed backups", "✓".green().bold());
    }

    eprintln!();
    eprintln!("Optional cleanup:");
    if forge_home.is_dir() {
        eprintln!("  rm -rf {}", forge_home.display());
    }

    // Hint about PATH cleanup
    let home = dirs::home_dir().unwrap_or_default();
    for rc in &[".zshrc", ".bashrc", ".bash_profile"] {
        let rc_path = home.join(rc);
        if rc_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&rc_path) {
                if content.contains(".forge/bin") {
                    eprintln!("  Remove '.forge/bin' line from {}", rc_path.display());
                }
            }
        }
    }

    eprintln!();
    eprintln!("{} forge has been uninstalled.", "✓".green().bold());

    Ok(())
}

async fn cmd_exec(service: &str, cmd: Vec<String>) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;

    // Resolve to exactly one service
    let resolved = resolver::resolve_targets(&project, &[service.to_string()])?;
    if resolved.len() != 1 {
        anyhow::bail!(
            "'fr exec' requires a single service, but '{}' resolved to {} services: {}",
            service,
            resolved.len(),
            resolved.join(", ")
        );
    }

    let svc_name = &resolved[0];
    let svc = project
        .services
        .get(svc_name)
        .ok_or_else(|| anyhow::anyhow!("Service '{}' not found", svc_name))?;

    let cwd = if let Some(ref custom_cwd) = svc.config.cwd {
        if std::path::Path::new(custom_cwd).is_absolute() {
            std::path::PathBuf::from(custom_cwd)
        } else {
            svc.dir.join(custom_cwd)
        }
    } else {
        svc.dir.clone()
    };

    let cmd_str = cmd.join(" ");
    let status = tokio::process::Command::new("sh")
        .args(["-c", &cmd_str])
        .current_dir(&cwd)
        .envs(&svc.config.env)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .stdin(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute command: {}", e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn cmd_inspect(target: Option<String>, json: bool) -> Result<()> {
    let workspace_root = find_workspace_root()?;
    let project = config::load_project(&workspace_root)?;

    match target.filter(|s| !s.is_empty()) {
        Some(name) => {
            let detail = inspect::build_service_inspect(&project, &name)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                output::print_service_inspect(&detail);
            }
        }
        None => {
            let overview = inspect::build_project_inspect(&project)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&overview)?);
            } else {
                output::print_project_inspect(&overview);
            }
        }
    }

    Ok(())
}

fn cmd_validate(json: bool) -> Result<()> {
    use config::validate::{
        check_port_conflicts, check_service_warnings, detect_unknown_service_fields,
        detect_unknown_workspace_fields, FileValidationResult, IssueLevel, ValidationIssue,
    };

    let workspace_root = find_workspace_root()?;

    // ── Step 1: collect all forge.toml paths ─────────────────────────────────
    let forge_tomls = collect_forge_tomls(&workspace_root)?;

    // ── Step 2: unknown field detection on raw TOML ───────────────────────────
    let mut file_results: Vec<FileValidationResult> = vec![];
    for path in &forge_tomls {
        let relative = path
            .strip_prefix(&workspace_root)
            .unwrap_or(path.as_path())
            .to_path_buf();
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
        let is_workspace = content
            .parse::<toml::Table>()
            .map(|t| t.contains_key("workspace"))
            .unwrap_or(false);
        let mut issues = if is_workspace {
            detect_unknown_workspace_fields(&content)
        } else {
            detect_unknown_service_fields(&content)
        };

        // Service-level env_file warning requires knowing the service dir
        if !is_workspace {
            let service_dir = path.parent().unwrap_or(workspace_root.as_path());
            // Parse env_file from raw TOML if present
            if let Ok(table) = content.parse::<toml::Table>() {
                if let Some(toml::Value::Table(svc)) = table.get("service") {
                    let is_single = svc.contains_key("port") || svc.contains_key("up");
                    if is_single {
                        let env_file = svc
                            .get("env_file")
                            .and_then(|v| v.as_str());
                        check_service_warnings("(file)", service_dir, env_file, &mut issues);
                    } else {
                        for (name, val) in svc {
                            if let toml::Value::Table(sub) = val {
                                let env_file =
                                    sub.get("env_file").and_then(|v| v.as_str());
                                check_service_warnings(
                                    name,
                                    service_dir,
                                    env_file,
                                    &mut issues,
                                );
                            }
                        }
                    }
                }
            }
        }

        file_results.push(FileValidationResult {
            relative_path: relative,
            issues,
        });
    }

    // ── Step 3: load project for semantic validation + port conflicts ─────────
    let mut semantic_issues: Vec<ValidationIssue> = vec![];
    let load_error: Option<String> = match config::load_project(&workspace_root) {
        Ok(project) => {
            // Port conflicts (warnings)
            semantic_issues.extend(check_port_conflicts(&project));
            None
        }
        Err(e) => Some(e.to_string()),
    };

    // ── Step 4: output ────────────────────────────────────────────────────────
    let total_errors: usize = file_results
        .iter()
        .flat_map(|f| f.errors())
        .count()
        + semantic_issues
            .iter()
            .filter(|i| i.level == IssueLevel::Error)
            .count()
        + usize::from(load_error.is_some());
    let total_warnings: usize = file_results
        .iter()
        .flat_map(|f| f.warnings())
        .count()
        + semantic_issues
            .iter()
            .filter(|i| i.level == IssueLevel::Warning)
            .count();

    if json {
        let files_json: Vec<serde_json::Value> = file_results
            .iter()
            .map(|fr| {
                serde_json::json!({
                    "path": fr.relative_path.display().to_string(),
                    "issues": fr.issues.iter().map(|i| serde_json::json!({
                        "level": i.level.to_string(),
                        "path": i.path,
                        "message": i.message,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();

        let mut semantic_json: Vec<serde_json::Value> = semantic_issues
            .iter()
            .map(|i| {
                serde_json::json!({
                    "level": i.level.to_string(),
                    "path": i.path,
                    "message": i.message,
                })
            })
            .collect();
        if let Some(ref err) = load_error {
            semantic_json.push(serde_json::json!({
                "level": "error",
                "path": "",
                "message": err,
            }));
        }

        let output = serde_json::json!({
            "valid": total_errors == 0,
            "errors": total_errors,
            "warnings": total_warnings,
            "files": files_json,
            "semantic": semantic_json,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        for fr in &file_results {
            let path_str = fr.relative_path.display().to_string();
            if fr.issues.is_empty() {
                println!("{} ✓", path_str);
            } else {
                let e = fr.errors().count();
                let w = fr.warnings().count();
                let summary = match (e, w) {
                    (0, w) => format!("{} warning{}", w, if w == 1 { "" } else { "s" }),
                    (e, 0) => format!("{} error{}", e, if e == 1 { "" } else { "s" }),
                    (e, w) => format!(
                        "{} error{}, {} warning{}",
                        e,
                        if e == 1 { "" } else { "s" },
                        w,
                        if w == 1 { "" } else { "s" }
                    ),
                };
                println!("{} — {}", path_str, summary);
                for issue in &fr.issues {
                    let level_label = match issue.level {
                        IssueLevel::Error => "  error  ",
                        IssueLevel::Warning => "  warning",
                    };
                    println!("{}  {}  {}", level_label, issue.path, issue.message);
                }
            }
        }
        if !semantic_issues.is_empty() || load_error.is_some() {
            println!("semantic checks:");
            if let Some(ref err) = load_error {
                println!("  error    (load): {}", err);
            }
            for issue in &semantic_issues {
                let label = match issue.level {
                    IssueLevel::Error => "  error  ",
                    IssueLevel::Warning => "  warning",
                };
                println!("{}  {}", label, issue.message);
            }
        }
        println!();
        if total_errors == 0 && total_warnings == 0 {
            println!("All configuration files are valid.");
        } else {
            println!(
                "{} error{}, {} warning{}",
                total_errors,
                if total_errors == 1 { "" } else { "s" },
                total_warnings,
                if total_warnings == 1 { "" } else { "s" }
            );
        }
        if total_errors > 0 {
            anyhow::bail!("Validation failed");
        }
    }
    Ok(())
}

/// Recursively collect all forge.toml file paths under workspace_root,
/// respecting the default ignore list.
fn collect_forge_tomls(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    const DEFAULT_IGNORE: &[&str] = &[
        "node_modules", "target", "dist", ".git", ".next", ".nuxt",
        ".output", "__pycache__", "vendor", ".turbo", ".nx", ".forge",
    ];
    let mut results = vec![];
    collect_forge_tomls_recursive(root, DEFAULT_IGNORE, &mut results);
    results.sort();
    Ok(results)
}

fn collect_forge_tomls_recursive(
    dir: &std::path::Path,
    ignore: &[&str],
    out: &mut Vec<std::path::PathBuf>,
) {
    let forge_toml = dir.join("forge.toml");
    if forge_toml.is_file() {
        out.push(forge_toml);
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut subdirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    subdirs.sort_by_key(|e| e.file_name());
    for entry in subdirs {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if ignore.iter().any(|&p| p == name_str.as_ref()) {
            continue;
        }
        collect_forge_tomls_recursive(&entry.path(), ignore, out);
    }
}

fn cmd_graph(targets: Vec<String>, json: bool) -> Result<()> {
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

    let levels = {
        let all: Vec<String> = filtered.services.keys().cloned().collect();
        graph::DependencyGraph::build(&filtered)?
            .topological_levels_for(&all)?
    };

    if json {
        let mut nodes: Vec<serde_json::Value> = Vec::new();
        let mut edges: Vec<serde_json::Value> = Vec::new();
        let mut sorted_names: Vec<&String> = filtered.services.keys().collect();
        sorted_names.sort();
        for name in sorted_names {
            let svc = &filtered.services[name];
            nodes.push(serde_json::json!({
                "name": name,
                "port": svc.config.port,
                "groups": svc.config.groups,
            }));
            for dep in &svc.config.depends_on {
                edges.push(serde_json::json!({
                    "from": dep,
                    "to": name,
                }));
            }
        }
        let output = serde_json::json!({
            "nodes": nodes,
            "edges": edges,
            "topology": levels,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let runtime = inspect::detect_all_runtime_info(&filtered);
        let avail_w = terminal_width();
        let output = tui::dag::render_dag_ansi(&filtered, &runtime, &levels, avail_w);
        print!("{}", output);
    }
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
