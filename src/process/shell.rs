//! Platform shell dispatch.
//!
//! forge.toml lets users write `up`, `down`, `health.cmd`, and `commands.*`
//! as plain shell strings with pipes, env vars, and quoting. Each platform
//! routes these through its native shell: `sh -c <cmd>` on Unix,
//! `cmd /C <cmd>` on Windows.

/// Program + args to invoke the platform shell against `cmd`.
pub fn shell_invocation(cmd: &str) -> (&'static str, [String; 2]) {
    #[cfg(windows)]
    {
        ("cmd", ["/C".to_string(), cmd.to_string()])
    }
    #[cfg(not(windows))]
    {
        ("sh", ["-c".to_string(), cmd.to_string()])
    }
}

/// A `tokio::process::Command` preconfigured to run `cmd` through the
/// platform shell. Callers add stdio, cwd, env, and await `.status()` etc.
pub fn tokio_shell(cmd: &str) -> tokio::process::Command {
    let (program, args) = shell_invocation(cmd);
    let mut c = tokio::process::Command::new(program);
    c.args(args);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_shape() {
        let (prog, args) = shell_invocation("echo hi");
        #[cfg(windows)]
        {
            assert_eq!(prog, "cmd");
            assert_eq!(args[0], "/C");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(prog, "sh");
            assert_eq!(args[0], "-c");
        }
        assert_eq!(args[1], "echo hi");
    }

    #[tokio::test]
    async fn tokio_shell_runs_true() {
        // On Windows, `cmd /C exit 0` succeeds; on Unix, `sh -c true` succeeds.
        let cmd = if cfg!(windows) { "exit 0" } else { "true" };
        let status = tokio_shell(cmd).status().await.unwrap();
        assert!(status.success());
    }
}
