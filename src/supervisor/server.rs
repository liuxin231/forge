use super::protocol::{
    HealthStatus, ProcessStatus, Request, Response, ServiceStatus,
};
use crate::config::{ProjectConfig, ServiceMode};
use crate::log::collector::{spawn_log_collector, LogBuffer, LogLine};
use crate::process::{health, platform, runner};
use crate::process::restart::RestartTracker;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex, oneshot};

struct ManagedService {
    name: String,
    pid: Option<u32>,
    status: ProcessStatus,
    health: HealthStatus,
    restart_tracker: RestartTracker,
    /// Port detected from the OS after the service starts.
    detected_port: Option<u16>,
    /// True when stopped via explicit `down` command.
    /// Prevents spawn_monitor from restarting the service or triggering shutdown.
    explicitly_stopped: bool,
}

struct SupervisorState {
    services: HashMap<String, ManagedService>,
    project: ProjectConfig,
    workspace_root: PathBuf,
    log_tx: broadcast::Sender<LogLine>,
    log_buffer: LogBuffer,
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Port this supervisor is listening on — used to filter it out of service port detection.
    supervisor_port: u16,
}

pub async fn run_server(
    listener: TcpListener,
    project: ProjectConfig,
    workspace_root: PathBuf,
) -> Result<()> {
    let (log_tx, _) = broadcast::channel::<LogLine>(10000);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let supervisor_port = listener.local_addr()?.port();
    let log_buffer: LogBuffer = Arc::new(std::sync::Mutex::new(HashMap::new()));

    let state = Arc::new(Mutex::new(SupervisorState {
        services: HashMap::new(),
        project,
        workspace_root,
        log_tx: log_tx.clone(),
        log_buffer,
        shutdown_tx: Some(shutdown_tx),
        supervisor_port,
    }));

    tracing::info!(
        "Supervisor listening on {}",
        listener.local_addr()?
    );

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, state).await {
                                tracing::error!("Connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Failed to accept connection: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
            _ = &mut shutdown_rx => {
                tracing::info!("Supervisor shutting down: all services stopped");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    state: Arc<Mutex<SupervisorState>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;

        if n == 0 {
            // Client disconnected
            break;
        }

        let request: Request = match serde_json::from_str(line.trim()) {
            Ok(req) => req,
            Err(e) => {
                let error_response = Response::Error(format!("Invalid request: {}", e));
                let resp_json = serde_json::to_string(&error_response)? + "\n";
                writer.write_all(resp_json.as_bytes()).await?;
                continue;
            }
        };

        // Streaming logs hijack the connection — handle separately and return.
        if let Request::Logs { services, tail, follow: true } = request {
            // Subscribe before reading history to avoid missing messages at the boundary.
            let mut log_rx = state.lock().await.log_tx.subscribe();
            let svc_set: std::collections::HashSet<String> = services.iter().cloned().collect();

            // Signal client to enter stream mode.
            let stream_signal = serde_json::to_string(&Response::LogStream)? + "\n";
            writer.write_all(stream_signal.as_bytes()).await?;

            // Stream historical tail lines first.
            let initial = handle_logs_tail(&services, tail, &state).await;
            if let Response::LogLines(lines) = initial {
                for line in lines {
                    let json = serde_json::to_string(&line)? + "\n";
                    if writer.write_all(json.as_bytes()).await.is_err() {
                        return Ok(());
                    }
                }
            }

            // Separator: marks end of history, start of live stream.
            let sep = serde_json::to_string(&crate::log::collector::LogLine {
                service: String::new(),
                timestamp: String::new(),
                stream: "_follow_start_".to_string(),
                message: String::new(),
            })? + "\n";
            if writer.write_all(sep.as_bytes()).await.is_err() {
                return Ok(());
            }

            loop {
                match log_rx.recv().await {
                    Ok(log_line) => {
                        if svc_set.is_empty() || svc_set.contains(&log_line.service) {
                            let json = serde_json::to_string(&log_line)? + "\n";
                            if writer.write_all(json.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Log stream lagged, skipped {} messages", n);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            return Ok(());
        }

        let response = match request {
            Request::Up(services) => handle_up(services, &state).await,
            Request::Down(services) => handle_down(services, &state).await,
            Request::Restart(services) => handle_restart(services, &state).await,
            Request::Status(filter) => handle_status(filter, &state).await,
            Request::Logs { services, tail, follow: false } => {
                handle_logs_tail(&services, tail, &state).await
            }
            Request::Logs { .. } => unreachable!(), // handled above
        };

        let resp_json = serde_json::to_string(&response)? + "\n";
        writer.write_all(resp_json.as_bytes()).await?;
    }

    Ok(())
}

async fn handle_up(services: Vec<String>, state: &Arc<Mutex<SupervisorState>>) -> Response {
    let state_guard = state.lock().await;
    let project = state_guard.project.clone();
    let workspace_root = state_guard.workspace_root.clone();
    let log_tx = state_guard.log_tx.clone();
    let log_buffer = state_guard.log_buffer.clone();
    let supervisor_port = state_guard.supervisor_port;
    drop(state_guard);

    let mut statuses = Vec::new();

    for name in &services {
        // Skip if already running
        {
            let guard = state.lock().await;
            if let Some(svc) = guard.services.get(name)
                && svc.status == ProcessStatus::Running {
                    statuses.push(ServiceStatus {
                        name: name.clone(),
                        port: svc.detected_port,
                        status: ProcessStatus::Running,
                        health: svc.health.clone(),
                        pid: svc.pid,
                        restarts: svc.restart_tracker.restart_count,
                    });
                    continue;
                }
        }

        let svc_config = match project.services.get(name) {
            Some(s) => s,
            None => {
                return Response::Error(format!("Service '{}' not found", name));
            }
        };

        // Start the service
        match runner::start_service(svc_config, &workspace_root).await {
            Ok(mut child) => {
                let pid = child.id();

                // Set up log collection
                if let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) {
                    spawn_log_collector(
                        name.clone(),
                        stdout,
                        stderr,
                        log_tx.clone(),
                        Some(log_buffer.clone()),
                    );
                }

                // Wait for health check — pass pid so port can be auto-detected
                // Pass 0 to use the health.timeout value from config
                let health_result = health::wait_healthy(
                    name,
                    pid,
                    svc_config.config.port,
                    &svc_config.config.health,
                    0,
                    &svc_config.dir,
                )
                .await;

                let health_status = match health_result {
                    Ok(()) => HealthStatus::Healthy,
                    Err(_) => HealthStatus::Unhealthy,
                };

                // Detect the port the service is actually listening on.
                // Filter out the supervisor's own port in case the child inherited the socket.
                // Retry with backoff: some services (e.g. rsbuild) finish health checks before
                // they have bound their listen socket, so a single probe would miss the port.
                // We retry for up to PORT_DETECT_TIMEOUT_MS, then fall back.
                const PORT_DETECT_INTERVAL_MS: u64 = 300;
                const PORT_DETECT_TIMEOUT_MS: u64 = 5_000;
                let detected_port = 'detect: {
                    if let Some(p) = pid {
                        let attempts = PORT_DETECT_TIMEOUT_MS / PORT_DETECT_INTERVAL_MS;
                        for attempt in 0..=attempts {
                            let ports = platform::detect_listening_ports(p);
                            if let Some(port) =
                                ports.into_iter().find(|&pp| pp != supervisor_port)
                            {
                                break 'detect Some(port);
                            }
                            if attempt < attempts {
                                tokio::time::sleep(
                                    std::time::Duration::from_millis(PORT_DETECT_INTERVAL_MS),
                                )
                                .await;
                            }
                        }
                    }
                    platform::detect_docker_compose_port(
                        &svc_config.dir,
                        &svc_config.config.up,
                    )
                    .or(svc_config.config.port)
                };

                let mut tracker = RestartTracker::new(
                    svc_config.config.autorestart,
                    svc_config.config.max_restarts,
                    svc_config.config.restart_delay,
                );
                tracker.record_start();

                statuses.push(ServiceStatus {
                    name: name.clone(),
                    port: detected_port,
                    status: ProcessStatus::Running,
                    health: health_status.clone(),
                    pid,
                    restarts: 0,
                });

                // Spawn process monitor for auto-restart
                spawn_monitor(
                    child,
                    name.clone(),
                    svc_config.clone(),
                    workspace_root.clone(),
                    log_tx.clone(),
                    log_buffer.clone(),
                    state.clone(),
                );

                let mut guard = state.lock().await;
                guard.services.insert(
                    name.clone(),
                    ManagedService {
                        name: name.clone(),
                        pid,
                        status: ProcessStatus::Running,
                        health: health_status,
                        restart_tracker: tracker,
                        detected_port,
                        explicitly_stopped: false,
                    },
                );
            }
            Err(e) => {
                return Response::Error(format!("Failed to start '{}': {}", name, e));
            }
        }
    }

    Response::Services(statuses)
}

async fn handle_down(services: Vec<String>, state: &Arc<Mutex<SupervisorState>>) -> Response {
    // Phase 1: collect stop tasks; mark services as explicitly stopped (under lock, no await).
    struct StopTask {
        pid: Option<u32>,
        kill_timeout: u64,
        treekill: bool,
        name: String,
        /// `down` command to run for oneshot services (no PID to kill).
        down_cmd: Option<String>,
        svc_dir: std::path::PathBuf,
    }

    let stop_tasks: Vec<StopTask> = {
        let mut guard = state.lock().await;

        let to_stop: Vec<String> = if services.is_empty() {
            guard.services.keys().cloned().collect()
        } else {
            services
        };

        let workspace_root = guard.workspace_root.clone();
        let mut tasks = Vec::new();

        for name in &to_stop {
            let (kill_timeout, treekill, mode, down_cmd, svc_dir) = {
                let svc = guard.project.services.get(name);
                (
                    svc.map(|s| s.config.kill_timeout).unwrap_or(10),
                    svc.map(|s| s.config.treekill).unwrap_or(true),
                    svc.map(|s| s.config.mode.clone()).unwrap_or_default(),
                    svc.and_then(|s| s.config.down.clone()),
                    svc.map(|s| s.dir.clone()).unwrap_or_default(),
                )
            };

            if let Some(managed) = guard.services.get_mut(name) {
                managed.explicitly_stopped = true;
                managed.status = ProcessStatus::Stopped;
                managed.health = HealthStatus::None;

                let pid = managed.pid.take();
                if pid.is_some() {
                    runner::remove_pid_file(&workspace_root, name);
                }

                tasks.push(StopTask {
                    pid,
                    kill_timeout,
                    treekill,
                    name: name.clone(),
                    // Only run down_cmd when no PID to kill (oneshot or already-exited).
                    down_cmd: if mode == ServiceMode::Oneshot { down_cmd } else { None },
                    svc_dir,
                });
            }
        }

        tasks
    }; // lock released here

    // Phase 2: kill processes and run down commands sequentially without holding the lock.
    for task in &stop_tasks {
        if let Some(pid) = task.pid {
            let _ = platform::stop_process(pid, task.kill_timeout, task.treekill).await;
        }
        if let Some(cmd) = &task.down_cmd {
            tracing::info!("Running down command for '{}': {}", task.name, cmd);
            let _ = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&task.svc_dir)
                .status()
                .await;
        }
    }

    // Auto-shutdown supervisor if all services are now stopped.
    {
        let mut guard = state.lock().await;
        trigger_shutdown_if_all_stopped(&mut guard);
    }

    Response::Ok
}

async fn handle_restart(services: Vec<String>, state: &Arc<Mutex<SupervisorState>>) -> Response {
    // Stop then start
    let down_resp = handle_down(services.clone(), state).await;
    if matches!(down_resp, Response::Error(_)) {
        return down_resp;
    }
    handle_up(services, state).await
}

async fn handle_status(filter: Vec<String>, state: &Arc<Mutex<SupervisorState>>) -> Response {
    let guard = state.lock().await;

    // Collect running services that need a live health check
    let mut to_check: Vec<(String, Option<u16>, Option<crate::config::service::HealthConfig>, PathBuf)> = Vec::new();

    let mut statuses: Vec<ServiceStatus> = guard
        .services
        .values()
        .filter(|s| filter.is_empty() || filter.contains(&s.name))
        .map(|s| {
            if s.status == ProcessStatus::Running {
                if let Some(svc_cfg) = guard.project.services.get(&s.name) {
                    to_check.push((
                        s.name.clone(),
                        s.detected_port.or(svc_cfg.config.port),
                        svc_cfg.config.health.clone(),
                        svc_cfg.dir.clone(),
                    ));
                }
            }
            ServiceStatus {
                name: s.name.clone(),
                port: s.detected_port,
                status: s.status.clone(),
                health: s.health.clone(),
                pid: s.pid,
                restarts: s.restart_tracker.restart_count,
            }
        })
        .collect();

    // Also include services that are known but not started
    for name in guard.project.services.keys() {
        if !guard.services.contains_key(name)
            && (filter.is_empty() || filter.contains(name))
        {
            statuses.push(ServiceStatus {
                name: name.clone(),
                port: None,
                status: ProcessStatus::Stopped,
                health: HealthStatus::None,
                pid: None,
                restarts: 0,
            });
        }
    }

    // Release lock before doing async health checks
    drop(guard);

    // Run live health checks for running services
    if !to_check.is_empty() {
        let mut result_map: HashMap<String, HealthStatus> = HashMap::new();
        for (name, port, health_cfg, cwd) in to_check {
            let healthy = health::check_health_once(port, &health_cfg, &cwd).await;
            let status = if healthy { HealthStatus::Healthy } else { HealthStatus::Unhealthy };
            result_map.insert(name, status);
        }
        for status in &mut statuses {
            if let Some(health) = result_map.get(&status.name) {
                status.health = health.clone();
            }
        }

        // Also update cached state so monitor/restart logic sees fresh health
        let mut guard = state.lock().await;
        for (name, health) in &result_map {
            if let Some(managed) = guard.services.get_mut(name) {
                managed.health = health.clone();
            }
        }
    }

    statuses.sort_by(|a, b| a.name.cmp(&b.name));

    Response::Services(statuses)
}

async fn handle_logs_tail(
    services: &[String],
    tail: usize,
    state: &Arc<Mutex<SupervisorState>>,
) -> Response {
    let guard = state.lock().await;
    let buffer = guard.log_buffer.clone();

    let svc_names: Vec<String> = if services.is_empty() {
        guard.services.keys().cloned().collect()
    } else {
        services.to_vec()
    };
    drop(guard);

    let mut all_lines = Vec::new();
    for name in &svc_names {
        let lines = crate::log::collector::read_from_buffer(&buffer, name, tail);
        all_lines.extend(lines);
    }

    Response::LogLines(all_lines)
}

/// Trigger supervisor shutdown if every service has stopped.
/// Does NOT delete pid/port files here — run_as_daemon cleans those up on actual exit.
fn trigger_shutdown_if_all_stopped(guard: &mut SupervisorState) {
    if guard.services.is_empty() {
        return;
    }
    let all_stopped = guard
        .services
        .values()
        .all(|s| s.status == ProcessStatus::Stopped || s.status == ProcessStatus::Errored);
    if all_stopped {
        if let Some(tx) = guard.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Spawn a task that waits for `child` to exit, then handles restart or marks stopped.
/// On restart, spawns a new monitor for the replacement process (recursive, no zombie).
fn spawn_monitor(
    mut child: tokio::process::Child,
    svc_name: String,
    svc_cfg: crate::config::ResolvedService,
    ws_root: PathBuf,
    log_tx: broadcast::Sender<LogLine>,
    log_buffer: LogBuffer,
    state: Arc<Mutex<SupervisorState>>,
) {
    tokio::spawn(async move {
        let exit_status = child.wait().await;
        tracing::info!("Service '{}' exited: {:?}", svc_name, exit_status);

        let mut guard = state.lock().await;
        let Some(managed) = guard.services.get_mut(&svc_name) else { return };

        // If explicitly stopped via `down`, just clean up — no restart, no shutdown trigger.
        if managed.explicitly_stopped {
            managed.pid = None;
            managed.detected_port = None;
            return;
        }

        // Oneshot: the launcher process is expected to exit 0 immediately.
        // The real service runs externally (e.g. in Docker). Keep status = Running
        // and rely on the health check for liveness. Never restart on success.
        if svc_cfg.config.mode == ServiceMode::Oneshot {
            let code = exit_status.ok().and_then(|s| s.code());
            managed.pid = None;
            managed.detected_port = None;
            if code == Some(0) {
                // Launcher exited cleanly — daemon is running externally.
                managed.status = ProcessStatus::Running;
            } else {
                tracing::warn!(
                    "Oneshot service '{}' exited with code {:?}, marking as errored",
                    svc_name,
                    code
                );
                managed.status = ProcessStatus::Errored;
                trigger_shutdown_if_all_stopped(&mut guard);
            }
            return;
        }

        if managed.restart_tracker.should_restart() {
            let delay = managed.restart_tracker.delay();
            drop(guard);

            tokio::time::sleep(delay).await;

            match runner::start_service(&svc_cfg, &ws_root).await {
                Ok(mut new_child) => {
                    let new_pid = new_child.id();
                    if let (Some(stdout), Some(stderr)) =
                        (new_child.stdout.take(), new_child.stderr.take())
                    {
                        spawn_log_collector(
                            svc_name.clone(),
                            stdout,
                            stderr,
                            log_tx.clone(),
                            Some(log_buffer.clone()),
                        );
                    }

                    let restarted_port = new_pid
                        .and_then(|p| platform::detect_listening_ports(p).into_iter().next())
                        .or(svc_cfg.config.port);

                    let mut guard = state.lock().await;
                    if let Some(managed) = guard.services.get_mut(&svc_name) {
                        managed.pid = new_pid;
                        managed.status = ProcessStatus::Running;
                        managed.restart_tracker.record_start();
                        managed.detected_port = restarted_port;
                        managed.explicitly_stopped = false;
                    }
                    drop(guard);

                    // Monitor the new child — prevents zombie and handles further restarts
                    spawn_monitor(new_child, svc_name, svc_cfg, ws_root, log_tx, log_buffer, state);
                }
                Err(e) => {
                    tracing::error!("Failed to restart '{}': {}", svc_name, e);
                    let mut guard = state.lock().await;
                    if let Some(managed) = guard.services.get_mut(&svc_name) {
                        managed.status = ProcessStatus::Errored;
                        managed.pid = None;
                        managed.detected_port = None;
                    }
                    trigger_shutdown_if_all_stopped(&mut guard);
                }
            }
        } else {
            managed.status = if managed.restart_tracker.is_errored() {
                ProcessStatus::Errored
            } else {
                ProcessStatus::Stopped
            };
            managed.pid = None;
            managed.detected_port = None;
            trigger_shutdown_if_all_stopped(&mut guard);
        }
    });
}
