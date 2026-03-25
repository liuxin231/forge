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

pub fn print_ps_result(response: &Response, json_mode: bool) -> Result<()> {
    match response {
        Response::Services(statuses) => {
            if json_mode {
                json::print_services(statuses)?;
            } else {
                table::print_ps_table(statuses)?;
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
