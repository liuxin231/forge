use crate::config::ResolvedService;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, Command};

/// Start a service process, returning the Child handle
pub async fn start_service(svc: &ResolvedService, workspace_root: &Path) -> Result<Child> {
    let (program, args) = build_command(svc, workspace_root).await?;
    let cwd = get_working_dir(svc)?;

    tracing::info!("Starting '{}': {} {}", svc.name, program, args.join(" "));

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(&cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);

    // Inject environment variables
    for (key, value) in &svc.config.env {
        cmd.env(key, value);
    }

    // Set process group on Unix so we can kill the whole group
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("Failed to start service '{}': command='{}', cwd='{}'", svc.name, program, cwd.display()))?;

    // Write PID file — warn on failure but don't abort (process is already running)
    if let Some(pid) = child.id()
        && let Err(e) = write_pid_file(workspace_root, &svc.name, pid) {
            tracing::warn!("Failed to write PID file for '{}': {}", svc.name, e);
        }

    Ok(child)
}

async fn build_command(svc: &ResolvedService, _workspace_root: &Path) -> Result<(String, Vec<String>)> {
    let up_cmd = svc
        .config
        .up
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Service '{}' has no 'up' field", svc.name))?;

    #[cfg(windows)]
    return Ok(("cmd".to_string(), vec!["/C".to_string(), up_cmd.clone()]));

    #[cfg(not(windows))]
    Ok(("sh".to_string(), vec!["-c".to_string(), up_cmd.clone()]))
}

fn get_working_dir(svc: &ResolvedService) -> Result<std::path::PathBuf> {
    let dir = if let Some(cwd) = &svc.config.cwd {
        if Path::new(cwd).is_absolute() {
            std::path::PathBuf::from(cwd)
        } else {
            svc.dir.join(cwd)
        }
    } else {
        svc.dir.clone()
    };

    if !dir.is_dir() {
        anyhow::bail!(
            "Working directory for service '{}' does not exist: {}",
            svc.name,
            dir.display()
        );
    }

    Ok(dir)
}

/// Sanitize a service name for use as a filename — replace path separators
/// and any characters that are problematic in file paths.
pub fn sanitize_service_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' | ' ' | '\n' | '\r' => '-',
            '.' if name.starts_with('.') => '-', // avoid hidden files
            c => c,
        })
        .collect()
}

fn write_pid_file(workspace_root: &Path, service_name: &str, pid: u32) -> Result<()> {
    let pid_dir = workspace_root.join(".forge/pids");
    std::fs::create_dir_all(&pid_dir)?;
    let safe_name = sanitize_service_name(service_name);
    let pid_file = pid_dir.join(format!("{}.pid", safe_name));
    std::fs::write(&pid_file, pid.to_string())?;
    Ok(())
}

pub fn remove_pid_file(workspace_root: &Path, service_name: &str) {
    let safe_name = sanitize_service_name(service_name);
    let pid_file = workspace_root.join(format!(".forge/pids/{}.pid", safe_name));
    if let Err(e) = std::fs::remove_file(&pid_file) {
        tracing::debug!("Could not remove PID file {}: {}", pid_file.display(), e);
    }
}

/// Shell-like word splitting with basic quoting support.
/// Returns an error if quotes are not properly closed.
#[cfg(test)]
fn shell_words(s: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    for c in s.chars() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    words.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if in_single_quote {
        anyhow::bail!("Unclosed single quote in: {}", s);
    }
    if in_double_quote {
        anyhow::bail!("Unclosed double quote in: {}", s);
    }

    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience constructor for tests — avoids repeating the full ServiceConfig struct.
    fn make_svc(dir: &std::path::Path, up: Option<&str>, cwd: Option<&str>) -> ResolvedService {
        ResolvedService {
            name: "test".to_string(),
            config: crate::config::ServiceConfig {
                port: None,
                groups: vec![],
                depends_on: vec![],
                health: None,
                env: std::collections::HashMap::new(),
                env_file: None,
                up: up.map(|s| s.to_string()),
                down: None,
                build: None,
                dev: None,
                logs: None,
                cwd: cwd.map(|s| s.to_string()),
                args: None,
                autorestart: true,
                max_restarts: 10,
                restart_delay: 3,
                kill_timeout: 10,
                treekill: true,
                attach: false,
                max_memory: None,
                mode: crate::config::ServiceMode::Service,
                commands: std::collections::HashMap::new(),
            },
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn test_shell_words_simple() {
        assert_eq!(shell_words("a b c").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_shell_words_empty() {
        assert!(shell_words("").unwrap().is_empty());
        assert!(shell_words("   ").unwrap().is_empty());
    }

    #[test]
    fn test_shell_words_single_quotes() {
        assert_eq!(
            shell_words("'hello world' foo").unwrap(),
            vec!["hello world", "foo"]
        );
    }

    #[test]
    fn test_shell_words_double_quotes() {
        assert_eq!(
            shell_words(r#""hello world" foo"#).unwrap(),
            vec!["hello world", "foo"]
        );
    }

    #[test]
    fn test_shell_words_mixed_quotes() {
        assert_eq!(
            shell_words(r#"'a b' "c d" e"#).unwrap(),
            vec!["a b", "c d", "e"]
        );
    }

    #[test]
    fn test_shell_words_unclosed_single_quote() {
        let result = shell_words("'hello world");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unclosed single quote"));
    }

    #[test]
    fn test_shell_words_unclosed_double_quote() {
        let result = shell_words(r#""hello world"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unclosed double quote"));
    }

    #[test]
    fn test_shell_words_tabs() {
        assert_eq!(shell_words("a\tb\tc").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_shell_words_consecutive_spaces() {
        assert_eq!(shell_words("a    b").unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn test_get_working_dir_default() {
        let dir = tempfile::tempdir().unwrap();
        let svc = ResolvedService {
            name: "test".to_string(),
            config: crate::config::ServiceConfig {
                port: None,
                groups: vec![],
                depends_on: vec![],
                health: None,
                env: std::collections::HashMap::new(),
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
                mode: crate::config::ServiceMode::Service,
                commands: std::collections::HashMap::new(),
            },
            dir: dir.path().to_path_buf(),
        };
        let result = get_working_dir(&svc).unwrap();
        assert_eq!(result, dir.path());
    }

    #[test]
    fn test_get_working_dir_nonexistent() {
        let svc = ResolvedService {
            name: "test".to_string(),
            config: crate::config::ServiceConfig {
                port: None,
                groups: vec![],
                depends_on: vec![],
                health: None,
                env: std::collections::HashMap::new(),
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
                mode: crate::config::ServiceMode::Service,
                commands: std::collections::HashMap::new(),
            },
            dir: std::path::PathBuf::from("/nonexistent/path"),
        };
        let result = get_working_dir(&svc);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_write_and_remove_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        write_pid_file(dir.path(), "test/api", 12345).unwrap();

        let pid_file = dir.path().join(".forge/pids/test-api.pid");
        assert!(pid_file.exists());
        assert_eq!(std::fs::read_to_string(&pid_file).unwrap(), "12345");

        remove_pid_file(dir.path(), "test/api");
        assert!(!pid_file.exists());
    }

    // ── sanitize_service_name ─────────────────────────────────────────────────

    #[test]
    fn test_sanitize_plain_name_unchanged() {
        assert_eq!(sanitize_service_name("api"), "api");
        assert_eq!(sanitize_service_name("my-service"), "my-service");
    }

    #[test]
    fn test_sanitize_forward_slash_becomes_dash() {
        assert_eq!(sanitize_service_name("apps/api"), "apps-api");
        assert_eq!(sanitize_service_name("a/b/c"), "a-b-c");
    }

    #[test]
    fn test_sanitize_backslash_becomes_dash() {
        assert_eq!(sanitize_service_name("apps\\api"), "apps-api");
    }

    #[test]
    fn test_sanitize_special_chars_become_dashes() {
        assert_eq!(sanitize_service_name("my:service"), "my-service");
        assert_eq!(sanitize_service_name("my*service"), "my-service");
        assert_eq!(sanitize_service_name("my?service"), "my-service");
        assert_eq!(sanitize_service_name("my service"), "my-service");
        assert_eq!(sanitize_service_name("my\nservice"), "my-service");
        assert_eq!(sanitize_service_name("my\rservice"), "my-service");
        assert_eq!(sanitize_service_name("my\0service"), "my-service");
    }

    #[test]
    fn test_sanitize_leading_dot_becomes_dash() {
        assert_eq!(sanitize_service_name(".hidden"), "-hidden");
    }

    #[test]
    fn test_sanitize_inner_dot_preserved() {
        // Only the FIRST character is checked for leading dot
        assert_eq!(sanitize_service_name("my.service"), "my.service");
        assert_eq!(sanitize_service_name("a.b.c"), "a.b.c");
    }

    #[test]
    fn test_sanitize_empty_string() {
        assert_eq!(sanitize_service_name(""), "");
    }

    // ── build_command ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_build_command_uses_sh_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        let svc = make_svc(dir.path(), Some("echo hello"), None);
        let (program, args) = build_command(&svc, dir.path()).await.unwrap();
        #[cfg(not(windows))]
        {
            assert_eq!(program, "sh");
            assert_eq!(args, vec!["-c".to_string(), "echo hello".to_string()]);
        }
        #[cfg(windows)]
        {
            assert_eq!(program, "cmd");
            assert!(args.contains(&"/C".to_string()));
            assert!(args.contains(&"echo hello".to_string()));
        }
    }

    #[tokio::test]
    async fn test_build_command_no_up_field_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let svc = make_svc(dir.path(), None, None);
        let result = build_command(&svc, dir.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no 'up' field"));
    }

    // ── get_working_dir ───────────────────────────────────────────────────────

    #[test]
    fn test_get_working_dir_relative_cwd_resolved_against_service_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let svc = make_svc(dir.path(), Some("echo"), Some("subdir"));
        let result = get_working_dir(&svc).unwrap();
        assert_eq!(result, sub);
    }

    #[test]
    fn test_get_working_dir_absolute_cwd_used_directly() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let svc = make_svc(dir.path(), Some("echo"), Some(other.path().to_str().unwrap()));
        let result = get_working_dir(&svc).unwrap();
        assert_eq!(result, other.path());
    }

    #[test]
    fn test_get_working_dir_relative_cwd_nonexistent_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let svc = make_svc(dir.path(), Some("echo"), Some("does_not_exist"));
        let result = get_working_dir(&svc);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    // ── remove_pid_file ───────────────────────────────────────────────────────

    #[test]
    fn test_remove_pid_file_nonexistent_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // File was never created — should silently succeed
        remove_pid_file(dir.path(), "ghost-service");
    }

    #[test]
    fn test_write_pid_file_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        // .forge/pids/ doesn't exist yet
        write_pid_file(dir.path(), "svc", 9999).unwrap();
        let pid_file = dir.path().join(".forge/pids/svc.pid");
        assert!(pid_file.exists());
        assert_eq!(std::fs::read_to_string(&pid_file).unwrap(), "9999");
    }
}
