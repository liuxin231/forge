use crate::config::ProjectConfig;
use crate::supervisor::protocol::{HealthStatus, ProcessStatus, ServiceStatus};
use anyhow::Result;
use colored::Colorize;
use std::collections::HashMap;

const W_SVC: usize = 20;
const W_PORT: usize = 8;
const W_STATUS: usize = 10;
const W_HEALTH: usize = 9;
const W_PID: usize = 7;
const W_RESTARTS: usize = 7;

/// Build a border row. Each char argument is a single Unicode box-drawing char.
fn border(left: &str, fill: &str, mid: &str, right: &str, widths: &[usize]) -> String {
    let parts: Vec<String> = widths.iter().map(|w| fill.repeat(w + 2)).collect();
    format!("{}{}{}", left, parts.join(mid), right)
}

pub fn print_ps_table(statuses: &[ServiceStatus], project: &ProjectConfig) -> Result<()> {
    // Compute dynamic widths for DEPENDS ON and DIR columns
    let w_deps = statuses
        .iter()
        .map(|s| {
            project
                .services
                .get(&s.name)
                .map(|svc| {
                    if svc.config.depends_on.is_empty() {
                        1
                    } else {
                        svc.config.depends_on.join(", ").len()
                    }
                })
                .unwrap_or(1)
        })
        .max()
        .unwrap_or(10)
        .max("DEPENDS ON".len());

    let w_dir = statuses
        .iter()
        .filter_map(|s| {
            project.services.get(&s.name).map(|svc| {
                svc.dir
                    .strip_prefix(&project.root)
                    .unwrap_or(&svc.dir)
                    .to_string_lossy()
                    .len()
            })
        })
        .max()
        .unwrap_or(3)
        .max(3);

    let widths = [W_SVC, W_PORT, W_STATUS, W_HEALTH, W_PID, W_RESTARTS, w_deps, w_dir];

    // Heavy top border
    println!("{}", border("┏", "━", "┳", "┓", &widths).dimmed());
    // Heavy header
    println!(
        "{}",
        format!(
            "┃ {:<svc$} ┃ {:<port$} ┃ {:<status$} ┃ {:<health$} ┃ {:<pid$} ┃ {:<r$} ┃ {:<deps$} ┃ {:<dir$} ┃",
            "SERVICE", "PORT", "STATUS", "HEALTH", "PID", "RESTART", "DEPENDS ON", "DIR",
            svc = W_SVC, port = W_PORT, status = W_STATUS,
            health = W_HEALTH, pid = W_PID, r = W_RESTARTS,
            deps = w_deps, dir = w_dir,
        )
        .dimmed()
    );
    // Mixed separator: heavy horizontal, light vertical
    println!("{}", border("┡", "━", "╇", "┩", &widths).dimmed());

    for status in statuses {
        let port_str = status
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());

        let status_plain = match &status.status {
            ProcessStatus::Running => "running",
            ProcessStatus::Stopped => "stopped",
            ProcessStatus::Errored => "errored",
            ProcessStatus::Starting => "starting",
        };
        let sp = format!("{:<width$}", status_plain, width = W_STATUS);
        let status_col = match &status.status {
            ProcessStatus::Running => sp.green().to_string(),
            ProcessStatus::Stopped => sp.dimmed().to_string(),
            ProcessStatus::Errored => sp.red().to_string(),
            ProcessStatus::Starting => sp.yellow().to_string(),
        };

        let health_plain = match &status.health {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Unhealthy => "unhealthy",
            HealthStatus::Unknown => "unknown",
            HealthStatus::None => "-",
        };
        let hp = format!("{:<width$}", health_plain, width = W_HEALTH);
        let health_col = match &status.health {
            HealthStatus::Healthy => hp.green().to_string(),
            HealthStatus::Unhealthy => hp.red().to_string(),
            HealthStatus::Unknown => hp.yellow().to_string(),
            HealthStatus::None => hp.dimmed().to_string(),
        };

        let pid_str = status
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());

        let restarts_str = if status.restarts > 0 {
            format!("{:<width$}", status.restarts, width = W_RESTARTS)
                .yellow()
                .to_string()
        } else {
            format!("{:<width$}", "0", width = W_RESTARTS)
                .dimmed()
                .to_string()
        };

        let (deps_str, dir_str) = match project.services.get(&status.name) {
            Some(svc) => {
                let deps = if svc.config.depends_on.is_empty() {
                    "-".to_string()
                } else {
                    svc.config.depends_on.join(", ")
                };
                let dir = svc
                    .dir
                    .strip_prefix(&project.root)
                    .unwrap_or(&svc.dir)
                    .to_string_lossy()
                    .to_string();
                (deps, dir)
            }
            None => ("-".to_string(), "-".to_string()),
        };

        println!(
            "│ {:<svc$} │ {:<port$} │ {} │ {} │ {:<pid$} │ {} │ {:<deps$} │ {:<dir$} │",
            status.name, port_str, status_col, health_col, pid_str, restarts_str,
            deps_str, dir_str,
            svc = W_SVC, port = W_PORT, pid = W_PID,
            deps = w_deps, dir = w_dir,
        );
    }

    // Light bottom border
    println!("{}", border("└", "─", "┴", "┘", &widths).dimmed());
    Ok(())
}

/// Services table for `fr inspect` — shows service name, port, depends_on, dir.
pub fn print_inspect_services_table(services: &[crate::inspect::ServiceSummary]) {
    use unicode_width::UnicodeWidthStr;

    let w_svc = services.iter().map(|s| UnicodeWidthStr::width(s.name.as_str())).max().unwrap_or(7).max(7);
    let w_port = 6;
    let w_deps = services
        .iter()
        .map(|s| {
            if s.depends_on.is_empty() {
                1
            } else {
                UnicodeWidthStr::width(s.depends_on.join(", ").as_str())
            }
        })
        .max()
        .unwrap_or(10)
        .max("DEPENDS ON".len());
    let w_dir = services
        .iter()
        .map(|s| UnicodeWidthStr::width(s.dir.as_str()))
        .max()
        .unwrap_or(3)
        .max(3);

    let widths = [w_svc, w_port, w_deps, w_dir];

    eprintln!("{}", border("┏", "━", "┳", "┓", &widths).dimmed());
    eprintln!(
        "{}",
        format!(
            "┃ {:<svc$} ┃ {:<port$} ┃ {:<deps$} ┃ {:<dir$} ┃",
            "SERVICE", "PORT", "DEPENDS ON", "DIR",
            svc = w_svc, port = w_port, deps = w_deps, dir = w_dir,
        )
        .dimmed()
    );
    eprintln!("{}", border("┡", "━", "╇", "┩", &widths).dimmed());

    for svc in services {
        let port_str = svc.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
        let deps_str = if svc.depends_on.is_empty() {
            "-".to_string()
        } else {
            svc.depends_on.join(", ")
        };
        eprintln!(
            "│ {:<svc$} │ {:<port$} │ {:<deps$} │ {:<dir$} │",
            svc.name, port_str, deps_str, svc.dir,
            svc = w_svc, port = w_port, deps = w_deps, dir = w_dir,
        );
    }

    eprintln!("{}", border("└", "─", "┴", "┘", &widths).dimmed());
}

/// Final summary table printed after `up` completes.
/// Shows SERVICE, PORT, HEALTH, PID, RESTARTS, TIME, DEPENDS ON.
pub fn print_up_final_table(
    start_order: &[String],
    statuses: &HashMap<String, ServiceStatus>,
    durations: &HashMap<String, f64>,
    project: &ProjectConfig,
) -> Result<()> {
    const W_SVC: usize = 20;
    const W_PORT: usize = 6;
    const W_HEALTH: usize = 9;
    const W_PID: usize = 7;
    const W_RESTART: usize = 7;
    const W_TIME: usize = 6;

    // Compute depends_on column width from actual data
    let w_deps = start_order
        .iter()
        .map(|name| {
            project
                .services
                .get(name)
                .map(|s| {
                    if s.config.depends_on.is_empty() {
                        1 // "-"
                    } else {
                        s.config.depends_on.join(", ").len()
                    }
                })
                .unwrap_or(1)
        })
        .max()
        .unwrap_or(10)
        .max("DEPENDS ON".len());

    let widths = [W_SVC, W_PORT, W_HEALTH, W_PID, W_RESTART, W_TIME, w_deps];

    eprintln!("{}", border("┏", "━", "┳", "┓", &widths).dimmed());
    eprintln!(
        "{}",
        format!(
            "┃ {:<svc$} ┃ {:<port$} ┃ {:<health$} ┃ {:<pid$} ┃ {:<r$} ┃ {:<t$} ┃ {:<deps$} ┃",
            "SERVICE", "PORT", "HEALTH", "PID", "RESTART", "TIME", "DEPENDS ON",
            svc = W_SVC, port = W_PORT, health = W_HEALTH,
            pid = W_PID, r = W_RESTART, t = W_TIME, deps = w_deps,
        )
        .dimmed()
    );
    eprintln!("{}", border("┡", "━", "╇", "┩", &widths).dimmed());

    for name in start_order {
        let status = statuses.get(name);
        let deps = project
            .services
            .get(name)
            .map(|s| {
                if s.config.depends_on.is_empty() {
                    "-".to_string()
                } else {
                    s.config.depends_on.join(", ")
                }
            })
            .unwrap_or_else(|| "-".to_string());

        let port_str = status
            .and_then(|s| s.port)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());

        let health_plain = status
            .map(|s| match &s.health {
                HealthStatus::Healthy => "healthy",
                HealthStatus::Unhealthy => "unhealthy",
                HealthStatus::Unknown => "unknown",
                HealthStatus::None => "-",
            })
            .unwrap_or("-");
        let hp = format!("{:<width$}", health_plain, width = W_HEALTH);
        let health_col = match status.map(|s| &s.health) {
            Some(HealthStatus::Healthy) => hp.green().to_string(),
            Some(HealthStatus::Unhealthy) => hp.red().to_string(),
            Some(HealthStatus::Unknown) => hp.yellow().to_string(),
            _ => hp.dimmed().to_string(),
        };

        let pid_str = status
            .and_then(|s| s.pid)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());

        let restarts = status.map(|s| s.restarts).unwrap_or(0);
        let restarts_str = if restarts > 0 {
            format!("{:<width$}", restarts, width = W_RESTART)
                .yellow()
                .to_string()
        } else {
            format!("{:<width$}", "0", width = W_RESTART)
                .dimmed()
                .to_string()
        };

        let time_str = durations
            .get(name)
            .map(|d| {
                if *d < 10.0 {
                    format!("{:.1}s", d)
                } else {
                    format!("{:.0}s", d)
                }
            })
            .unwrap_or_else(|| "-".to_string());

        eprintln!(
            "│ {:<svc$} │ {:<port$} │ {} │ {:<pid$} │ {} │ {:<t$} │ {:<deps$} │",
            name, port_str, health_col, pid_str, restarts_str, time_str, deps,
            svc = W_SVC, port = W_PORT, pid = W_PID, t = W_TIME, deps = w_deps,
        );
    }

    eprintln!("{}", border("└", "─", "┴", "┘", &widths).dimmed());
    Ok(())
}

pub fn print_up_table(statuses: &[ServiceStatus]) -> Result<()> {
    let widths = [W_SVC, W_PORT, W_STATUS, W_HEALTH];

    eprintln!("{}", border("┏", "━", "┳", "┓", &widths).dimmed());
    eprintln!(
        "{}",
        format!(
            "┃ {:<svc$} ┃ {:<port$} ┃ {:<status$} ┃ {:<health$} ┃",
            "SERVICE", "PORT", "STATUS", "HEALTH",
            svc = W_SVC, port = W_PORT, status = W_STATUS, health = W_HEALTH,
        )
        .dimmed()
    );
    eprintln!("{}", border("┡", "━", "╇", "┩", &widths).dimmed());

    for status in statuses {
        let port_str = status
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());

        let sp = format!("{:<width$}", "running", width = W_STATUS);
        let hp_plain = match &status.health {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Unhealthy => "unhealthy",
            HealthStatus::Unknown => "unknown",
            HealthStatus::None => "-",
        };
        let hp = format!("{:<width$}", hp_plain, width = W_HEALTH);
        let health_col = match &status.health {
            HealthStatus::Healthy => hp.green().to_string(),
            HealthStatus::Unhealthy => hp.red().to_string(),
            HealthStatus::Unknown => hp.yellow().to_string(),
            HealthStatus::None => hp.dimmed().to_string(),
        };

        eprintln!(
            "│ {:<svc$} │ {:<port$} │ {} │ {} │",
            status.name, port_str, sp.green(), health_col,
            svc = W_SVC, port = W_PORT,
        );
    }

    eprintln!("{}", border("└", "─", "┴", "┘", &widths).dimmed());
    Ok(())
}
