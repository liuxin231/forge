pub mod json;
pub mod live_list;
pub mod table;
pub mod topo;

pub use live_list::LiveList;

use crate::config::ProjectConfig;
use crate::log::collector::LogLine;
use crate::supervisor::protocol::{Response, ServiceStatus};
use anyhow::Result;
use std::collections::HashMap;

pub fn print_up_final_table(
    start_order: &[String],
    statuses: &HashMap<String, ServiceStatus>,
    durations: &HashMap<String, f64>,
    project: &ProjectConfig,
) -> Result<()> {
    table::print_up_final_table(start_order, statuses, durations, project)
}

pub fn print_up_result(response: &Response, json_mode: bool) -> Result<()> {
    match response {
        Response::Services(statuses) => {
            if json_mode {
                json::print_services(statuses)?;
            } else {
                table::print_up_table(statuses)?;
            }
        }
        Response::Error(e) => {
            anyhow::bail!("{}", e);
        }
        other => {
            tracing::warn!("Unexpected response type in print_up_result: {:?}", other);
        }
    }
    Ok(())
}

pub fn print_down_result(response: &Response, json_mode: bool) -> Result<()> {
    match response {
        Response::Ok => {
            if json_mode {
                println!("{}", serde_json::to_string(&serde_json::json!({"status": "ok"}))?);
            } else {
                eprintln!("All services stopped.");
            }
        }
        Response::Error(e) => {
            anyhow::bail!("{}", e);
        }
        other => {
            tracing::warn!("Unexpected response type in print_down_result: {:?}", other);
        }
    }
    Ok(())
}

pub fn print_restart_result(response: &Response, json_mode: bool) -> Result<()> {
    print_up_result(response, json_mode)
}

pub fn print_ps_result(response: &Response, json_mode: bool, project: &ProjectConfig) -> Result<()> {
    match response {
        Response::Services(statuses) => {
            if json_mode {
                json::print_services(statuses)?;
            } else {
                table::print_ps_table(statuses, project)?;
            }
        }
        Response::Error(e) => {
            anyhow::bail!("{}", e);
        }
        other => {
            tracing::warn!("Unexpected response type in print_ps_result: {:?}", other);
        }
    }
    Ok(())
}

pub fn print_hints(hints: &[crate::config::workspace::HintSection]) {
    if hints.is_empty() {
        return;
    }
    use colored::Colorize;
    use unicode_width::UnicodeWidthStr;
    eprintln!();
    for section in hints {
        if let Some(title) = &section.title {
            eprintln!("  {}", title.bold());
        }
        let max_label = section.items.iter().map(|i| UnicodeWidthStr::width(i.label.as_str())).max().unwrap_or(0);
        for item in &section.items {
            let w = UnicodeWidthStr::width(item.label.as_str());
            let padding = " ".repeat(max_label - w);
            eprintln!("  {}{}  {}", item.label, padding, item.value);
        }
        eprintln!();
    }
}

pub fn print_project_inspect(overview: &crate::inspect::ProjectInspect) {
    use colored::Colorize;

    eprintln!("{}", "Workspace".bold());
    eprintln!("  Name         {}", overview.workspace.name);
    if let Some(desc) = &overview.workspace.description {
        eprintln!("  Description  {}", desc);
    }
    eprintln!("  Root         {}", overview.workspace.root);
    eprintln!();

    // Services table with depends/dir
    eprintln!("{}", "Services".bold());
    table::print_inspect_services_table(&overview.services);
    eprintln!();

    // Topology
    if !overview.topology.is_empty() {
        eprintln!("{}", "Topology (startup order)".bold());
        for (i, level) in overview.topology.iter().enumerate() {
            eprintln!("  {}. {}", i + 1, level.join(", "));
        }
        eprintln!();
    }

    // Groups
    if !overview.groups.is_empty() {
        eprintln!("{}", "Groups".bold());
        let mut groups: Vec<_> = overview.groups.iter().collect();
        groups.sort_by_key(|(k, _)| (*k).clone());
        for (name, info) in groups {
            let desc = info.description.as_deref().unwrap_or("");
            if desc.is_empty() {
                eprintln!("  {}  {}", name.cyan(), info.services.join(", "));
            } else {
                eprintln!("  {} ({})  {}", name.cyan(), desc.dimmed(), info.services.join(", "));
            }
        }
        eprintln!();
    }

    // Commands
    if !overview.commands.is_empty() {
        eprintln!("{}", "Commands".bold());
        let mut cmds: Vec<_> = overview.commands.iter().collect();
        cmds.sort_by_key(|(k, _)| (*k).clone());
        for (name, info) in cmds {
            let desc = info.description.as_deref().unwrap_or("");
            let mode_str = format!("[{}]", info.mode).dimmed();
            if desc.is_empty() {
                eprintln!("  fr run {}  {}", name.cyan(), mode_str);
            } else {
                eprintln!("  fr run {}  {} {}", name.cyan(), mode_str, desc.dimmed());
            }
        }
        eprintln!();
    }
}

pub fn print_service_inspect(detail: &crate::inspect::ServiceInspect) {
    use colored::Colorize;

    eprintln!("{}", detail.name.bold());
    eprintln!("  Directory    {}", detail.dir);
    if let Some(port) = detail.port {
        eprintln!("  Port         {}", port);
    }
    if let Some(up) = &detail.up {
        eprintln!("  Up           {}", up);
    }
    if let Some(build) = &detail.build {
        eprintln!("  Build        {}", build);
    }
    if let Some(down) = &detail.down {
        eprintln!("  Down         {}", down);
    }
    if let Some(dev) = &detail.dev {
        eprintln!("  Dev          {}", dev);
    }
    if !detail.groups.is_empty() {
        eprintln!("  Groups       {}", detail.groups.join(", "));
    }
    eprintln!();

    // Dependencies
    eprintln!("{}", "Dependencies".bold());
    if detail.depends_on.is_empty() {
        eprintln!("  (none)");
    } else {
        eprintln!("  Direct       {}", detail.depends_on.join(", "));
    }
    if !detail.transitive_deps.is_empty() {
        eprintln!("  Transitive   {}", detail.transitive_deps.join(", "));
    }
    if !detail.depended_by.is_empty() {
        eprintln!("  Depended by  {}", detail.depended_by.join(", "));
    }
    eprintln!();

    // Health
    if let Some(h) = &detail.health {
        eprintln!("{}", "Health Check".bold());
        if let Some(http) = &h.http {
            eprintln!("  HTTP         {}", http);
        }
        if let Some(cmd) = &h.cmd {
            eprintln!("  Command      {}", cmd);
        }
        eprintln!("  Interval     {}s", h.interval);
        eprintln!("  Timeout      {}s", h.timeout);
        eprintln!();
    }

    // Environment
    if !detail.env.is_empty() {
        eprintln!("{}", "Environment".bold());
        let mut vars: Vec<_> = detail.env.iter().collect();
        vars.sort_by_key(|(k, _)| (*k).clone());
        for (k, v) in vars {
            eprintln!("  {}={}", k.cyan(), v);
        }
        eprintln!();
    }

    // Commands
    if !detail.commands.is_empty() {
        eprintln!("{}", "Commands".bold());
        let mut cmds: Vec<_> = detail.commands.iter().collect();
        cmds.sort_by_key(|(k, _)| (*k).clone());
        for (name, info) in cmds {
            let desc = info.description.as_deref().unwrap_or("");
            if desc.is_empty() {
                eprintln!("  {}  {}", name.cyan(), info.run.dimmed());
            } else {
                eprintln!("  {}  {} {}", name.cyan(), info.run.dimmed(), format!("({})", desc).dimmed());
            }
        }
        eprintln!();
    }

    // Restart config
    eprintln!("{}", "Restart".bold());
    eprintln!("  Autorestart  {}", detail.restart.autorestart);
    eprintln!("  Max          {}", detail.restart.max_restarts);
    eprintln!("  Delay        {}s", detail.restart.restart_delay);
    eprintln!("  Kill timeout {}s", detail.restart.kill_timeout);
    eprintln!("  Treekill     {}", detail.restart.treekill);
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::protocol::{HealthStatus, ProcessStatus, Response, ServiceStatus};

    fn error_response(msg: &str) -> Response {
        Response::Error(msg.to_string())
    }

    fn services_response() -> Response {
        Response::Services(vec![ServiceStatus {
            name: "api".to_string(),
            port: Some(8080),
            status: ProcessStatus::Running,
            health: HealthStatus::Healthy,
            pid: Some(1234),
            restarts: 0,
        }])
    }

    // ── print_up_result ───────────────────────────────────────────────────────

    #[test]
    fn test_print_up_result_error_propagates_as_err() {
        let result = print_up_result(&error_response("start failed"), false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start failed"));
    }

    #[test]
    fn test_print_up_result_unexpected_ok_variant_returns_ok() {
        // Response::Ok is unexpected here but must not crash — only warn
        let result = print_up_result(&Response::Ok, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_up_result_json_mode_with_services_returns_ok() {
        let result = print_up_result(&services_response(), true);
        assert!(result.is_ok());
    }

    // ── print_down_result ─────────────────────────────────────────────────────

    #[test]
    fn test_print_down_result_ok_text_mode_returns_ok() {
        let result = print_down_result(&Response::Ok, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_down_result_ok_json_mode_returns_ok() {
        let result = print_down_result(&Response::Ok, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_down_result_error_propagates_as_err() {
        let result = print_down_result(&error_response("stop failed"), false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stop failed"));
    }

    #[test]
    fn test_print_down_result_unexpected_services_variant_returns_ok() {
        // Services is an unexpected variant for down; must not crash — only warn
        let result = print_down_result(&services_response(), false);
        assert!(result.is_ok());
    }

    // ── print_restart_result ──────────────────────────────────────────────────

    #[test]
    fn test_print_restart_result_delegates_to_up_path() {
        // restart reuses up display; error must still propagate
        let result = print_restart_result(&error_response("restart failed"), false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("restart failed"));
    }

    // ── print_hints ───────────────────────────────────────────────────────────

    #[test]
    fn test_print_hints_empty_slice_is_noop() {
        print_hints(&[]);
    }

    #[test]
    fn test_print_hints_with_sections_does_not_panic() {
        use crate::config::workspace::{HintItem, HintSection};
        let sections = vec![HintSection {
            title: Some("Dev servers".to_string()),
            items: vec![
                HintItem { label: "admin".to_string(), value: "http://localhost:3001".to_string() },
                HintItem { label: "api".to_string(),   value: "http://localhost:9000".to_string() },
            ],
        }];
        // Must not panic regardless of unequal label widths
        print_hints(&sections);
    }

    #[test]
    fn test_print_hints_section_without_title_does_not_panic() {
        use crate::config::workspace::{HintItem, HintSection};
        let sections = vec![HintSection {
            title: None,
            items: vec![HintItem { label: "url".to_string(), value: "http://x".to_string() }],
        }];
        print_hints(&sections);
    }
}

pub fn print_log_lines(lines: &[LogLine], json_mode: bool) -> Result<()> {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(lines)?);
    } else {
        for line in lines {
            use colored::Colorize;
            let prefix = format!("[{}]", line.service).dimmed();
            println!("{} {}", prefix, line.message);
        }
    }
    Ok(())
}
