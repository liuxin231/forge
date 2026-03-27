pub mod health;
pub mod platform;
pub mod restart;
pub mod runner;

use crate::config::ProjectConfig;
use anyhow::{bail, Result};
use std::net::TcpListener;

/// Check for port conflicts before starting services
pub fn check_port_conflicts(project: &ProjectConfig, services: &[String]) -> Result<()> {
    let mut used_ports: Vec<(u16, String)> = Vec::new();

    for name in services {
        if let Some(svc) = project.services.get(name)
            && let Some(port) = svc.config.port {
                // Skip port 0 (should be caught by validation, but be safe)
                if port == 0 {
                    continue;
                }

                // Check for duplicates within our services
                if let Some((_, other)) = used_ports.iter().find(|(p, _)| *p == port) {
                    bail!(
                        "Port conflict: both '{}' and '{}' use port {}",
                        other,
                        name,
                        port
                    );
                }
                used_ports.push((port, name.clone()));
            }
    }

    // Check if ports are available on the system
    for (port, name) in &used_ports {
        if !is_port_available(*port) {
            bail!(
                "Port {} (used by '{}') is already in use by another process",
                port,
                name
            );
        }
    }

    Ok(())
}

pub fn is_port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ResolvedService, ServiceConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_svc(name: &str, port: Option<u16>) -> ResolvedService {
        ResolvedService {
            name: name.to_string(),
            config: ServiceConfig {
                port,
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
                commands: HashMap::new(),
            },
            dir: PathBuf::from("/tmp"),
        }
    }

    fn make_project(svcs: Vec<(&str, Option<u16>)>) -> ProjectConfig {
        use crate::config::workspace::{WorkspaceConfig, WorkspaceSection};
        let services: HashMap<String, ResolvedService> = svcs
            .into_iter()
            .map(|(name, port)| (name.to_string(), make_svc(name, port)))
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

    /// Get a free port for testing (bind to 0, get assigned port, release it)
    fn get_free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn test_no_conflicts() {
        let p1 = get_free_port();
        let p2 = get_free_port();
        let project = make_project(vec![("a", Some(p1)), ("b", Some(p2))]);
        let result = check_port_conflicts(&project, &["a".to_string(), "b".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_duplicate_ports() {
        let p = get_free_port();
        let project = make_project(vec![("a", Some(p)), ("b", Some(p))]);
        let result = check_port_conflicts(&project, &["a".to_string(), "b".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Port conflict"));
    }

    #[test]
    fn test_no_port_services() {
        let project = make_project(vec![("a", None), ("b", None)]);
        let result = check_port_conflicts(&project, &["a".to_string(), "b".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_nonexistent_service_skipped() {
        let p = get_free_port();
        let project = make_project(vec![("a", Some(p))]);
        let result = check_port_conflicts(&project, &["a".to_string(), "nonexistent".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_port_zero_skipped() {
        let project = make_project(vec![("a", Some(0)), ("b", Some(0))]);
        let result = check_port_conflicts(&project, &["a".to_string(), "b".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_port_available_with_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!is_port_available(port));
        drop(listener);
        assert!(is_port_available(port));
    }
}
