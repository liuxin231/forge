use anyhow::Result;
use std::time::Duration;

/// Per-request timeout for HTTP health checks
const HTTP_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-command timeout for cmd health checks
const CMD_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Wait for a service to become healthy.
/// `pid` is used to auto-detect the listening port when `port_hint` is not configured.
/// `cwd` is used as the working directory for `cmd` health checks (e.g. `docker compose exec`).
/// Returns the port the service was confirmed healthy on (for HTTP checks), or `None` for cmd/unconfigured checks.
pub async fn wait_healthy(
    service_name: &str,
    pid: Option<u32>,
    port_hint: Option<u16>,
    health: &Option<crate::config::service::HealthConfig>,
    timeout_secs: u64,
    cwd: &std::path::Path,
) -> Result<Option<u16>> {
    let health = match health {
        Some(h) => h,
        None => {
            tracing::debug!("No health check configured for '{}', assuming ready", service_name);
            return Ok(None);
        }
    };

    let interval = Duration::from_secs(health.interval.max(1));
    // Use the config timeout if caller passes 0, otherwise use the caller's override
    let effective_timeout = if timeout_secs > 0 {
        timeout_secs
    } else {
        health.timeout.max(1)
    };
    let timeout = Duration::from_secs(effective_timeout);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "Health check timeout for '{}' after {}s",
                service_name,
                timeout.as_secs()
            );
        }

        // For HTTP health checks, only use ports from this service's own process tree.
        // Falling back to port_hint risks hitting a *different* service already bound
        // to that port (e.g. another dev-server), producing a false-positive on the
        // wrong port.  For cmd checks the port is irrelevant.
        let effective_port = if health.http.is_some() {
            pid.and_then(|p| {
                crate::process::platform::detect_listening_ports(p)
                    .into_iter()
                    .next()
            })
        } else {
            port_hint
        };

        let healthy = probe_once(health, effective_port, cwd).await;

        if healthy {
            // TOCTOU guard: for HTTP checks, confirm the port we just probed
            // is still owned by the same PID we started. Between port lookup
            // and response arrival the service could have exited and something
            // else (another dev server, a sidecar, even the supervisor
            // looping) could have bound the port — without this check we'd
            // report the wrong service as healthy on the wrong port.
            if health.http.is_some() {
                if let (Some(p), Some(port)) = (pid, effective_port) {
                    let still_ours = crate::process::platform::detect_listening_ports(p)
                        .into_iter()
                        .any(|owned| owned == port);
                    if !still_ours {
                        tracing::warn!(
                            "'{}' port {} no longer owned by PID {} after health probe; retrying",
                            service_name,
                            port,
                            p
                        );
                        tokio::time::sleep(interval).await;
                        continue;
                    }
                }
            }
            tracing::info!("'{}' is healthy", service_name);
            // For HTTP checks return the port we actually connected to so callers
            // can display it without re-running port detection.
            let confirmed_port = if health.http.is_some() { effective_port } else { None };
            return Ok(confirmed_port);
        }

        tokio::time::sleep(interval).await;
    }
}

async fn check_http(port: Option<u16>, path: &str) -> bool {
    let port = match port {
        Some(0) | None => return false,
        Some(p) => p,
    };

    // Ensure path starts with /
    let normalized_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    let url = format!("http://127.0.0.1:{}{}", port, normalized_path);

    let client = reqwest::Client::builder()
        .timeout(HTTP_CHECK_TIMEOUT)
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return false,
    };

    match client.get(&url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(e) => {
            tracing::debug!("Health check HTTP error for {}: {}", url, e);
            false
        }
    }
}

async fn check_cmd(cmd: &crate::config::service::HealthCmd, cwd: &std::path::Path) -> bool {
    use crate::config::service::HealthCmd;

    let fut = match cmd {
        HealthCmd::Shell(s) => {
            if s.trim().is_empty() {
                return false;
            }
            crate::process::shell::tokio_shell(s)
                .current_dir(cwd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
        }
        HealthCmd::Exec(argv) => {
            let (bin, args) = match argv.as_slice() {
                [] => return false,
                [bin, rest @ ..] => (bin.as_str(), rest),
            };
            tokio::process::Command::new(bin)
                .args(args)
                .current_dir(cwd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
        }
    };

    match tokio::time::timeout(CMD_CHECK_TIMEOUT, fut).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            tracing::debug!("Health check cmd error: {}", e);
            false
        }
        Err(_) => {
            tracing::warn!("Health check cmd timed out after {}s", CMD_CHECK_TIMEOUT.as_secs());
            false
        }
    }
}

/// Run one probe against `health`: HTTP if configured, else cmd, else treat as healthy.
/// Extracted so `wait_healthy` (retry loop) and `check_health_once` (single-shot
/// from `ps`) share one definition of "what does a healthy probe look like".
async fn probe_once(
    health: &crate::config::service::HealthConfig,
    port: Option<u16>,
    cwd: &std::path::Path,
) -> bool {
    if let Some(http_path) = &health.http {
        check_http(port, http_path).await
    } else if let Some(cmd) = &health.cmd {
        check_cmd(cmd, cwd).await
    } else {
        // No check configured — validation should catch this,
        // but treat as healthy to avoid blocking.
        true
    }
}

/// Perform a single health check (no retries, no timeout loop).
/// Returns true if healthy, false otherwise.
pub async fn check_health_once(
    port: Option<u16>,
    health: &Option<crate::config::service::HealthConfig>,
    cwd: &std::path::Path,
) -> bool {
    match health {
        Some(h) => probe_once(h, port, cwd).await,
        None => true, // no health check configured = healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::service::{HealthCmd, HealthConfig};

    #[tokio::test]
    async fn test_no_health_config_returns_ok() {
        let result = wait_healthy("test", None, None, &None, 5, std::path::Path::new("/tmp")).await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn test_check_http_no_port() {
        assert!(!check_http(None, "/health").await);
    }

    #[tokio::test]
    async fn test_check_http_port_zero() {
        assert!(!check_http(Some(0), "/health").await);
    }

    #[tokio::test]
    async fn test_check_http_unreachable() {
        // Port 1 is very unlikely to have an HTTP server
        assert!(!check_http(Some(1), "/health").await);
    }

    #[tokio::test]
    async fn test_check_http_normalizes_path() {
        // Should not crash even without leading /
        assert!(!check_http(Some(1), "health").await);
    }

    #[tokio::test]
    async fn test_check_cmd_success() {
        let cwd = std::path::Path::new("/tmp");
        assert!(check_cmd(&HealthCmd::Shell("true".to_string()), cwd).await);
        assert!(check_cmd(&HealthCmd::Exec(vec!["true".to_string()]), cwd).await);
    }

    #[tokio::test]
    async fn test_check_cmd_failure() {
        let cwd = std::path::Path::new("/tmp");
        assert!(!check_cmd(&HealthCmd::Shell("false".to_string()), cwd).await);
        assert!(!check_cmd(&HealthCmd::Exec(vec!["false".to_string()]), cwd).await);
    }

    #[tokio::test]
    async fn test_check_cmd_empty() {
        let cwd = std::path::Path::new("/tmp");
        assert!(!check_cmd(&HealthCmd::Shell(String::new()), cwd).await);
        assert!(!check_cmd(&HealthCmd::Shell("   ".to_string()), cwd).await);
        assert!(!check_cmd(&HealthCmd::Exec(vec![]), cwd).await);
    }

    #[tokio::test]
    async fn test_wait_healthy_timeout() {
        let health = Some(HealthConfig {
            http: Some("/nonexistent".to_string()),
            cmd: None,

            interval: 1,
            timeout: 60,
        });
        // Use a very short timeout to make test fast
        let result = wait_healthy("test-svc", None, Some(1), &health, 1, std::path::Path::new("/tmp")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timeout"));
    }

    #[tokio::test]
    async fn test_wait_healthy_cmd_success() {
        let health = Some(HealthConfig {
            http: None,
            cmd: Some(crate::config::service::HealthCmd::Shell("true".to_string())),

            interval: 1,
            timeout: 5,
        });
        let result = wait_healthy("test-svc", None, None, &health, 5, std::path::Path::new("/tmp")).await;
        assert_eq!(result.unwrap(), None);
    }
}
