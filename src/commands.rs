use crate::cache::{self, CacheCheckResult};
use crate::config::{ProjectConfig, ResolvedService};
use crate::graph::DependencyGraph;
use crate::resolver;
use anyhow::{bail, Result};
use colored::Colorize;
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

/// Options for `fr run`
pub struct RunOptions {
    pub parallel: bool,
    pub dry_run: bool,
    pub concurrency: Option<usize>,
    pub since: Option<String>,
    pub verbose: u8,
    pub json: bool,
}

/// Execute a custom command
pub async fn execute_command(
    project: &ProjectConfig,
    name: &str,
    targets: &[String],
    opts: RunOptions,
) -> Result<()> {
    // Workspace-level direct command
    if let Some(cmd_config) = project.workspace.commands.get(name)
        && cmd_config.mode == "direct"
    {
        return execute_direct_command(project, name, cmd_config, &opts).await;
    }

    execute_service_command(project, name, targets, opts).await
}

/// Execute a direct (workspace-level) command
async fn execute_direct_command(
    project: &ProjectConfig,
    name: &str,
    config: &crate::config::workspace::CommandConfig,
    opts: &RunOptions,
) -> Result<()> {
    let run_cmd = config
        .run
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Command '{}' (mode=direct) has no 'run' field", name))?;

    if opts.dry_run {
        eprintln!("  {} {} (workspace)", "would run:".dimmed(), run_cmd.bold());
        return Ok(());
    }

    if !opts.json {
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

    if opts.json {
        println!(
            "{}",
            serde_json::to_string(&DirectCommandResult {
                command: name.to_string(),
                status: "ok".to_string(),
            })?
        );
    }

    Ok(())
}

/// Execute a command delegated to matching services
async fn execute_service_command(
    project: &ProjectConfig,
    name: &str,
    targets: &[String],
    opts: RunOptions,
) -> Result<()> {
    let mut resolved = resolver::resolve_targets(project, targets)?;

    // --since: filter to services with changed files
    if let Some(ref git_ref) = opts.since {
        let changed = get_changed_services(project, git_ref)?;
        if opts.verbose >= 2 {
            eprintln!(
                "[debug] --since {}: changed services = {:?}",
                git_ref, changed
            );
        }
        resolved.retain(|s| changed.contains(s));
        if resolved.is_empty() {
            if !opts.json {
                eprintln!(
                    "{} No services changed since '{}' — nothing to run.",
                    "→".dimmed(),
                    git_ref
                );
            }
            return Ok(());
        }
    }

    // Determine execution order
    let cmd_config = project.workspace.commands.get(name);
    let order = if opts.parallel {
        "parallel"
    } else {
        cmd_config
            .map(|c| c.order.as_str())
            .unwrap_or("topological")
    };

    if !["topological", "parallel", "sequential"].contains(&order) {
        bail!(
            "Invalid command order '{}' for '{}'. Must be: topological, parallel, or sequential",
            order,
            name
        );
    }

    let fail_fast = cmd_config.map(|c| c.fail_fast).unwrap_or(true);

    // Filter to services that have this command
    let services_with_cmd: Vec<&str> = resolved
        .iter()
        .filter(|svc_name| service_has_command(project, svc_name, name))
        .map(|s| s.as_str())
        .collect();

    if services_with_cmd.is_empty() {
        bail!(
            "No services have command '{}' defined. Add [service.commands.{}] to the service's forge.toml.",
            name, name
        );
    }

    // Order services
    let ordered: Vec<String> = match order {
        "topological" => {
            let graph = DependencyGraph::build(project)?;
            let all_names: Vec<String> =
                services_with_cmd.iter().map(|s| s.to_string()).collect();
            let topo = graph.topological_order_for(&all_names)?;
            let cmd_set: HashSet<&str> = services_with_cmd.iter().copied().collect();
            topo.into_iter()
                .filter(|s| cmd_set.contains(s.as_str()))
                .collect()
        }
        _ => services_with_cmd.iter().map(|s| s.to_string()).collect(),
    };

    // --dry-run: just print execution plan
    if opts.dry_run {
        eprintln!("{} {} ({}, {} service(s)):", "dry-run:".bold(), name, order, ordered.len());
        for svc_name in &ordered {
            let cmd_str = project
                .services
                .get(svc_name)
                .and_then(|s| get_command_string(s, name).ok())
                .unwrap_or_else(|| "<unknown>".to_string());
            eprintln!("  {} {} — {}", "→".dimmed(), svc_name.bold(), cmd_str);
        }
        return Ok(());
    }

    let cache_root = cache::cache_root(&project.root);
    let total_start = Instant::now();
    let mut results: Vec<CommandResult> = Vec::new();

    if order == "parallel" {
        let concurrency = opts.concurrency.unwrap_or(usize::MAX);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut handles: Vec<(String, tokio::task::JoinHandle<(bool, bool, u64, String)>)> =
            Vec::new();

        for svc_name in &ordered {
            let svc = match project.services.get(svc_name) {
                Some(s) => s.clone(),
                None => continue,
            };
            let cmd_name = name.to_string();
            let root = project.root.clone();
            let cache_root = cache_root.clone();
            let token = cancel.clone();
            let sem = semaphore.clone();
            let verbose = opts.verbose;

            handles.push((
                svc_name.clone(),
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.unwrap();
                    let start = Instant::now();
                    let res = tokio::select! {
                        r = run_service_command(&svc, &cmd_name, &root, &cache_root, verbose) => r,
                        _ = token.cancelled() => Err(anyhow::anyhow!("cancelled")),
                    };
                    let elapsed = start.elapsed().as_millis() as u64;
                    let (success, cache_hit, msg) = match res {
                        Ok(hit) => (true, hit, "ok".to_string()),
                        Err(e) => (false, false, e.to_string()),
                    };
                    (success, cache_hit, elapsed, msg)
                }),
            ));
        }

        for (svc_name, handle) in handles {
            let (success, cache_hit, elapsed_ms, message) = handle.await?;
            if !opts.json {
                print_service_result(&svc_name, name, success, cache_hit);
            }
            results.push(CommandResult {
                service: svc_name.clone(),
                command: name.to_string(),
                success,
                cache_hit,
                skipped: false,
                duration_ms: elapsed_ms,
                message,
            });
            if !success && fail_fast {
                cancel.cancel();
            }
        }
    } else {
        for svc_name in &ordered {
            let svc = match project.services.get(svc_name) {
                Some(s) => s,
                None => continue,
            };
            let start = Instant::now();
            let result =
                run_service_command(svc, name, &project.root, &cache_root, opts.verbose).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let (success, cache_hit, message) = match result {
                Ok(hit) => (true, hit, "ok".to_string()),
                Err(e) => (false, false, e.to_string()),
            };

            if !opts.json {
                print_service_result(svc_name, name, success, cache_hit);
            }

            results.push(CommandResult {
                service: svc_name.clone(),
                command: name.to_string(),
                success,
                cache_hit,
                skipped: false,
                duration_ms: elapsed_ms,
                message,
            });

            if !success && fail_fast {
                break;
            }
        }
    }

    let total_ms = total_start.elapsed().as_millis() as u64;

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_run_summary(&results, total_ms);
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

/// Run a command for a single service; returns Ok(cache_hit)
async fn run_service_command(
    svc: &ResolvedService,
    cmd_name: &str,
    _workspace_root: &std::path::Path,
    cache_root: &std::path::Path,
    verbose: u8,
) -> Result<bool> {
    let cmd_config = svc.config.commands.get(cmd_name);
    let inputs = cmd_config.map(|c| c.inputs.as_slice()).unwrap_or(&[]);

    // Check cache
    match cache::check_cache(cache_root, &svc.dir, &svc.name, cmd_name, inputs)? {
        CacheCheckResult::Hit => {
            return Ok(true); // cache hit — skip execution
        }
        CacheCheckResult::Miss { hash } => {
            // Run command, then persist cache on success
            let cmd_str = get_command_string(svc, cmd_name)?;
            if verbose >= 1 {
                eprintln!("  {} {}", "▶".dimmed(), cmd_str);
            }
            let cwd = resolve_cwd(svc);
            let status = tokio::process::Command::new("sh")
                .args(["-c", &cmd_str])
                .current_dir(&cwd)
                .envs(&svc.config.env)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .await?;

            if !status.success() {
                bail!(
                    "Command '{}' failed for '{}' (exit {})",
                    cmd_name,
                    svc.name,
                    status.code().unwrap_or(-1)
                );
            }
            // Persist cache
            if let Err(e) = cache::write_cache(cache_root, &svc.name, cmd_name, &hash) {
                tracing::warn!("Failed to write cache for {}/{}: {}", svc.name, cmd_name, e);
            }
            Ok(false)
        }
        CacheCheckResult::Disabled => {
            // No inputs declared — run without caching
            let cmd_str = get_command_string(svc, cmd_name)?;
            if verbose >= 1 {
                eprintln!("  {} {}", "▶".dimmed(), cmd_str);
            }
            let cwd = resolve_cwd(svc);
            let status = tokio::process::Command::new("sh")
                .args(["-c", &cmd_str])
                .current_dir(&cwd)
                .envs(&svc.config.env)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .await?;

            if !status.success() {
                bail!(
                    "Command '{}' failed for '{}' (exit {})",
                    cmd_name,
                    svc.name,
                    status.code().unwrap_or(-1)
                );
            }
            Ok(false)
        }
    }
}

fn resolve_cwd(svc: &ResolvedService) -> std::path::PathBuf {
    if let Some(ref custom_cwd) = svc.config.cwd {
        if std::path::Path::new(custom_cwd).is_absolute() {
            std::path::PathBuf::from(custom_cwd)
        } else {
            svc.dir.join(custom_cwd)
        }
    } else {
        svc.dir.clone()
    }
}

/// Get changed services based on `git diff --name-only <ref>`
fn get_changed_services(project: &ProjectConfig, git_ref: &str) -> Result<HashSet<String>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", git_ref])
        .current_dir(&project.root)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run git: {}", e))?;

    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let changed_files: Vec<std::path::PathBuf> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| project.root.join(l))
        .collect();

    let mut changed = HashSet::new();
    for (name, svc) in &project.services {
        if changed_files.iter().any(|f| f.starts_with(&svc.dir)) {
            changed.insert(name.clone());
        }
    }

    Ok(changed)
}

/// Check if a service has a given command
pub fn service_has_command(project: &ProjectConfig, svc_name: &str, cmd_name: &str) -> bool {
    let svc = match project.services.get(svc_name) {
        Some(s) => s,
        None => return false,
    };
    if svc.config.commands.contains_key(cmd_name) {
        return true;
    }
    matches!(cmd_name, "build") && svc.config.build.is_some()
}

/// Resolve the shell command string for a service + command name
fn get_command_string(svc: &ResolvedService, cmd_name: &str) -> Result<String> {
    if let Some(cmd) = svc.config.commands.get(cmd_name) {
        return Ok(cmd.run.clone());
    }
    if cmd_name == "build"
        && let Some(ref build_cmd) = svc.config.build
    {
        return Ok(build_cmd.clone());
    }
    bail!(
        "No command '{}' defined for service '{}'. Add [service.commands.{}] to its forge.toml.",
        cmd_name,
        svc.name,
        cmd_name
    )
}

fn print_service_result(svc_name: &str, cmd_name: &str, success: bool, cache_hit: bool) {
    if cache_hit {
        eprintln!(
            "  {} {} {}  {}",
            "·".dimmed(),
            cmd_name.bold(),
            svc_name,
            "cache hit".dimmed()
        );
    } else if success {
        eprintln!("  {} {} {}", "+".green(), cmd_name.bold(), svc_name);
    } else {
        eprintln!("  {} {} {}", "x".red(), cmd_name.bold(), svc_name);
    }
}

fn print_run_summary(results: &[CommandResult], total_ms: u64) {
    if results.is_empty() {
        return;
    }
    let ok = results.iter().filter(|r| r.success && !r.cache_hit).count();
    let hits = results.iter().filter(|r| r.cache_hit).count();
    let failed = results.iter().filter(|r| !r.success).count();
    let skipped = results.iter().filter(|r| r.skipped).count();

    eprintln!();
    eprint!("  {}", "Summary:".bold());
    eprint!("  {} services", results.len());
    if ok > 0 {
        eprint!("  {} {}", ok.to_string().green(), "ok".green());
    }
    if hits > 0 {
        eprint!("  {} {}", hits.to_string().cyan(), "cached".cyan());
    }
    if failed > 0 {
        eprint!("  {} {}", failed.to_string().red(), "failed".red());
    }
    if skipped > 0 {
        eprint!("  {} skipped", skipped);
    }
    eprintln!("  ({})", format_duration(total_ms).dimmed());
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
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
    cache_hit: bool,
    skipped: bool,
    duration_ms: u64,
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
                    inputs: vec![],
                    outputs: vec![],
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
                    env: HashMap::new(),
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

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0), "0ms");
        assert_eq!(format_duration(500), "500ms");
        assert_eq!(format_duration(1000), "1.0s");
        assert_eq!(format_duration(2500), "2.5s");
    }
}
