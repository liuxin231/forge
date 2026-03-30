use crate::config::ProjectConfig;
use crate::supervisor::protocol::{HealthStatus, ProcessStatus, ServiceStatus};
use anyhow::Result;
use comfy_table::{presets, Attribute, Cell, Color, Table};
use std::collections::HashMap;

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
    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_header(vec![
        "SERVICE", "PORT", "STATUS", "HEALTH", "PID", "RESTART", "DEPENDS ON", "DIR",
    ]);

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
            Cell::new(&status.name),
            Cell::new(&port_str),
            status_cell(&status.status),
            health_cell(&status.health),
            Cell::new(&pid_str),
            restarts_cell,
            Cell::new(&deps_str),
            Cell::new(&dir_str),
        ]);
    }

    println!("{}", table);
    Ok(())
}

/// Services table for `fr inspect` — shows service name, port, depends_on, dir.
pub fn print_inspect_services_table(services: &[crate::inspect::ServiceSummary]) {
    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_header(vec!["SERVICE", "PORT", "DEPENDS ON", "DIR"]);

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
            Cell::new(&svc.name),
            Cell::new(&port_str),
            Cell::new(&deps_str),
            Cell::new(&svc.dir),
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
    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
    table.set_header(vec![
        "SERVICE", "PORT", "HEALTH", "PID", "RESTART", "TIME", "DEPENDS ON",
    ]);

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
            Cell::new(name),
            Cell::new(&port_str),
            health_cell(health),
            Cell::new(&pid_str),
            restarts_cell,
            Cell::new(&time_str),
            Cell::new(&deps),
        ]);
    }

    eprintln!("{}", table);
    Ok(())
}

pub fn print_up_table(statuses: &[ServiceStatus]) -> Result<()> {
    let mut table = Table::new();
    table.load_preset(presets::UTF8_BORDERS_ONLY);
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
