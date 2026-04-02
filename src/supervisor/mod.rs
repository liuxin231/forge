pub mod client;
pub mod daemon;
pub mod protocol;
pub mod server;

use crate::config::ProjectConfig;
use crate::output::topo::{ServiceState, TopoRenderer};
use anyhow::{Context, Result};
use std::path::Path;

pub use client::SupervisorClient;

/// Ensure a supervisor is running, starting one if needed.
pub async fn ensure_supervisor(
    workspace_root: &Path,
    project: &ProjectConfig,
) -> Result<SupervisorClient> {
    if let Some(port) = daemon::get_running_supervisor(workspace_root) {
        match SupervisorClient::connect(port).await {
            Ok(client) => return Ok(client),
            Err(_) => {
                daemon::cleanup_supervisor_files(workspace_root);
            }
        }
    }

    let port = daemon::start_supervisor(workspace_root, project).await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    SupervisorClient::connect(port)
        .await
        .context("Failed to connect to newly started supervisor")
}

/// Connect to an existing supervisor
pub async fn connect_supervisor(workspace_root: &Path) -> Result<SupervisorClient> {
    let port = daemon::get_running_supervisor(workspace_root)
        .ok_or_else(|| anyhow::anyhow!("No supervisor is running. Start services with 'fr up' first."))?;

    SupervisorClient::connect(port)
        .await
        .context("Failed to connect to supervisor")
}

/// Start services by topological levels with a live topology graph.
pub async fn run_mixed_mode(
    project: &ProjectConfig,
    levels: &[Vec<String>],
    attach_set: &std::collections::HashSet<String>,
    parallel: bool,
    json: bool,
) -> Result<()> {
    use crate::log::collector::LogLine;
    use colored::Colorize;
    use tokio::sync::broadcast;

    let (log_tx, mut log_rx) = broadcast::channel::<LogLine>(1000);

    let all_services: Vec<String> = levels.iter().flatten().cloned().collect();
    let has_bg = all_services.iter().any(|n| !attach_set.contains(n));

    // Render initial topology graph (non-JSON mode)
    let mut topo = if !json {
        Some(TopoRenderer::new(levels, project))
    } else {
        None
    };

    let mut supervisor_client = if has_bg {
        Some(ensure_supervisor(&project.root, project).await?)
    } else {
        None
    };

    let mut children: Vec<(String, tokio::process::Child)> = Vec::new();
    let mut all_statuses: Vec<protocol::ServiceStatus> = Vec::new();

    for level in levels {
        if parallel && level.len() > 1 {
            // Mark all as starting
            for name in level {
                if let Some(ref mut t) = topo {
                    t.update_state(name, ServiceState::Starting);
                }
            }

            // Parallel: attached services concurrently, bg services sequentially via supervisor
            let mut handles = Vec::new();
            let mut bg_names = Vec::new();

            for name in level {
                if attach_set.contains(name) {
                    let svc = match project.services.get(name) {
                        Some(s) => s.clone(),
                        None => {
                            tracing::error!("Service '{}' not found in project config", name);
                            continue;
                        }
                    };
                    let root = project.root.clone();
                    let tx = log_tx.clone();
                    let n = name.clone();
                    handles.push(tokio::spawn(async move {
                        start_attached_service(&n, &svc, &root, tx).await
                    }));
                } else {
                    bg_names.push(name.clone());
                }
            }

            // Start bg services via supervisor
            for name in &bg_names {
                let client = supervisor_client.as_mut().unwrap();
                let response = client
                    .send(protocol::Request::Up(vec![name.clone()]))
                    .await?;
                if let protocol::Response::Services(ref statuses) = response {
                    for s in statuses {
                        let state = match s.health {
                            protocol::HealthStatus::Healthy => ServiceState::Healthy,
                            _ => ServiceState::Unhealthy,
                        };
                        if let Some(ref mut t) = topo {
                            t.set_port(&s.name, s.port);
                            t.update_state(&s.name, state);
                        }
                        all_statuses.push(s.clone());
                    }
                }
            }

            // Collect attached results
            for handle in handles {
                match handle.await? {
                    Ok(StartResult::Attached(name, child, status)) => {
                        let state = match status.health {
                            protocol::HealthStatus::Healthy => ServiceState::Healthy,
                            _ => ServiceState::Unhealthy,
                        };
                        if let Some(ref mut t) = topo {
                            t.set_port(&name, status.port);
                            t.update_state(&name, state);
                        }
                        all_statuses.push(status);
                        children.push((name, *child));
                    }
                    Err(e) => {
                        eprintln!("  {} {}", "✗".red(), e);
                    }
                    _ => {}
                }
            }
        } else {
            // Sequential
            for name in level {
                if let Some(ref mut t) = topo {
                    t.update_state(name, ServiceState::Starting);
                }

                if attach_set.contains(name) {
                    let svc = match project.services.get(name) {
                        Some(s) => s,
                        None => {
                            tracing::error!("Service '{}' not found in project config", name);
                            continue;
                        }
                    };
                    let (child, status) = start_attached_service_inline(
                        name, svc, &project.root, &log_tx,
                    )
                    .await?;

                    let state = match status.health {
                        protocol::HealthStatus::Healthy => ServiceState::Healthy,
                        _ => ServiceState::Unhealthy,
                    };
                    if let Some(ref mut t) = topo {
                        t.set_port(name, status.port);
                        t.update_state(name, state);
                    }
                    all_statuses.push(status);
                    children.push((name.clone(), child));
                } else {
                    let client = supervisor_client.as_mut().unwrap();
                    let response = client
                        .send(protocol::Request::Up(vec![name.clone()]))
                        .await?;
                    match response {
                        protocol::Response::Services(ref statuses) => {
                            for s in statuses {
                                let state = match s.health {
                                    protocol::HealthStatus::Healthy => ServiceState::Healthy,
                                    _ => ServiceState::Unhealthy,
                                };
                                if let Some(ref mut t) = topo {
                                    t.set_port(&s.name, s.port);
                                    t.update_state(&s.name, state);
                                }
                                all_statuses.push(s.clone());
                            }
                        }
                        protocol::Response::Error(e) => {
                            if let Some(ref mut t) = topo {
                                t.update_state(name, ServiceState::Failed(e.clone()));
                            }
                            anyhow::bail!("Failed to start '{}': {}", name, e);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Print summary
    if let Some(ref t) = topo {
        t.print_summary();
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_statuses)?);
    }

    if children.is_empty() {
        return Ok(());
    }

    if !json {
        eprintln!();
        if has_bg {
            eprintln!("  Streaming attached service logs... (Ctrl+C to stop attached only)");
        } else {
            eprintln!("  Streaming logs... (Ctrl+C to stop all)");
        }
        eprintln!();
    }

    // Stream logs from attached services and wait for Ctrl+C
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(std::sync::Mutex::new(Some(shutdown_tx)));

    let shutdown_clone = shutdown_tx.clone();
    ctrlc_handler(move || {
        if let Ok(mut guard) = shutdown_clone.lock()
            && let Some(tx) = guard.take() {
                let _ = tx.send(());
            }
    });

    loop {
        tokio::select! {
            result = log_rx.recv() => {
                match result {
                    Ok(line) => {
                        if attach_set.contains(&line.service) {
                            if json {
                                println!("{}", serde_json::to_string(&line).unwrap_or_default());
                            } else {
                                print_log_line_colored(&line);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Log stream lagged, skipped {} messages", n);
                    }
                    Err(_) => break,
                }
            }
            _ = &mut shutdown_rx => {
                break;
            }
        }
    }

    eprintln!("\nStopping attached services...");

    for (name, mut child) in children.into_iter().rev() {
        if let Some(pid) = child.id() {
            let svc = match project.services.get(&name) {
                Some(s) => s,
                None => {
                    let _ = child.kill().await;
                    continue;
                }
            };
            let _ = crate::process::platform::stop_process(
                pid,
                svc.config.kill_timeout,
                svc.config.treekill,
            )
            .await;
            crate::process::runner::remove_pid_file(&project.root, &name);
        }
        let _ = child.wait().await;
        eprintln!("  {} {} stopped", "✓".green(), name);
    }

    if has_bg && !json {
        eprintln!("Background services are still running. Use 'fr down' to stop them.");
    }

    Ok(())
}

enum StartResult {
    Attached(String, Box<tokio::process::Child>, protocol::ServiceStatus),
    #[allow(dead_code)]
    NeedsSupervisor(String),
}

async fn start_attached_service(
    name: &str,
    svc: &crate::config::ResolvedService,
    workspace_root: &std::path::Path,
    log_tx: tokio::sync::broadcast::Sender<crate::log::collector::LogLine>,
) -> Result<StartResult> {
    use crate::log::collector::spawn_log_collector;
    use crate::process::{health, platform, runner};

    let mut child = runner::start_service(svc, workspace_root).await?;
    let pid = child.id();

    if let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) {
        spawn_log_collector(name.to_string(), stdout, stderr, log_tx, None);
    }

    let health_result =
        health::wait_healthy(name, pid, svc.config.port, &svc.config.health, 0, &svc.dir).await;

    let (status, health_status, health_port) = match health_result {
        Ok(p) => (protocol::ProcessStatus::Running, protocol::HealthStatus::Healthy, p),
        Err(_) => (protocol::ProcessStatus::Running, protocol::HealthStatus::Unhealthy, None),
    };

    let detected_port = health_port
        .or_else(|| pid.and_then(|p| platform::detect_listening_ports(p).into_iter().next()))
        .or(svc.config.port);

    Ok(StartResult::Attached(
        name.to_string(),
        Box::new(child),
        protocol::ServiceStatus {
            name: name.to_string(),
            port: detected_port,
            status,
            health: health_status,
            pid,
            restarts: 0,
        },
    ))
}

async fn start_attached_service_inline(
    name: &str,
    svc: &crate::config::ResolvedService,
    workspace_root: &std::path::Path,
    log_tx: &tokio::sync::broadcast::Sender<crate::log::collector::LogLine>,
) -> Result<(tokio::process::Child, protocol::ServiceStatus)> {
    use crate::log::collector::spawn_log_collector;
    use crate::process::{health, platform, runner};

    let mut child = runner::start_service(svc, workspace_root).await?;
    let pid = child.id();

    if let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) {
        spawn_log_collector(name.to_string(), stdout, stderr, log_tx.clone(), None);
    }

    let health_result =
        health::wait_healthy(name, pid, svc.config.port, &svc.config.health, 0, &svc.dir).await;

    let (status, health_status, health_port) = match health_result {
        Ok(p) => (protocol::ProcessStatus::Running, protocol::HealthStatus::Healthy, p),
        Err(_) => (protocol::ProcessStatus::Running, protocol::HealthStatus::Unhealthy, None),
    };

    let detected_port = health_port
        .or_else(|| pid.and_then(|p| platform::detect_listening_ports(p).into_iter().next()))
        .or(svc.config.port);

    Ok((
        child,
        protocol::ServiceStatus {
            name: name.to_string(),
            port: detected_port,
            status,
            health: health_status,
            pid,
            restarts: 0,
        },
    ))
}

fn print_log_line_colored(line: &crate::log::collector::LogLine) {
    use colored::Colorize;

    let colors = ["blue", "green", "yellow", "cyan", "magenta", "red"];
    let hash = line
        .service
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_add(b as usize));
    let color = colors[hash % colors.len()];

    let prefix = format!("[{}]", line.service);
    let colored_prefix = match color {
        "blue" => prefix.blue(),
        "green" => prefix.green(),
        "yellow" => prefix.yellow(),
        "cyan" => prefix.cyan(),
        "magenta" => prefix.magenta(),
        "red" => prefix.red(),
        _ => prefix.normal(),
    };

    if line.stream == "stderr" {
        eprintln!("{} {}", colored_prefix, line.message);
    } else {
        println!("{} {}", colored_prefix, line.message);
    }
}

fn ctrlc_handler<F: FnOnce() + Send + 'static>(f: F) {
    let f = std::sync::Mutex::new(Some(f));
    let _ = ctrlc::set_handler(move || {
        if let Ok(mut guard) = f.lock()
            && let Some(f) = guard.take() {
                f();
            }
    });
}
