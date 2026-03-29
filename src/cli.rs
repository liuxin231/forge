use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fr", about = "forge — multi-service orchestration tool", version)]
pub struct Cli {
    /// Change to directory before doing anything
    #[arg(short = 'C', long = "directory", global = true)]
    pub directory: Option<PathBuf>,

    /// Verbose output (-v: show command strings, -vv: debug info)
    #[arg(short = 'v', action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start services (topological order, with health checks)
    Up {
        /// Target services/domains (empty = all services)
        targets: Vec<String>,

        /// Attach to terminal. Without value: attach services with attach=true in config (or all if none configured).
        /// With values: attach only the specified services, e.g. --attach gateway/api
        #[arg(long, num_args = 0..)]
        attach: Option<Vec<String>>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Stop services (reverse topological order)
    Down {
        /// Target services/domains (empty = all)
        targets: Vec<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Restart services
    Restart {
        /// Target services/domains (empty = all)
        targets: Vec<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show service status
    Ps {
        /// Target services/domains (empty = all)
        targets: Vec<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show service logs
    Logs {
        /// Target services/domains (empty = all)
        targets: Vec<String>,

        /// Number of recent lines to show (like tail -n)
        #[arg(short = 'n', long, default_value = "100")]
        tail: usize,

        /// Follow log output (like tail -f)
        #[arg(short = 'f', long)]
        follow: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Run a custom command (workspace-level or delegated to services)
    Run {
        /// Command name (e.g. migrate, lint, deploy)
        name: String,

        /// Target services/domains (empty = all, only for service mode)
        targets: Vec<String>,

        /// Run in parallel (override config)
        #[arg(long)]
        parallel: bool,

        /// Only show what would run, without executing
        #[arg(long)]
        dry_run: bool,

        /// Maximum number of concurrent service tasks (default: unlimited)
        #[arg(long, value_name = "N")]
        concurrency: Option<usize>,

        /// Only run for services with files changed since this git ref (e.g. HEAD~1, main)
        #[arg(long, value_name = "GIT_REF")]
        since: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Inspect project structure or a specific service (machine-friendly)
    Inspect {
        /// A specific service name to inspect (empty = whole project)
        target: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Execute an arbitrary command in a service's context (directory + env)
    Exec {
        /// Target service name (required, must be a single service)
        service: String,

        /// Command to execute (everything after --)
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },

    /// Show dependency graph of services
    Graph {
        /// Target services/domains (empty = all)
        targets: Vec<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Initialize a new forge workspace
    Init {
        /// Project directory (empty = current directory)
        path: Option<PathBuf>,

        /// Workspace name (skip interactive prompt)
        #[arg(long)]
        name: Option<String>,

        /// Workspace description (skip interactive prompt)
        #[arg(long)]
        description: Option<String>,

        /// Enable parallel startup (skip interactive prompt)
        #[arg(long)]
        parallel: Option<bool>,
    },

    /// Uninstall fr (remove binary, backups, and optionally clean up PATH)
    Uninstall,

    /// Validate forge.toml configuration files for errors and unknown fields
    Validate {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Upgrade fr to the latest release
    Upgrade {
        /// Only check for updates, do not install
        #[arg(long)]
        check: bool,
    },

    /// Internal: run as a background supervisor daemon (not for direct use)
    #[command(hide = true)]
    Supervisor {
        #[arg(long)]
        workspace_root: PathBuf,
    },

    /// Run a user-defined command by name (e.g. `fr migrate`, `fr lint`)
    #[command(external_subcommand)]
    External(Vec<String>),
}
