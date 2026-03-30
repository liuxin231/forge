use crate::config::ProjectConfig;
use crate::supervisor::protocol::{HealthStatus, ProcessStatus, ServiceStatus};
use anyhow::Result;
use comfy_table::{presets, Attribute, Cell, Color, ContentArrangement, Table};
use std::collections::HashMap;

fn term_width() -> u16 {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        if let Ok((w, _)) = crossterm::terminal::size() {
            return w;
        }
    }
    80
}

/// Truncate a string to fit within `max` display columns, appending "…" if truncated.
fn truncate(s: &str, max: usize) -> String {
    if max <= 1 {
        return ".".to_string();
    }
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max - 1; // reserve 1 char for "…"
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn status_cell(status: &ProcessStatus) -> Cell {
    match status {
        ProcessStatus::Running => Cell::new("running").fg(Color::Green),
        ProcessStatus::Stopped => Cell::new("stopped").add_attribute(Attribute::Dim),
        ProcessStatus::Errored => Cell::new("errored").fg(Color::Red),
        ProcessStatus::Starting => Cell::new("starting").fg(Color::Yellow),
    }
}

fn health_cell(health: &HealthStatus) -> Cell {
    match health {
        HealthStatus::Healthy => Cell::new("healthy").fg(Color::Green),
        HealthStatus::Unhealthy => Cell::new("unhealthy").fg(Color::Red),
        HealthStatus::Unknown => Cell::new("unknown").fg(Color::Yellow),
        HealthStatus::None => Cell::new("-").add_attribute(Attribute::Dim),
    }
}

pub fn print_ps_table(statuses: &[ServiceStatus], project: &ProjectConfig) -> Result<()> {
    let width = term_width();

    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_width(width);
    table.set_header(vec![
        "SERVICE", "PORT", "STATUS", "HEALTH", "PID", "RESTART", "DEPENDS ON", "DIR",
    ]);

    // Fixed-width columns take ~50 chars (PORT+STATUS+HEALTH+PID+RESTART + separators).
    // Remaining space is split among SERVICE, DEPENDS ON, DIR.
    let flexible_budget = (width as usize).saturating_sub(58);
    let max_svc = (flexible_budget * 35 / 100).max(10);
    let max_deps = (flexible_budget * 35 / 100).max(8);
    let max_dir = (flexible_budget * 30 / 100).max(8);

    for status in statuses {
        let port_str = status
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let pid_str = status
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let restarts_cell = if status.restarts > 0 {
            Cell::new(status.restarts.to_string()).fg(Color::Yellow)
        } else {
            Cell::new("0").add_attribute(Attribute::Dim)
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

        table.add_row(vec![
            Cell::new(truncate(&status.name, max_svc)),
            Cell::new(&port_str),
            status_cell(&status.status),
            health_cell(&status.health),
            Cell::new(&pid_str),
            restarts_cell,
            Cell::new(truncate(&deps_str, max_deps)),
            Cell::new(truncate(&dir_str, max_dir)),
        ]);
    }

    println!("{}", table);
    Ok(())
}

/// Services table for `fr inspect` — shows service name, port, depends_on, dir.
pub fn print_inspect_services_table(services: &[crate::inspect::ServiceSummary]) {
    let width = term_width();

    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_width(width);
    table.set_header(vec!["SERVICE", "PORT", "DEPENDS ON", "DIR"]);

    let flexible_budget = (width as usize).saturating_sub(20);
    let max_svc = (flexible_budget * 30 / 100).max(10);
    let max_deps = (flexible_budget * 35 / 100).max(8);
    let max_dir = (flexible_budget * 35 / 100).max(8);

    for svc in services {
        let port_str = svc
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let deps_str = if svc.depends_on.is_empty() {
            "-".to_string()
        } else {
            svc.depends_on.join(", ")
        };
        table.add_row(vec![
            Cell::new(truncate(&svc.name, max_svc)),
            Cell::new(&port_str),
            Cell::new(truncate(&deps_str, max_deps)),
            Cell::new(truncate(&svc.dir, max_dir)),
        ]);
    }

    eprintln!("{}", table);
}

/// Final summary table printed after `up` completes.
pub fn print_up_final_table(
    start_order: &[String],
    statuses: &HashMap<String, ServiceStatus>,
    durations: &HashMap<String, f64>,
    project: &ProjectConfig,
) -> Result<()> {
    let width = term_width();

    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_width(width);
    table.set_header(vec![
        "SERVICE", "PORT", "HEALTH", "PID", "RESTART", "TIME", "DEPENDS ON",
    ]);

    let flexible_budget = (width as usize).saturating_sub(48);
    let max_svc = (flexible_budget * 50 / 100).max(10);
    let max_deps = (flexible_budget * 50 / 100).max(8);

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
        let pid_str = status
            .and_then(|s| s.pid)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let restarts = status.map(|s| s.restarts).unwrap_or(0);
        let restarts_cell = if restarts > 0 {
            Cell::new(restarts.to_string()).fg(Color::Yellow)
        } else {
            Cell::new("0").add_attribute(Attribute::Dim)
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
        let health = status.map(|s| &s.health).unwrap_or(&HealthStatus::None);

        table.add_row(vec![
            Cell::new(truncate(name, max_svc)),
            Cell::new(&port_str),
            health_cell(health),
            Cell::new(&pid_str),
            restarts_cell,
            Cell::new(&time_str),
            Cell::new(truncate(&deps, max_deps)),
        ]);
    }

    eprintln!("{}", table);
    Ok(())
}

pub fn print_up_table(statuses: &[ServiceStatus]) -> Result<()> {
    let width = term_width();

    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_width(width);
    table.set_header(vec!["SERVICE", "PORT", "STATUS", "HEALTH"]);

    for status in statuses {
        let port_str = status
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        table.add_row(vec![
            Cell::new(&status.name),
            Cell::new(&port_str),
            Cell::new("running").fg(Color::Green),
            health_cell(&status.health),
        ]);
    }

    eprintln!("{}", table);
    Ok(())
}
