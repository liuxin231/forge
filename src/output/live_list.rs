use colored::Colorize;
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Instant;

#[derive(Clone)]
struct Row {
    port: Option<u16>,
    icon: &'static str,
    label: &'static str,
    state: RowState,
    started_at: Option<Instant>,
    elapsed_secs: Option<f64>,
}

#[derive(Clone, PartialEq)]
enum RowState {
    Pending,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// A live-updating flat table display for `up` / `down` progress.
pub struct LiveList {
    order: Vec<String>,
    rows: HashMap<String, Row>,
    lines_printed: usize,
    col_service: usize,
}

const MAX_COL_SERVICE: usize = 30;
const COL_PORT: usize = 8;
const COL_STATUS: usize = 18;

/// Truncate a string to `max` chars, appending "…" if truncated.
fn truncate_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(1);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

impl LiveList {
    pub fn new(order: Vec<String>) -> Self {
        let col_service = order
            .iter()
            .map(|n| n.chars().count())
            .max()
            .unwrap_or(7)
            .max(7) // at least wide enough for "SERVICE" header
            .min(MAX_COL_SERVICE);

        let rows = order
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    Row {
                        port: None,
                        icon: "·",
                        label: "pending",
                        state: RowState::Pending,
                        started_at: None,
                        elapsed_secs: None,
                    },
                )
            })
            .collect();

        Self {
            order,
            rows,
            lines_printed: 0,
            col_service,
        }
    }

    pub fn set_starting(&mut self, name: &str) {
        if let Some(row) = self.rows.get_mut(name) {
            row.icon = "◐";
            row.label = "starting";
            row.state = RowState::Running;
            row.started_at = Some(Instant::now());
        }
    }

    pub fn set_healthy(&mut self, name: &str, port: Option<u16>) {
        if let Some(row) = self.rows.get_mut(name) {
            row.icon = "●";
            row.label = "healthy";
            row.port = port;
            row.state = RowState::Running;
            if let Some(started) = row.started_at {
                row.elapsed_secs = Some(started.elapsed().as_secs_f64());
            }
        }
    }

    pub fn set_unhealthy(&mut self, name: &str) {
        if let Some(row) = self.rows.get_mut(name) {
            row.icon = "✗";
            row.label = "unhealthy";
            row.state = RowState::Failed;
            if let Some(started) = row.started_at {
                row.elapsed_secs = Some(started.elapsed().as_secs_f64());
            }
        }
    }

    pub fn set_stopping(&mut self, name: &str) {
        if let Some(row) = self.rows.get_mut(name) {
            row.icon = "◐";
            row.label = "stopping";
            row.state = RowState::Stopping;
        }
    }

    pub fn set_stopped(&mut self, name: &str) {
        if let Some(row) = self.rows.get_mut(name) {
            row.icon = "○";
            row.label = "stopped";
            row.port = None;
            row.state = RowState::Stopped;
        }
    }

    pub fn set_failed(&mut self, name: &str) {
        if let Some(row) = self.rows.get_mut(name) {
            row.icon = "✗";
            row.label = "failed";
            row.state = RowState::Failed;
        }
    }

    /// Return per-service elapsed seconds (only for services that started).
    pub fn elapsed_secs(&self) -> HashMap<String, f64> {
        self.rows
            .iter()
            .filter_map(|(name, row)| row.elapsed_secs.map(|d| (name.clone(), d)))
            .collect()
    }

    /// Clear the live table from the terminal (move cursor up and erase).
    pub fn clear(&self) {
        let stderr = io::stderr();
        let mut out = stderr.lock();
        if self.lines_printed > 0 {
            let _ = write!(out, "\x1b[{}A\x1b[0J", self.lines_printed);
        }
        let _ = out.flush();
    }

    pub fn render(&mut self) {
        let stderr = io::stderr();
        let mut out = stderr.lock();

        if self.lines_printed > 0 {
            let _ = write!(out, "\x1b[{}A\x1b[0J", self.lines_printed);
        }

        // inner width: 1 space + SERVICE + 3 spaces + PORT + 3 spaces + STATUS + 1 space
        let inner = 1 + self.col_service + 3 + COL_PORT + 3 + COL_STATUS + 1;
        let top = format!("┌{}┐", "─".repeat(inner));
        let sep = format!("╞{}╡", "═".repeat(inner));
        let bot = format!("└{}┘", "─".repeat(inner));

        let _ = writeln!(out, "{}", top.dimmed());
        let _ = writeln!(
            out,
            "{}",
            format!(
                "│ {:<svc$}   {:<port$}   {:<status$} │",
                "SERVICE", "PORT", "STATUS",
                svc = self.col_service, port = COL_PORT, status = COL_STATUS,
            )
            .dimmed()
        );
        let _ = writeln!(out, "{}", sep.dimmed());

        for name in &self.order {
            let row = match self.rows.get(name) {
                Some(r) => r,
                None => continue,
            };

            let name_display = truncate_to(name, self.col_service);
            let port_str = row
                .port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string());

            let status_plain = if row.label == "starting" || row.label == "stopping" {
                if let Some(started) = row.started_at {
                    let secs = started.elapsed().as_secs();
                    format!("{} {} ({}s)", row.icon, row.label, secs)
                } else {
                    format!("{} {}", row.icon, row.label)
                }
            } else if let Some(elapsed) = row.elapsed_secs {
                if elapsed < 10.0 {
                    format!("{} {} ({:.1}s)", row.icon, row.label, elapsed)
                } else {
                    format!("{} {} ({:.0}s)", row.icon, row.label, elapsed)
                }
            } else {
                format!("{} {}", row.icon, row.label)
            };
            let sp = format!("{:<width$}", status_plain, width = COL_STATUS);
            let status_col = match row.state {
                RowState::Running => sp.green().to_string(),
                RowState::Stopping => sp.yellow().to_string(),
                RowState::Stopped => sp.dimmed().to_string(),
                RowState::Failed => sp.red().to_string(),
                RowState::Pending => sp.dimmed().to_string(),
            };

            let _ = writeln!(
                out,
                "│ {:<svc$}   {:<port$}   {} │",
                name_display, port_str, status_col,
                svc = self.col_service, port = COL_PORT,
            );
        }

        let _ = writeln!(out, "{}", bot.dimmed());
        let _ = out.flush();

        // top border + header + separator + rows + bottom border
        self.lines_printed = 4 + self.order.len();
    }

    pub fn print_summary(&self, verb: &str) {
        let stderr = io::stderr();
        let mut out = stderr.lock();

        let total = self.order.len();
        let healthy = self
            .rows
            .values()
            .filter(|r| r.state == RowState::Running)
            .count();
        let stopped = self
            .rows
            .values()
            .filter(|r| r.state == RowState::Stopped)
            .count();
        let failed = self
            .rows
            .values()
            .filter(|r| r.state == RowState::Failed)
            .count();

        let _ = writeln!(out);
        if failed > 0 {
            let _ = writeln!(
                out,
                "  {} {}/{} healthy, {} failed",
                "!".yellow(),
                healthy,
                total,
                failed
            );
        } else if verb == "down" {
            let _ = writeln!(
                out,
                "  {} {}/{} services stopped",
                "✓".green(),
                stopped,
                total
            );
        } else {
            let _ = writeln!(
                out,
                "  {} {}/{} services started",
                "✓".green(),
                healthy,
                total
            );
        }
        let _ = out.flush();
    }
}
