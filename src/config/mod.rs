mod scanner;
pub mod service;
pub mod validate;
pub mod workspace;

pub use scanner::load_project;
pub use service::{ServiceConfig, ServiceMode};
pub use workspace::WorkspaceConfig;

use std::collections::HashMap;
use std::path::PathBuf;

#[allow(dead_code)]
/// Complete project configuration: workspace config + all discovered services
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub workspace: WorkspaceConfig,
    pub services: HashMap<String, ResolvedService>,
    pub root: PathBuf,
}

/// A fully resolved service with its config and location
#[derive(Debug, Clone)]
pub struct ResolvedService {
    pub name: String,
    pub config: ServiceConfig,
    pub dir: PathBuf,
}
