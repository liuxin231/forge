use anyhow::Result;

/// Send SIGTERM, wait for timeout, then SIGKILL
#[cfg(unix)]
pub async fn stop_process(pid: u32, kill_timeout_secs: u64, treekill: bool) -> Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    let pid_i32 = i32::try_from(pid)
        .map_err(|_| anyhow::anyhow!("PID {} exceeds i32 range", pid))?;

    let target_pid = if treekill {
        Pid::from_raw(-pid_i32) // Process group
    } else {
        Pid::from_raw(pid_i32)
    };

    // Send SIGTERM
    match signal::kill(target_pid, Signal::SIGTERM) {
        Ok(()) => {}
        Err(Errno::ESRCH) => {
            // Process already dead
            return Ok(());
        }
        Err(Errno::EPERM) => {
            anyhow::bail!("Permission denied sending SIGTERM to process {}", pid);
        }
        Err(e) => {
            anyhow::bail!("Failed to send SIGTERM to process {}: {}", pid, e);
        }
    }

    // Wait for process to exit
    let timeout = std::time::Duration::from_secs(kill_timeout_secs);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if !is_process_alive(pid) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Force kill
    tracing::warn!("Process {} did not exit in time, sending SIGKILL", pid);
    match signal::kill(target_pid, Signal::SIGKILL) {
        Ok(()) => {}
        Err(Errno::ESRCH) => {
            // Died between check and kill — that's fine
            return Ok(());
        }
        Err(Errno::EPERM) => {
            anyhow::bail!("Permission denied sending SIGKILL to process {}", pid);
        }
        Err(e) => {
            anyhow::bail!("Failed to send SIGKILL to process {}: {}", pid, e);
        }
    }

    Ok(())
}

#[cfg(windows)]
pub async fn stop_process(pid: u32, kill_timeout_secs: u64, _treekill: bool) -> Result<()> {
    // On Windows, use taskkill
    let status = tokio::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T"])
        .status()
        .await;

    if let Err(e) = status {
        anyhow::bail!("Failed to run taskkill for PID {}: {}", pid, e);
    }

    let timeout = std::time::Duration::from_secs(kill_timeout_secs);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if !is_process_alive(pid) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Force kill
    let _ = tokio::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
        .await;

    Ok(())
}

/// Check if a process is alive
#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal;
    use nix::unistd::Pid;

    let pid_i32 = match i32::try_from(pid) {
        Ok(v) => v,
        Err(_) => return false,
    };

    match signal::kill(Pid::from_raw(pid_i32), None) {
        Ok(()) => true,
        Err(Errno::EPERM) => true, // Process exists but we lack permission
        Err(_) => false, // ESRCH or other: process doesn't exist
    }
}

#[cfg(windows)]
pub fn is_process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH", "/FO", "CSV"])
        .output();
    match output {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            // CSV format: "name.exe","PID","..."
            // Check for exact PID match in CSV output
            s.lines().any(|line| {
                line.split(',')
                    .nth(1)
                    .and_then(|field| field.trim_matches('"').parse::<u32>().ok())
                    .map(|p| p == pid)
                    .unwrap_or(false)
            })
        }
        Err(_) => false,
    }
}

/// Kill any process currently listening on the given TCP port.
/// Best-effort: logs warnings on failure but never returns an error.
pub fn kill_port_listeners(port: u16) {
    #[cfg(unix)]
    {
        // lsof -t -i TCP:{port} -sTCP:LISTEN prints one PID per line
        let output = match std::process::Command::new("lsof")
            .args(["-t", &format!("-iTCP:{}", port), "-sTCP:LISTEN"])
            .output()
        {
            Ok(o) => o,
            Err(_) => return,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                tracing::warn!(
                    "Port {} is occupied by PID {}; killing before service start",
                    port, pid
                );
                // Try SIGTERM first, then SIGKILL
                let _ = signal::kill(Pid::from_raw(-(pid as i32)), Signal::SIGTERM);
                std::thread::sleep(std::time::Duration::from_millis(200));
                if crate::process::platform::is_process_alive(pid) {
                    let _ = signal::kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
                }
            }
        }
    }
    #[cfg(windows)]
    {
        // netstat -ano output: Proto  LocalAddr  ForeignAddr  State  PID
        let output = match std::process::Command::new("netstat")
            .args(["-ano"])
            .output()
        {
            Ok(o) => o,
            Err(_) => return,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 5 || fields[3] != "LISTENING" {
                continue;
            }
            // LocalAddr: "0.0.0.0:8080" or "[::]:8080"
            let line_port = fields[1]
                .rsplit(':')
                .next()
                .and_then(|s| s.parse::<u16>().ok());
            if line_port != Some(port) {
                continue;
            }
            if let Ok(pid) = fields[4].parse::<u32>() {
                tracing::warn!(
                    "Port {} is occupied by PID {}; killing before service start",
                    port, pid
                );
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output();
            }
        }
    }
}

/// Detect TCP ports that a process is listening on by inspecting OS state.
/// Checks the entire process tree (pid + all descendants) so that child processes
/// spawned by shell wrappers (e.g. sh → yarn → node) are also detected.
/// Returns an empty vec if no listening ports are found or detection fails.
pub fn detect_listening_ports(pid: u32) -> Vec<u16> {
    detect_ports_impl(pid)
}

/// Collect all PIDs in the process tree rooted at `root` (including `root` itself).
/// Uses `ps -axo pid=,ppid=` which is portable across Linux and macOS.
#[cfg(not(windows))]
fn get_process_tree(root: u32) -> Vec<u32> {
    use std::collections::HashMap;

    let output = match std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![root],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in stdout.lines() {
        let mut iter = line.split_whitespace();
        let pid: u32 = match iter.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let ppid: u32 = match iter.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut result = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if result.contains(&pid) {
            continue;
        }
        result.push(pid);
        if let Some(kids) = children.get(&pid) {
            queue.extend_from_slice(kids);
        }
    }
    result
}

#[cfg(target_os = "linux")]
fn detect_ports_impl(pid: u32) -> Vec<u16> {
    let pids = get_process_tree(pid);
    // Prefer /proc (no external tools) with lsof as fallback
    let ports = detect_ports_proc_tree(&pids);
    if !ports.is_empty() {
        return ports;
    }
    detect_ports_lsof(&pids)
}

#[cfg(target_os = "macos")]
fn detect_ports_impl(pid: u32) -> Vec<u16> {
    let pids = get_process_tree(pid);
    detect_ports_lsof(&pids)
}

#[cfg(windows)]
fn detect_ports_impl(pid: u32) -> Vec<u16> {
    detect_ports_netstat(pid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn detect_ports_impl(_pid: u32) -> Vec<u16> {
    vec![]
}

/// Detect listening ports via `lsof` for a set of PIDs (works on macOS and Linux).
#[cfg(not(windows))]
fn detect_ports_lsof(pids: &[u32]) -> Vec<u16> {
    if pids.is_empty() {
        return vec![];
    }
    let pid_list = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    // Use -a (AND logic) to combine -p and -i filters; without -a, lsof treats them as OR
    // on macOS and returns all internet sockets regardless of PID.
    let output = match std::process::Command::new("lsof")
        .args([
            "-a",
            "-p",
            &pid_list,
            "-iTCP",
            "-sTCP:LISTEN",
            "-nP",
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = Vec::new();

    for line in stdout.lines() {
        // Regular output format: COMMAND PID USER FD TYPE ... NAME
        // NAME field looks like "*:8080" or "127.0.0.1:9090" or "[::]:5100"
        // Skip header line
        if line.starts_with("COMMAND") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if let Some(name) = fields.last() {
            // Strip trailing " (LISTEN)" if present — it shouldn't be since -sTCP:LISTEN filters it
            if let Some(port_str) = name.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    if port > 0 && !ports.contains(&port) {
                        ports.push(port);
                    }
                }
            }
        }
    }

    ports
}

/// Detect listening ports via `netstat -ano` (Windows only).
#[cfg(windows)]
fn detect_ports_netstat(pid: u32) -> Vec<u16> {
    let output = match std::process::Command::new("netstat")
        .args(["-ano"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = Vec::new();

    for line in stdout.lines() {
        // Format: Proto  LocalAddress  ForeignAddress  State  PID
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 || fields[3] != "LISTENING" {
            continue;
        }
        let line_pid: u32 = match fields[4].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if line_pid != pid {
            continue;
        }
        // LocalAddress: "0.0.0.0:8080" or "[::]:8080" or "[::1]:8080"
        if let Some(port_str) = fields[1].rsplit(':').next() {
            if let Ok(port) = port_str.parse::<u16>() {
                if port > 0 && !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }

    ports
}

/// Detect listening ports via Linux /proc filesystem for a set of PIDs (no external tools needed).
#[cfg(target_os = "linux")]
fn detect_ports_proc_tree(pids: &[u32]) -> Vec<u16> {
    let mut all_inodes = std::collections::HashSet::new();
    for &pid in pids {
        all_inodes.extend(collect_socket_inodes(pid));
    }
    if all_inodes.is_empty() {
        return vec![];
    }

    let mut ports = Vec::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(content) = std::fs::read_to_string(path) {
            for port in parse_proc_tcp_listen(&content, &all_inodes) {
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    ports
}

/// Read socket inodes from /proc/{pid}/fd/ symlinks.
#[cfg(target_os = "linux")]
fn collect_socket_inodes(pid: u32) -> std::collections::HashSet<u64> {
    let mut inodes = std::collections::HashSet::new();
    let fd_dir = format!("/proc/{}/fd", pid);
    let entries = match std::fs::read_dir(&fd_dir) {
        Ok(e) => e,
        Err(_) => return inodes,
    };
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path()) {
            let s = target.to_string_lossy();
            // Symlink format: "socket:[123456]"
            if let Some(rest) = s.strip_prefix("socket:[") {
                if let Some(inode_str) = rest.strip_suffix(']') {
                    if let Ok(inode) = inode_str.parse::<u64>() {
                        inodes.insert(inode);
                    }
                }
            }
        }
    }
    inodes
}

/// Parse /proc/net/tcp or /proc/net/tcp6 for LISTEN entries matching given inodes.
#[cfg(target_os = "linux")]
fn parse_proc_tcp_listen(
    content: &str,
    inodes: &std::collections::HashSet<u64>,
) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in content.lines().skip(1) {
        // Fields: sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        // State 0A = LISTEN
        if fields[3] != "0A" {
            continue;
        }
        let inode: u64 = match fields[9].parse() {
            Ok(i) => i,
            Err(_) => continue,
        };
        if !inodes.contains(&inode) {
            continue;
        }
        // local_address: "XXXXXXXX:PPPP" hex — port is after the last colon
        if let Some(port_hex) = fields[1].rsplit(':').next() {
            if let Ok(port) = u16::from_str_radix(port_hex, 16) {
                if port > 0 {
                    ports.push(port);
                }
            }
        }
    }
    ports
}

/// Detect the first host port from a docker-compose.yml in the service directory.
/// Only activates when the service's `up` command involves docker compose.
pub fn detect_docker_compose_port(
    service_dir: &std::path::Path,
    up_cmd: &Option<String>,
) -> Option<u16> {
    let cmd = up_cmd.as_deref()?;
    if !cmd.contains("docker compose") && !cmd.contains("docker-compose") {
        return None;
    }

    let candidates = [
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ];

    let content = candidates
        .iter()
        .find_map(|name| std::fs::read_to_string(service_dir.join(name)).ok())?;

    parse_first_host_port(&content)
}

/// Parse the first host port from a docker-compose file's `ports:` section.
/// Supports formats: "5432:5432", "127.0.0.1:5432:5432", "5432"
fn parse_first_host_port(content: &str) -> Option<u16> {
    let mut in_ports = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect the start of a ports section
        if trimmed == "ports:" {
            in_ports = true;
            continue;
        }

        if in_ports {
            // A YAML list item under ports
            if let Some(item) = trimmed.strip_prefix("- ") {
                let val = item.trim().trim_matches('"').trim_matches('\'');
                if let Some(port) = extract_host_port(val) {
                    return Some(port);
                }
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                // Left the ports section
                in_ports = false;
            }
        }
    }
    None
}

/// Extract host port from a docker compose port mapping string.
/// "5432:5432" → 5432, "127.0.0.1:5432:5432" → 5432, "5432" → 5432
fn extract_host_port(mapping: &str) -> Option<u16> {
    // Strip protocol suffix like "/tcp", "/udp"
    let mapping = mapping.split('/').next().unwrap_or(mapping);

    let parts: Vec<&str> = mapping.split(':').collect();
    match parts.len() {
        1 => parts[0].parse().ok(),             // "5432"
        2 => parts[0].parse().ok(),             // "5432:5432"
        3 => parts[1].parse().ok(),             // "127.0.0.1:5432:5432"
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_process_is_alive() {
        let pid = std::process::id();
        assert!(is_process_alive(pid));
    }

    #[test]
    fn test_nonexistent_process_is_not_alive() {
        // PID 99999999 is extremely unlikely to exist
        assert!(!is_process_alive(99999999));
    }

    #[test]
    fn test_pid_zero_is_not_alive() {
        // PID 0 is special (kernel), should not panic
        // On macOS, kill(0, 0) sends to own process group — we just test no panic
        let _ = is_process_alive(0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_stop_nonexistent_process() {
        // Should return Ok (ESRCH handled gracefully)
        let result = stop_process(99999999, 1, false).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_first_host_port_standard() {
        let yaml = r#"
services:
  postgres:
    image: postgres:16
    ports:
      - "5432:5432"
    volumes:
      - data:/var/lib/postgresql/data
"#;
        assert_eq!(parse_first_host_port(yaml), Some(5432));
    }

    #[test]
    fn test_parse_first_host_port_with_host() {
        let yaml = r#"
services:
  redis:
    ports:
      - "127.0.0.1:6379:6379"
"#;
        assert_eq!(parse_first_host_port(yaml), Some(6379));
    }

    #[test]
    fn test_parse_first_host_port_short_form() {
        let yaml = r#"
services:
  app:
    ports:
      - "3000"
"#;
        assert_eq!(parse_first_host_port(yaml), Some(3000));
    }

    #[test]
    fn test_parse_first_host_port_no_ports() {
        let yaml = r#"
services:
  app:
    image: myapp
"#;
        assert_eq!(parse_first_host_port(yaml), None);
    }

    #[test]
    fn test_extract_host_port() {
        assert_eq!(extract_host_port("5432:5432"), Some(5432));
        assert_eq!(extract_host_port("127.0.0.1:5432:5432"), Some(5432));
        assert_eq!(extract_host_port("3000"), Some(3000));
        assert_eq!(extract_host_port("8080:80/tcp"), Some(8080));
    }
}
