use crate::config::ProjectConfig;
use std::collections::HashMap;

// ─── Status types ───────────────────────────────────────────────

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

/// Combined runtime status and detected port for a service.
#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub status: RuntimeStatus,
    /// The port the service is actually listening on, detected from the OS.
    /// `None` if the service is not running or has no listening TCP port.
    pub port: Option<u16>,
}

// ─── Runtime detection ─────────────────────────────────────────

/// Detect runtime status and listening port for all services via PID files + OS inspection.
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

/// Detect runtime status and port for a single service via its PID file.
pub fn detect_service_runtime(project: &ProjectConfig, name: &str) -> RuntimeInfo {
    let safe_name = name.replace('/', "-");
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
