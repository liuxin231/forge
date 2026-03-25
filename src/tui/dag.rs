use crate::config::ProjectConfig;
use crate::inspect::{RuntimeInfo, RuntimeStatus};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
enum Color {
    Green,
    DarkGray,
    White,
}

const NODE_H: usize = 4; // ┌─┐, name, info, └─┘
const V_GAP: usize = 3;  // rows between layers for edge routing
const MIN_NODE_W: usize = 14; // minimum readable width
const MIN_H_GAP: usize = 2;

/// Layout parameters computed from available width
struct LayoutParams {
    node_w: usize,
    h_gap: usize,
}

/// A positioned node in the DAG layout
#[allow(dead_code)]
struct NodeLayout {
    name: String,
    port: Option<u16>,
    status: RuntimeStatus,
    level: usize,
    col: usize,
    x: usize,
    y: usize,
}

struct Edge {
    from_x_center: usize,
    from_y_bottom: usize,
    to_x_center: usize,
    to_y_top: usize,
}

#[derive(Clone)]
struct GridCell {
    ch: char,
    fg: Color,
    is_node: bool,
}

impl Default for GridCell {
    fn default() -> Self {
        GridCell { ch: ' ', fg: Color::White, is_node: false }
    }
}

/// Compute node_w and h_gap to fit `max_cols` nodes into `avail_w` chars.
fn compute_layout_params(avail_w: usize, max_cols: usize) -> LayoutParams {
    if max_cols == 0 {
        return LayoutParams { node_w: MIN_NODE_W, h_gap: MIN_H_GAP };
    }

    // total = max_cols * node_w + (max_cols - 1) * h_gap
    // Try preferred sizes first, then shrink
    let preferred_gap = 4usize;

    // Available width for nodes after subtracting gaps
    let gap_total = if max_cols > 1 {
        (max_cols - 1) * preferred_gap
    } else {
        0
    };

    let node_w_ideal = if avail_w > gap_total {
        (avail_w - gap_total) / max_cols
    } else {
        MIN_NODE_W
    };

    let node_w = node_w_ideal.max(MIN_NODE_W).min(28); // cap at 28 to avoid overly wide boxes

    // Recompute gap with actual node_w
    let used_by_nodes = max_cols * node_w;
    let h_gap = if max_cols > 1 && avail_w > used_by_nodes {
        (avail_w - used_by_nodes) / (max_cols - 1)
    } else {
        MIN_H_GAP
    };
    let h_gap = h_gap.max(MIN_H_GAP).min(8); // cap gap

    LayoutParams { node_w, h_gap }
}

fn layout_nodes(
    _project: &ProjectConfig,
    statuses: &HashMap<String, RuntimeInfo>,
    levels: &[Vec<String>],
    params: &LayoutParams,
) -> Vec<NodeLayout> {
    let mut nodes = Vec::new();

    let max_level_width = levels
        .iter()
        .map(|l| {
            if l.is_empty() { 0 }
            else { l.len() * params.node_w + (l.len() - 1) * params.h_gap }
        })
        .max()
        .unwrap_or(0);

    for (level_idx, level) in levels.iter().enumerate() {
        let level_width = if level.is_empty() { 0 }
            else { level.len() * params.node_w + (level.len() - 1) * params.h_gap };
        let x_offset = max_level_width.saturating_sub(level_width) / 2;
        let y = level_idx * (NODE_H + V_GAP);

        for (col_idx, name) in level.iter().enumerate() {
            let x = x_offset + col_idx * (params.node_w + params.h_gap);
            let runtime = statuses.get(name);
            nodes.push(NodeLayout {
                name: name.clone(),
                port: runtime.and_then(|i| i.port),
                status: runtime
                    .map(|i| i.status.clone())
                    .unwrap_or(RuntimeStatus::Unknown),
                level: level_idx,
                col: col_idx,
                x,
                y,
            });
        }
    }

    nodes
}

fn draw_node(grid: &mut [Vec<GridCell>], node: &NodeLayout, node_w: usize) {
    let x = node.x;
    let y = node.y;
    let color = status_color(&node.status);
    let icon = status_icon(&node.status);
    let inner = node_w.saturating_sub(4); // "│ " + " │"

    // Top border
    set_cell(grid, y, x, '┌', color, true);
    for dx in 1..node_w - 1 { set_cell(grid, y, x + dx, '─', color, true); }
    set_cell(grid, y, x + node_w - 1, '┐', color, true);

    // Name line
    set_cell(grid, y + 1, x, '│', color, true);
    let name_content = format!("{} {}", icon, &node.name);
    let padded = pad_or_truncate(&name_content, inner);
    write_str(grid, y + 1, x + 2, &padded, color, true);
    set_cell(grid, y + 1, x + node_w - 1, '│', color, true);

    // Info line
    set_cell(grid, y + 2, x, '│', color, true);
    let port_str = node.port.map(|p| format!(":{}", p)).unwrap_or_default();
    let info = port_str.clone();
    let padded_info = pad_or_truncate(&info, inner);
    write_str(grid, y + 2, x + 2, &padded_info, Color::DarkGray, true);
    set_cell(grid, y + 2, x + node_w - 1, '│', color, true);

    // Bottom border
    set_cell(grid, y + 3, x, '└', color, true);
    for dx in 1..node_w - 1 { set_cell(grid, y + 3, x + dx, '─', color, true); }
    set_cell(grid, y + 3, x + node_w - 1, '┘', color, true);
}

fn draw_edge(grid: &mut [Vec<GridCell>], edge: &Edge) {
    let x1 = edge.from_x_center;
    let y1 = edge.from_y_bottom;
    let x2 = edge.to_x_center;
    let y2 = edge.to_y_top;

    if y2 <= y1 + 1 { return; }

    let color = Color::DarkGray;
    let mid_y = y1 + (y2 - y1) / 2;

    if x1 == x2 {
        for y in (y1 + 1)..y2 { set_edge_cell(grid, y, x1, '│', color); }
        set_edge_cell(grid, y2 - 1, x2, '▼', color);
    } else {
        for y in (y1 + 1)..mid_y { set_edge_cell(grid, y, x1, '│', color); }

        if x1 < x2 {
            set_edge_cell(grid, mid_y, x1, '└', color);
            for x in (x1 + 1)..x2 { set_edge_cell(grid, mid_y, x, '─', color); }
            set_edge_cell(grid, mid_y, x2, '┐', color);
        } else {
            set_edge_cell(grid, mid_y, x1, '┘', color);
            for x in (x2 + 1)..x1 { set_edge_cell(grid, mid_y, x, '─', color); }
            set_edge_cell(grid, mid_y, x2, '┌', color);
        }

        for y in (mid_y + 1)..y2 { set_edge_cell(grid, y, x2, '│', color); }
        if y2 > mid_y + 1 { set_edge_cell(grid, y2 - 1, x2, '▼', color); }
    }
}

fn set_cell(grid: &mut [Vec<GridCell>], row: usize, col: usize, ch: char, fg: Color, is_node: bool) {
    if row < grid.len() && col < grid[0].len() {
        grid[row][col] = GridCell { ch, fg, is_node };
    }
}

fn set_edge_cell(grid: &mut [Vec<GridCell>], row: usize, col: usize, ch: char, fg: Color) {
    if row < grid.len() && col < grid[0].len() {
        let cell = &grid[row][col];
        if !cell.is_node {
            let new_ch = match (cell.ch, ch) {
                (' ', c) => c,
                ('│', '─') | ('─', '│') => '┼',
                ('│', '└') | ('│', '┘') => '├',
                ('│', '┐') | ('│', '┌') => '┤',
                ('─', '┐') | ('─', '┌') => '┬',
                ('─', '└') | ('─', '┘') => '┴',
                ('│', '│') => '│',
                ('─', '─') => '─',
                ('│', '▼') => '▼',
                (_, c) => c,
            };
            grid[row][col] = GridCell { ch: new_ch, fg, is_node: false };
        }
    }
}

fn write_str(grid: &mut [Vec<GridCell>], row: usize, col: usize, s: &str, fg: Color, is_node: bool) {
    for (i, ch) in s.chars().enumerate() {
        set_cell(grid, row, col + i, ch, fg, is_node);
    }
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.chars().take(width).collect()
    } else {
        format!("{}{}", s, " ".repeat(width - len))
    }
}

fn status_color(status: &RuntimeStatus) -> Color {
    match status {
        RuntimeStatus::Running => Color::Green,
        RuntimeStatus::Stopped => Color::White,
        RuntimeStatus::Unknown => Color::DarkGray,
    }
}

fn status_icon(status: &RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Running => "●",
        RuntimeStatus::Stopped => "○",
        RuntimeStatus::Unknown => "?",
    }
}

/// Render the DAG as a plain string with ANSI color codes, for use outside TUI.
pub fn render_dag_ansi(
    project: &ProjectConfig,
    statuses: &HashMap<String, RuntimeInfo>,
    levels: &[Vec<String>],
    avail_w: usize,
) -> String {
    if levels.is_empty() {
        return "  No services found.\n".to_string();
    }

    let max_cols = levels.iter().map(|l| l.len()).max().unwrap_or(1);
    let params = compute_layout_params(avail_w, max_cols);
    let nodes = layout_nodes(project, statuses, levels, &params);

    let grid_w = max_cols * (params.node_w + params.h_gap);
    let grid_h = levels.len() * NODE_H + levels.len().saturating_sub(1) * V_GAP;
    let mut grid = vec![vec![GridCell::default(); grid_w + 1]; grid_h + 1];

    for node in &nodes {
        draw_node(&mut grid, node, params.node_w);
    }

    let node_map: HashMap<&str, &NodeLayout> =
        nodes.iter().map(|n| (n.name.as_str(), n)).collect();
    let mut edges = Vec::new();
    for node in &nodes {
        if let Some(svc) = project.services.get(&node.name) {
            for dep in &svc.config.depends_on {
                if let Some(parent) = node_map.get(dep.as_str()) {
                    edges.push(Edge {
                        from_x_center: parent.x + params.node_w / 2,
                        from_y_bottom: parent.y + NODE_H - 1,
                        to_x_center: node.x + params.node_w / 2,
                        to_y_top: node.y,
                    });
                }
            }
        }
    }
    for edge in &edges {
        draw_edge(&mut grid, edge);
    }

    grid_to_ansi_string(&grid)
}

fn ansi_color(color: Color) -> &'static str {
    match color {
        Color::Green => "\x1b[32m",
        Color::DarkGray => "\x1b[90m",
        _ => "\x1b[0m",
    }
}

fn grid_to_ansi_string(grid: &[Vec<GridCell>]) -> String {
    let reset = "\x1b[0m";
    let mut out = String::new();
    for row in grid {
        let last = row.iter().rposition(|c| c.ch != ' ').map(|i| i + 1).unwrap_or(0);
        let mut cur_color: Option<Color> = None;
        for cell in &row[..last] {
            if cell.ch == ' ' {
                if cur_color.is_some() {
                    out.push_str(reset);
                    cur_color = None;
                }
                out.push(' ');
            } else {
                if cur_color != Some(cell.fg) {
                    out.push_str(ansi_color(cell.fg));
                    cur_color = Some(cell.fg);
                }
                out.push(cell.ch);
            }
        }
        if cur_color.is_some() {
            out.push_str(reset);
        }
        out.push('\n');
    }
    out
}
