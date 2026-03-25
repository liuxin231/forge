use serde::Deserialize;
use std::collections::HashMap;

#[allow(dead_code)]
/// Root forge.toml structure
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceConfig {
    pub workspace: WorkspaceSection,
    #[serde(default)]
    pub groups: HashMap<String, GroupConfig>,
    #[serde(default)]
    pub commands: HashMap<String, CommandConfig>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfig {
    #[serde(default)]
    pub description: Option<String>,
    /// "direct" (run at workspace root) or "service" (delegate to each service)
    #[serde(default = "default_command_mode")]
    pub mode: String,
    /// Command to run (only for mode = "direct")
    #[serde(default)]
    pub run: Option<String>,
    /// Execution order for service mode: "topological" | "parallel" | "sequential"
    #[serde(default = "default_command_order")]
    pub order: String,
    /// Stop on first failure (default true)
    #[serde(default = "default_true")]
    pub fail_fast: bool,
}

fn default_command_mode() -> String {
    "service".to_string()
}
fn default_command_order() -> String {
    "topological".to_string()
}
fn default_true() -> bool {
    true
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceSection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub zones: Option<HashMap<String, String>>,
    #[serde(default)]
    pub ignore: Option<Vec<String>>,
    #[serde(default)]
    pub ignore_override: Option<Vec<String>>,
    /// Allow parallel startup of independent services (default true)
    #[serde(default = "default_true")]
    pub parallel_startup: bool,
    /// Custom info panels shown after `fr ps` and `fr up`
    #[serde(default)]
    pub hints: Vec<HintSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HintSection {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub items: Vec<HintItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HintItem {
    pub label: String,
    pub value: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct GroupConfig {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
}
