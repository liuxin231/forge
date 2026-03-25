use anyhow::{Context, Result};
use std::path::Path;

/// Get the port of a running supervisor, if any
pub fn get_running_supervisor(workspace_root: &Path) -> Option<u16> {
    let port_file = workspace_root.join(".forge/supervisor.port");
    let pid_file = workspace_root.join(".forge/supervisor.pid");

    let pid_str = std::fs::read_to_string(&pid_file).ok()?;
    let pid: u32 = pid_str.trim().parse().ok()?;

    // Check if process is alive
    if !crate::process::platform::is_process_alive(pid) {
        cleanup_supervisor_files(workspace_root);
        return None;
    }

    let port_str = std::fs::read_to_string(&port_file).ok()?;
    let port: u16 = port_str.trim().parse().ok()?;
    Some(port)
}

/// Start a new supervisor as a detached subprocess.
/// Returns the port the supervisor is listening on.
pub async fn start_supervisor(workspace_root: &Path, _project: &crate::config::ProjectConfig) -> Result<u16> {
    let forge_dir = workspace_root.join(".forge");
    std::fs::create_dir_all(&forge_dir)?;

    // Kill any still-alive supervisor before starting a new one.
    // This prevents two supervisors competing for the same service ports.
    kill_existing_supervisor(workspace_root);

    let exe = std::env::current_exe().context("Failed to find current executable")?;

    // Spawn a fully detached child: new process group, stdio → /dev/null
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let log_file = std::fs::File::create(forge_dir.join("supervisor.log"))
            .unwrap_or_else(|_| std::fs::File::open("/dev/null").unwrap());
        std::process::Command::new(&exe)
            .arg("supervisor")
            .arg("--workspace-root")
            .arg(workspace_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log_file.try_clone().unwrap_or_else(|_| std::fs::File::open("/dev/null").unwrap())))
            .stderr(std::process::Stdio::from(log_file))
            .process_group(0)
            .spawn()
            .context("Failed to spawn supervisor process")?;
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new(&exe)
            .arg("supervisor")
            .arg("--workspace-root")
            .arg(workspace_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to spawn supervisor process")?;
    }

    // Poll until the supervisor writes its port file (up to 5 seconds)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Some(port) = get_running_supervisor(workspace_root) {
            return Ok(port);
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "Supervisor did not start within 5 seconds"
        );
    }
}

/// Kill an existing supervisor process (if alive) before starting a fresh one.
/// Reads supervisor.pid even if supervisor.port is missing (e.g. after trigger_shutdown cleanup).
fn kill_existing_supervisor(workspace_root: &Path) {
    let pid_file = workspace_root.join(".forge/supervisor.pid");
    let pid_str = match std::fs::read_to_string(&pid_file) {
        Ok(s) => s,
        Err(_) => return, // no pid file — nothing to kill
    };
    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => return,
    };

    if !crate::process::platform::is_process_alive(pid) {
        // Already dead — just clean up stale files
        cleanup_supervisor_files(workspace_root);
        return;
    }

    tracing::warn!("Killing existing supervisor PID {} before starting new one", pid);
    #[cfg(unix)]
    {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;
        let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
        // Wait up to 2 seconds for graceful exit
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !crate::process::platform::is_process_alive(pid) {
                break;
            }
        }
        if crate::process::platform::is_process_alive(pid) {
            let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    cleanup_supervisor_files(workspace_root);
}

/// Kill any service processes left over from a previous supervisor that crashed.
/// Reads all `.forge/pids/*.pid` files and kills still-alive processes,
/// then removes the stale PID files.
fn kill_stale_services(workspace_root: &Path) {
    let pid_dir = workspace_root.join(".forge/pids");
    let entries = match std::fs::read_dir(&pid_dir) {
        Ok(e) => e,
        Err(_) => return, // directory doesn't exist yet — nothing to clean
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }

        let pid_str = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let pid: u32 = match pid_str.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                continue;
            }
        };

        if crate::process::platform::is_process_alive(pid) {
            tracing::warn!(
                "Killing stale service process group {} (leftover from crashed supervisor)",
                pid
            );
            // Best-effort: send SIGKILL to the whole process group.
            // We don't wait — the processes will be cleaned up by init.
            #[cfg(unix)]
            {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                if let Ok(pgid) = i32::try_from(pid) {
                    let _ = signal::kill(Pid::from_raw(-pgid), Signal::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            {
                // On Windows, just kill the single process
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
        }

        let _ = std::fs::remove_file(&path);
    }
}

/// Entry point when this process IS the supervisor daemon.
/// Binds a port, writes PID/port files, then runs the server loop.
pub async fn run_as_daemon(workspace_root: &Path) -> Result<()> {
    let forge_dir = workspace_root.join(".forge");
    std::fs::create_dir_all(&forge_dir)?;

    // Kill any services left over from a previously crashed supervisor
    kill_stale_services(workspace_root);

    let project = crate::config::load_project(workspace_root)?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    // Ensure the listener socket is not inherited by service child processes.
    // On most Unix systems std sockets are CLOEXEC, but set it explicitly to be safe.
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = listener.as_raw_fd();
        // SAFETY: fd is valid, F_GETFD/F_SETFD are safe operations
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }

    // Write PID first, then port (clients poll for port file)
    std::fs::write(forge_dir.join("supervisor.pid"), std::process::id().to_string())?;
    std::fs::write(forge_dir.join("supervisor.port"), port.to_string())?;

    tracing::info!("Supervisor daemon starting on port {}", port);

    let result = super::server::run_server(listener, project, workspace_root.to_path_buf()).await;
    // Clean up files on orderly exit so stale-check in get_running_supervisor works correctly.
    cleanup_supervisor_files(workspace_root);
    result
}

pub fn cleanup_supervisor_files(workspace_root: &Path) {
    let pid_file = workspace_root.join(".forge/supervisor.pid");
    let port_file = workspace_root.join(".forge/supervisor.port");

    if let Err(e) = std::fs::remove_file(&pid_file)
        && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("Failed to remove supervisor PID file: {}", e);
        }
    if let Err(e) = std::fs::remove_file(&port_file)
        && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("Failed to remove supervisor port file: {}", e);
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_supervisor_running() {
        let dir = tempfile::tempdir().unwrap();
        assert!(get_running_supervisor(dir.path()).is_none());
    }

    #[test]
    fn test_stale_supervisor_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let forge_dir = dir.path().join(".forge");
        std::fs::create_dir_all(&forge_dir).unwrap();

        // Write a PID that doesn't exist
        std::fs::write(forge_dir.join("supervisor.pid"), "99999999").unwrap();
        std::fs::write(forge_dir.join("supervisor.port"), "12345").unwrap();

        assert!(get_running_supervisor(dir.path()).is_none());
        // Files should be cleaned up
        assert!(!forge_dir.join("supervisor.pid").exists());
        assert!(!forge_dir.join("supervisor.port").exists());
    }

    #[test]
    fn test_invalid_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        let forge_dir = dir.path().join(".forge");
        std::fs::create_dir_all(&forge_dir).unwrap();

        std::fs::write(forge_dir.join("supervisor.pid"), "not-a-number").unwrap();
        assert!(get_running_supervisor(dir.path()).is_none());
    }

    #[test]
    fn test_cleanup_nonexistent_files() {
        let dir = tempfile::tempdir().unwrap();
        // Should not panic
        cleanup_supervisor_files(dir.path());
    }

    #[test]
    fn test_cleanup_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let forge_dir = dir.path().join(".forge");
        std::fs::create_dir_all(&forge_dir).unwrap();

        std::fs::write(forge_dir.join("supervisor.pid"), "123").unwrap();
        std::fs::write(forge_dir.join("supervisor.port"), "456").unwrap();

        cleanup_supervisor_files(dir.path());
        assert!(!forge_dir.join("supervisor.pid").exists());
        assert!(!forge_dir.join("supervisor.port").exists());
    }
}
