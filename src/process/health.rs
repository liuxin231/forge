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

        // Resolve port: prefer the port the process is actually listening on (lsof),
        // fall back to the configured port hint. This handles dev servers (e.g. rsbuild)
        // that auto-switch to another port when the configured one is occupied.
        let effective_port = pid
            .and_then(|p| {
                crate::process::platform::detect_listening_ports(p)
                    .into_iter()
                    .next()
            })
            .or(port_hint);

        let healthy = if let Some(http_path) = &health.http {
            check_http(effective_port, http_path).await
        } else if let Some(cmd) = &health.cmd {
            check_cmd(cmd, cwd).await
        } else {
            // No check configured — validation should catch this,
            // but treat as healthy to avoid blocking
            true
        };

        if healthy {
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
            tokio::process::Command::new("sh")
                .args(["-c", s])
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

/// Perform a single health check (no retries, no timeout loop).
/// Returns true if healthy, false otherwise.
pub async fn check_health_once(
    port: Option<u16>,
    health: &Option<crate::config::service::HealthConfig>,
    cwd: &std::path::Path,
) -> bool {
    let health = match health {
        Some(h) => h,
        None => return true, // no health check configured = healthy
    };

    if let Some(http_path) = &health.http {
        check_http(port, http_path).await
    } else if let Some(cmd) = &health.cmd {
        check_cmd(cmd, cwd).await
    } else {
        true
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
