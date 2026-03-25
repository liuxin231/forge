use crate::config::ProjectConfig;
use colored::Colorize;
use std::collections::HashMap;
use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

/// Box width for each service node
pub const BOX_WIDTH: usize = 24;
/// Horizontal gap between boxes
pub const BOX_GAP: usize = 2;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum ServiceState {
    Pending,
    Starting,
    Healthy,
    Unhealthy,
    Failed(String),
    Stopping,
    Stopped,
}

/// Tracks layout position for a service box
struct BoxLayout {
    row: usize,
    col: usize,
    #[allow(dead_code)]
    name: String,
    port: Option<u16>,
}

/// Renders and dynamically updates a topology graph in the terminal
pub struct TopoRenderer {
    layouts: HashMap<String, BoxLayout>,
    total_rows: usize,
    states: HashMap<String, ServiceState>,
}

/// Truncate a string to fit within a display width, using unicode-aware width calculation.
/// Strips ANSI escape codes for width calculation.
pub fn truncate_to_display_width(s: &str, max_width: usize) -> String {
    // Strip ANSI codes for width calculation
    let plain = strip_ansi_codes(s);
    let current_width = UnicodeWidthStr::width(plain.as_str());

    if current_width <= max_width {
        return s.to_string();
    }

    // For colored strings, we need to work with the plain text
    let mut result = String::new();
    let mut width = 0;
    for ch in plain.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        result.push(ch);
        width += ch_width;
    }
    result
}

/// Pad a string to a target display width (unicode-aware)
pub fn pad_to_display_width(s: &str, target_width: usize) -> String {
    let plain = strip_ansi_codes(s);
    let current_width = UnicodeWidthStr::width(plain.as_str());
    if current_width >= target_width {
        truncate_to_display_width(s, target_width)
    } else {
        let padding = target_width - current_width;
        format!("{}{}", s, " ".repeat(padding))
    }
}

/// Strip ANSI escape codes from a string
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        result.push(ch);
    }
    result
}

impl TopoRenderer {
    /// Create a new renderer and draw the initial topology
    pub fn new(levels: &[Vec<String>], project: &ProjectConfig) -> Self {
        let mut layouts = HashMap::new();
        let mut current_row = 0;

        for (level_idx, level) in levels.iter().enumerate() {
            current_row += 1; // level label line

            if level_idx > 0 {
                current_row += 1; // connector arrows
            }

            let box_start_row = current_row;

            for (svc_idx, name) in level.iter().enumerate() {
                let col = svc_idx * (BOX_WIDTH + BOX_GAP);

                layouts.insert(
                    name.clone(),
                    BoxLayout {
                        row: box_start_row,
                        col,
                        name: name.clone(),
                        port: None, // populated after service starts via set_port()
                    },
                );
            }

            current_row += 4; // box: border + name + status + border
        }

        let total_rows = current_row;

        let mut states = HashMap::new();
        for name in layouts.keys() {
            states.insert(name.clone(), ServiceState::Pending);
        }

        let renderer = TopoRenderer {
            layouts,
            total_rows,
            states,
        };

        renderer.render_full(levels, project);
        renderer
    }

    /// Render the full topology graph
    fn render_full(&self, levels: &[Vec<String>], project: &ProjectConfig) {
        let stderr = io::stderr();
        let mut out = stderr.lock();

        for (level_idx, level) in levels.iter().enumerate() {
            if level_idx > 0 {
                let mut arrow_line = String::new();
                for (svc_idx, name) in level.iter().enumerate() {
                    let col = svc_idx * (BOX_WIDTH + BOX_GAP);
                    let target_pos = col + BOX_WIDTH / 2;
                    while arrow_line.len() < target_pos {
                        arrow_line.push(' ');
                    }
                    let svc = project.services.get(name);
                    let has_dep = svc
                        .map(|s| !s.config.depends_on.is_empty())
                        .unwrap_or(false);
                    if has_dep {
                        if target_pos < arrow_line.len() {
                            // Replace char at position safely
                            let mut new_line = String::new();
                            for (i, c) in arrow_line.chars().enumerate() {
                                if i == target_pos {
                                    new_line.push('|');
                                } else {
                                    new_line.push(c);
                                }
                            }
                            arrow_line = new_line;
                        } else {
                            arrow_line.push('|');
                        }
                    }
                }
                let _ = writeln!(out, "{}", arrow_line.dimmed());
            }

            let level_label = format!(" Level {} ", level_idx);
            let _ = writeln!(out, "{}", level_label.dimmed());

            // Box top borders
            let mut top_line = String::new();
            for (svc_idx, _name) in level.iter().enumerate() {
                let col = svc_idx * (BOX_WIDTH + BOX_GAP);
                while top_line.len() < col {
                    top_line.push(' ');
                }
                top_line.push('+');
                for _ in 0..BOX_WIDTH - 2 {
                    top_line.push('-');
                }
                top_line.push('+');
            }
            let _ = writeln!(out, "{}", top_line);

            // Box content: name + icon
            let mut name_line = String::new();
            for (svc_idx, name) in level.iter().enumerate() {
                let col = svc_idx * (BOX_WIDTH + BOX_GAP);
                while name_line.len() < col {
                    name_line.push(' ');
                }
                let icon = state_icon_plain(&self.states[name]);
                let content = format!("{} {}", icon, name);
                let inner_width = BOX_WIDTH - 4; // "| " + " |"
                let padded_name = pad_to_display_width(&content, inner_width);
                name_line.push_str(&format!("| {} |", padded_name));
            }
            let _ = writeln!(out, "{}", name_line);

            // Box content: type/port + status
            let mut status_line = String::new();
            for (svc_idx, name) in level.iter().enumerate() {
                let col = svc_idx * (BOX_WIDTH + BOX_GAP);
                while status_line.len() < col {
                    status_line.push(' ');
                }
                let layout = &self.layouts[name];
                let port_str = layout
                    .port
                    .map(|p| format!(":{}", p))
                    .unwrap_or_default();
                let state_str = state_label_plain(&self.states[name]);
                let inner_width = BOX_WIDTH - 4;
                let content = format!("{} {}", port_str, state_str);
                let padded = pad_to_display_width(&content, inner_width);
                status_line.push_str(&format!("| {} |", padded));
            }
            let _ = writeln!(out, "{}", status_line.dimmed());

            // Box bottom borders
            let mut bottom_line = String::new();
            for (svc_idx, _name) in level.iter().enumerate() {
                let col = svc_idx * (BOX_WIDTH + BOX_GAP);
                while bottom_line.len() < col {
                    bottom_line.push(' ');
                }
                bottom_line.push('+');
                for _ in 0..BOX_WIDTH - 2 {
                    bottom_line.push('-');
                }
                bottom_line.push('+');
            }
            let _ = writeln!(out, "{}", bottom_line);
        }

        let _ = out.flush();
    }

    /// Update the detected port for a service.
    /// Call this after a service has started and its port is known.
    pub fn set_port(&mut self, name: &str, port: Option<u16>) {
        if let Some(layout) = self.layouts.get_mut(name) {
            layout.port = port;
        }
    }

    /// Update a service's state
    pub fn update_state(&mut self, name: &str, state: ServiceState) {
        self.states.insert(name.to_string(), state);

        let layout = match self.layouts.get(name) {
            Some(l) => l,
            None => return,
        };

        let stderr = io::stderr();
        let mut out = stderr.lock();

        let _ = write!(out, "\x1b[s"); // save cursor

        let name_row = layout.row + 1;
        let lines_up = self.total_rows - name_row;
        if lines_up > 0 {
            let _ = write!(out, "\x1b[{}A", lines_up);
        }
        let _ = write!(out, "\r");

        let col = layout.col;
        if col > 0 {
            let _ = write!(out, "\x1b[{}C", col);
        }

        let icon = state_icon_plain(&self.states[name]);
        let port_str = layout.port.map(|p| format!(":{}", p)).unwrap_or_default();
        let inner_width = BOX_WIDTH - 4;

        // Name line
        let content = format!("{} {}", icon, name);
        let padded = pad_to_display_width(&content, inner_width);
        let colored_name = match &self.states[name] {
            ServiceState::Healthy => format!("| {} |", padded).green().to_string(),
            ServiceState::Failed(_) | ServiceState::Unhealthy => format!("| {} |", padded).red().to_string(),
            ServiceState::Starting => format!("| {} |", padded).yellow().to_string(),
            ServiceState::Stopping => format!("| {} |", padded).yellow().to_string(),
            ServiceState::Stopped => format!("| {} |", padded).dimmed().to_string(),
            _ => format!("| {} |", padded),
        };
        let _ = write!(out, "{}", colored_name);

        let _ = writeln!(out);
        if col > 0 {
            let _ = write!(out, "\x1b[{}C", col);
        }

        // Status line
        let state_str = state_label_plain(&self.states[name]);
        let status_content = format!("{} {}", port_str, state_str);
        let padded_status = pad_to_display_width(&status_content, inner_width);
        let colored_status = match &self.states[name] {
            ServiceState::Failed(_) | ServiceState::Unhealthy => {
                format!("| {} |", padded_status).red().dimmed().to_string()
            }
            _ => format!("| {} |", padded_status).dimmed().to_string(),
        };
        let _ = write!(out, "{}", colored_status);

        let _ = write!(out, "\x1b[u");
        let _ = out.flush();
    }

    /// Print a summary line after the graph
    pub fn print_summary(&self) {
        let stderr = io::stderr();
        let mut out = stderr.lock();

        let total = self.states.len();
        let healthy = self.states.values().filter(|s| matches!(s, ServiceState::Healthy)).count();
        let stopped = self.states.values().filter(|s| matches!(s, ServiceState::Stopped)).count();
        let failed = self.states.values().filter(|s| matches!(s, ServiceState::Failed(_) | ServiceState::Unhealthy)).count();

        let _ = writeln!(out);
        if stopped > 0 && failed == 0 {
            let _ = writeln!(
                out,
                "  {} {}/{} services stopped",
                "OK".green(),
                stopped,
                total
            );
        } else if failed == 0 {
            let _ = writeln!(
                out,
                "  {} {}/{} services started",
                "OK".green(),
                healthy,
                total
            );
        } else {
            let _ = writeln!(
                out,
                "  {} {}/{} healthy, {} failed",
                "!".yellow(),
                healthy,
                total,
                failed
            );
        }
        let _ = out.flush();
    }
}

/// Plain text icons (no ANSI) for rendering
fn state_icon_plain(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Pending => ".",
        ServiceState::Starting => "*",
        ServiceState::Healthy => "+",
        ServiceState::Unhealthy => "x",
        ServiceState::Failed(_) => "x",
        ServiceState::Stopping => "~",
        ServiceState::Stopped => "-",
    }
}

/// Plain text labels for rendering
fn state_label_plain(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Pending => "pending",
        ServiceState::Starting => "starting",
        ServiceState::Healthy => "healthy",
        ServiceState::Unhealthy => "unhealthy",
        ServiceState::Failed(_) => "failed",
        ServiceState::Stopping => "stopping",
        ServiceState::Stopped => "stopped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_to_display_width_ascii() {
        assert_eq!(truncate_to_display_width("hello", 3), "hel");
        assert_eq!(truncate_to_display_width("hi", 5), "hi");
        assert_eq!(truncate_to_display_width("", 5), "");
    }

    #[test]
    fn test_truncate_to_display_width_unicode() {
        // Chinese chars are typically 2 columns wide
        let s = "你好世界";
        let result = truncate_to_display_width(s, 4);
        assert_eq!(result, "你好"); // 4 columns
    }

    #[test]
    fn test_pad_to_display_width() {
        assert_eq!(pad_to_display_width("hi", 5), "hi   ");
        assert_eq!(pad_to_display_width("hello", 5), "hello");
        assert_eq!(pad_to_display_width("toolong", 4), "tool");
    }

    #[test]
    fn test_strip_ansi_codes() {
        assert_eq!(strip_ansi_codes("\x1b[32mgreen\x1b[0m"), "green");
        assert_eq!(strip_ansi_codes("no color"), "no color");
        assert_eq!(strip_ansi_codes(""), "");
    }

    #[test]
    fn test_state_icon_plain() {
        assert_eq!(state_icon_plain(&ServiceState::Pending), ".");
        assert_eq!(state_icon_plain(&ServiceState::Starting), "*");
        assert_eq!(state_icon_plain(&ServiceState::Healthy), "+");
        assert_eq!(state_icon_plain(&ServiceState::Unhealthy), "x");
        assert_eq!(state_icon_plain(&ServiceState::Failed("err".to_string())), "x");
    }

    #[test]
    fn test_state_label_plain() {
        assert_eq!(state_label_plain(&ServiceState::Pending), "pending");
        assert_eq!(state_label_plain(&ServiceState::Healthy), "healthy");
    }

    #[test]
    fn test_long_service_name_no_panic() {
        let long_name = "a".repeat(100);
        let inner_width = BOX_WIDTH - 4;
        let content = format!("+ {}", long_name);
        let padded = pad_to_display_width(&content, inner_width);
        // Should not panic, and should be truncated
        assert!(UnicodeWidthStr::width(strip_ansi_codes(&padded).as_str()) <= inner_width);
    }

    #[test]
    fn test_unicode_service_name_no_panic() {
        let name = "日本語サービス";
        let inner_width = BOX_WIDTH - 4;
        let content = format!("+ {}", name);
        let padded = pad_to_display_width(&content, inner_width);
        assert!(UnicodeWidthStr::width(strip_ansi_codes(&padded).as_str()) <= inner_width);
    }
}
