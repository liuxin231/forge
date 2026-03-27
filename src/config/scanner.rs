use super::service::{ServiceConfigOrMulti, ServiceFile};
use super::validate;
use super::{ProjectConfig, ResolvedService};
use super::workspace::WorkspaceConfig;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DEFAULT_IGNORE: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    ".git",
    ".next",
    ".nuxt",
    ".output",
    "__pycache__",
    "vendor",
    ".turbo",
    ".nx",
    ".forge",
];

/// Maximum recursion depth for directory scanning
const MAX_SCAN_DEPTH: usize = 20;

/// Load complete project configuration from workspace root
pub fn load_project(root: &Path) -> Result<ProjectConfig> {
    let root_toml = root.join("forge.toml");
    let content = std::fs::read_to_string(&root_toml)
        .with_context(|| format!("Failed to read {}", root_toml.display()))?;
    let workspace: WorkspaceConfig =
        toml::from_str(&content).with_context(|| "Failed to parse root forge.toml")?;

    // Build ignore patterns
    let ignore_patterns = build_ignore_patterns(&workspace);

    // Determine scan directories
    let scan_dirs = get_scan_dirs(root, &workspace);

    // Warn about non-existent zone directories
    for (zone_name, zone_dir) in &scan_dirs {
        if !zone_name.is_empty() && !zone_dir.is_dir() {
            tracing::warn!(
                "Zone '{}' directory does not exist: {}",
                zone_name,
                zone_dir.display()
            );
        }
    }

    // Scan for service forge.toml files
    let mut services = HashMap::new();
    for (zone_name, zone_dir) in &scan_dirs {
        scan_directory(zone_dir, zone_dir, zone_name, &ignore_patterns, &mut services, 0)?;
    }

    // Resolve environment variables: workspace env → env_file → service env
    resolve_service_env(&workspace, &mut services);

    let project = ProjectConfig {
        workspace,
        services,
        root: root.to_path_buf(),
    };

    // Validate the loaded configuration
    validate::validate(&project)?;

    Ok(project)
}

fn build_ignore_patterns(workspace: &WorkspaceConfig) -> Vec<String> {
    if let Some(override_patterns) = &workspace.workspace.ignore_override {
        return override_patterns.clone();
    }

    let mut patterns: Vec<String> = DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect();
    if let Some(extra) = &workspace.workspace.ignore {
        patterns.extend(extra.iter().cloned());
    }
    patterns
}

/// Resolve environment variables for all services:
/// 1. Start with workspace-level env (lowest priority)
/// 2. Load env_file if specified
/// 3. Service-level env overrides everything
fn resolve_service_env(workspace: &WorkspaceConfig, services: &mut HashMap<String, ResolvedService>) {
    let workspace_env = &workspace.workspace.env;

    for (_name, svc) in services.iter_mut() {
        let mut merged = HashMap::new();

        // 1. Workspace env (lowest priority)
        for (k, v) in workspace_env {
            merged.insert(k.clone(), v.clone());
        }

        // 2. env_file (middle priority)
        if let Some(env_file_path) = &svc.config.env_file {
            let full_path = if Path::new(env_file_path).is_absolute() {
                PathBuf::from(env_file_path)
            } else {
                svc.dir.join(env_file_path)
            };
            if let Ok(content) = std::fs::read_to_string(&full_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((key, value)) = line.split_once('=') {
                        let key = key.trim().to_string();
                        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
                        merged.insert(key, value);
                    }
                }
            } else {
                tracing::warn!(
                    "env_file '{}' not found for service '{}'",
                    full_path.display(),
                    _name
                );
            }
        }

        // 3. Service-level env (highest priority)
        for (k, v) in &svc.config.env {
            merged.insert(k.clone(), v.clone());
        }

        svc.config.env = merged;
    }
}

fn get_scan_dirs(root: &Path, workspace: &WorkspaceConfig) -> Vec<(String, PathBuf)> {
    match &workspace.workspace.zones {
        Some(zones) => zones
            .iter()
            .map(|(name, path)| (name.clone(), root.join(path)))
            .collect(),
        None => vec![("".to_string(), root.to_path_buf())],
    }
}

fn scan_directory(
    dir: &Path,
    zone_root: &Path,
    zone_name: &str,
    ignore_patterns: &[String],
    services: &mut HashMap<String, ResolvedService>,
    depth: usize,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    if depth > MAX_SCAN_DEPTH {
        tracing::warn!(
            "Maximum scan depth ({}) exceeded at {}, skipping",
            MAX_SCAN_DEPTH,
            dir.display()
        );
        return Ok(());
    }

    // Check if this directory should be ignored
    if let Some(dir_name) = dir.file_name().and_then(|n| n.to_str())
        && ignore_patterns.iter().any(|p| {
            if p.contains('*') {
                glob::Pattern::new(p)
                    .map(|pat| pat.matches(dir_name))
                    .unwrap_or(false)
            } else {
                p == dir_name
            }
        }) {
            return Ok(());
        }

    let forge_toml = dir.join("forge.toml");
    // Use is_file() instead of exists() to handle symlinks and directories named forge.toml
    if forge_toml.is_file() && !is_workspace_root(dir, zone_root) {
        // Check if this is a workspace config (not a service config)
        let content = std::fs::read_to_string(&forge_toml)
            .with_context(|| format!("Failed to read {}", forge_toml.display()))?;

        // Skip if this is a workspace forge.toml — but still recurse into children
        if is_likely_workspace_config(&content) {
            // Fall through to recursive scanning below
        } else {
            let file: ServiceFile = toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", forge_toml.display()))?;

            // Skip lib files — they don't define services
            if file.lib.is_none() {
                match file.parse_services() {
                    Some(svc) => match svc {
                        ServiceConfigOrMulti::Single(config) => {
                            let name = compute_service_name(dir, zone_root, zone_name);
                            if !name.is_empty() {
                                if services.contains_key(&name) {
                                    anyhow::bail!(
                                        "Duplicate service name '{}': found in {} and {}",
                                        name,
                                        services[&name].dir.display(),
                                        dir.display()
                                    );
                                }
                                services.insert(
                                    name.clone(),
                                    ResolvedService {
                                        name,
                                        config: *config,
                                        dir: dir.to_path_buf(),
                                    },
                                );
                            }
                            // Don't recurse into service directories
                            return Ok(());
                        }
                        ServiceConfigOrMulti::Multi(map) => {
                            for (svc_name, config) in map {
                                if services.contains_key(&svc_name) {
                                    anyhow::bail!(
                                        "Duplicate service name '{}': found in {} and {}",
                                        svc_name,
                                        services[&svc_name].dir.display(),
                                        dir.display()
                                    );
                                }
                                services.insert(
                                    svc_name.clone(),
                                    ResolvedService {
                                        name: svc_name,
                                        config,
                                        dir: dir.to_path_buf(),
                                    },
                                );
                            }
                            // Don't recurse further
                            return Ok(());
                        }
                    },
                    None => {
                        tracing::warn!(
                            "forge.toml at {} has a [service] section but could not be parsed as a valid service config",
                            forge_toml.display()
                        );
                    }
                }
            }
        }
    }

    // Recurse into subdirectories, handling permission errors gracefully
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("Cannot read directory {}: {}, skipping", dir.display(), e);
            return Ok(());
        }
    };

    let mut sorted_entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    sorted_entries.sort_by_key(|e| e.file_name());

    for entry in sorted_entries {
        scan_directory(
            &entry.path(),
            zone_root,
            zone_name,
            ignore_patterns,
            services,
            depth + 1,
        )?;
    }

    Ok(())
}

/// Check if this directory is the workspace root (parent of zone_root)
fn is_workspace_root(dir: &Path, zone_root: &Path) -> bool {
    if let Some(parent) = zone_root.parent() {
        dir == parent
    } else {
        // zone_root is filesystem root, treat as not workspace root
        false
    }
}

/// Heuristic check: does this content look like a workspace config?
/// We check if it has a proper [workspace] TOML section, not just in a comment or string.
fn is_likely_workspace_config(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[workspace]" || trimmed.starts_with("[workspace.") {
            return true;
        }
    }
    false
}

/// Compute service name from directory path relative to zone root
fn compute_service_name(dir: &Path, zone_root: &Path, _zone_name: &str) -> String {
    let relative = dir.strip_prefix(zone_root).unwrap_or(dir);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ignore_patterns_defaults() {
        let ws = WorkspaceConfig {
            workspace: super::super::workspace::WorkspaceSection {
                name: "test".to_string(),
                description: None,
                zones: None,
                ignore: None,
                ignore_override: None,
                parallel_startup: true,
                hints: vec![],
                env: HashMap::new(),
            },
            groups: HashMap::new(),
            commands: HashMap::new(),
        };
        let patterns = build_ignore_patterns(&ws);
        assert!(patterns.contains(&"node_modules".to_string()));
        assert!(patterns.contains(&".git".to_string()));
        assert!(patterns.contains(&"target".to_string()));
    }

    #[test]
    fn test_build_ignore_patterns_with_extra() {
        let ws = WorkspaceConfig {
            workspace: super::super::workspace::WorkspaceSection {
                name: "test".to_string(),
                description: None,
                zones: None,
                ignore: Some(vec!["tmp".to_string()]),
                ignore_override: None,
                parallel_startup: true,
                hints: vec![],
                env: HashMap::new(),
            },
            groups: HashMap::new(),
            commands: HashMap::new(),
        };
        let patterns = build_ignore_patterns(&ws);
        assert!(patterns.contains(&"tmp".to_string()));
        assert!(patterns.contains(&"node_modules".to_string()));
    }

    #[test]
    fn test_build_ignore_patterns_override() {
        let ws = WorkspaceConfig {
            workspace: super::super::workspace::WorkspaceSection {
                name: "test".to_string(),
                description: None,
                zones: None,
                ignore: None,
                ignore_override: Some(vec!["only_this".to_string()]),
                parallel_startup: true,
                hints: vec![],
                env: HashMap::new(),
            },
            groups: HashMap::new(),
            commands: HashMap::new(),
        };
        let patterns = build_ignore_patterns(&ws);
        assert_eq!(patterns, vec!["only_this"]);
        assert!(!patterns.contains(&"node_modules".to_string()));
    }

    #[test]
    fn test_compute_service_name() {
        let zone_root = Path::new("/project/apps");
        let dir = Path::new("/project/apps/iam/api");
        assert_eq!(compute_service_name(dir, zone_root, "apps"), "iam/api");
    }

    #[test]
    fn test_compute_service_name_single_level() {
        let zone_root = Path::new("/project/apps");
        let dir = Path::new("/project/apps/gateway");
        assert_eq!(compute_service_name(dir, zone_root, "apps"), "gateway");
    }

    #[test]
    fn test_is_likely_workspace_config() {
        assert!(is_likely_workspace_config("[workspace]\nname = \"test\""));
        assert!(is_likely_workspace_config("  [workspace]  \nname = \"test\""));
        assert!(is_likely_workspace_config("[workspace.zones]\napps = \"apps\""));
        assert!(!is_likely_workspace_config("# [workspace]\n[service]"));
        assert!(!is_likely_workspace_config("name = \"[workspace]\""));
    }

    #[test]
    fn test_is_workspace_root() {
        assert!(is_workspace_root(
            Path::new("/project"),
            Path::new("/project/apps")
        ));
        assert!(!is_workspace_root(
            Path::new("/project/apps"),
            Path::new("/project/apps")
        ));
        assert!(!is_workspace_root(
            Path::new("/project/apps/iam"),
            Path::new("/project/apps")
        ));
    }

    #[test]
    fn test_load_project_from_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create root forge.toml
        std::fs::write(
            root.join("forge.toml"),
            r#"
[workspace]
name = "test-project"
"#,
        )
        .unwrap();

        // Create a service directory
        let svc_dir = root.join("api");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(
            svc_dir.join("forge.toml"),
            r#"
[service]
type = "command"
up = "echo hello"
port = 8080
"#,
        )
        .unwrap();

        let project = load_project(root).unwrap();
        assert_eq!(project.workspace.workspace.name, "test-project");
        assert!(project.services.contains_key("api"));
        assert_eq!(project.services["api"].config.port, Some(8080));
    }

    #[test]
    fn test_load_project_with_zones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::write(
            root.join("forge.toml"),
            r#"
[workspace]
name = "test"

[workspace.zones]
apps = "apps"
infra = "infra"
"#,
        )
        .unwrap();

        // apps/gateway/forge.toml
        std::fs::create_dir_all(root.join("apps/gateway")).unwrap();
        std::fs::write(
            root.join("apps/gateway/forge.toml"),
            r#"
[service]
type = "command"
up = "echo gw"
port = 8000
"#,
        )
        .unwrap();

        // infra/forge.toml with multi-service
        std::fs::create_dir_all(root.join("infra")).unwrap();
        std::fs::write(
            root.join("infra/forge.toml"),
            r#"
[service.postgres]
type = "command"
up = "docker compose up postgres"
port = 5432

[service.redis]
type = "command"
up = "docker compose up redis"
port = 6379
"#,
        )
        .unwrap();

        let project = load_project(root).unwrap();
        assert!(project.services.contains_key("gateway"));
        assert!(project.services.contains_key("postgres"));
        assert!(project.services.contains_key("redis"));
    }

    #[test]
    fn test_duplicate_service_name_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::write(
            root.join("forge.toml"),
            r#"
[workspace]
name = "test"

[workspace.zones]
a = "zone_a"
b = "zone_b"
"#,
        )
        .unwrap();

        // Both zones have "svc" directory
        std::fs::create_dir_all(root.join("zone_a/svc")).unwrap();
        std::fs::write(
            root.join("zone_a/svc/forge.toml"),
            "[service]\ntype = \"command\"\nup = \"echo a\"\nport = 8001",
        )
        .unwrap();

        std::fs::create_dir_all(root.join("zone_b/svc")).unwrap();
        std::fs::write(
            root.join("zone_b/svc/forge.toml"),
            "[service]\ntype = \"command\"\nup = \"echo b\"\nport = 8002",
        )
        .unwrap();

        let result = load_project(root);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate service name"));
    }

    #[test]
    fn test_ignored_directories_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::write(
            root.join("forge.toml"),
            "[workspace]\nname = \"test\"",
        )
        .unwrap();

        // Create service in node_modules (should be ignored)
        std::fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        std::fs::write(
            root.join("node_modules/foo/forge.toml"),
            "[service]\ntype = \"command\"\nup = \"echo\"\nport = 9999",
        )
        .unwrap();

        let project = load_project(root).unwrap();
        assert!(project.services.is_empty());
    }

    #[test]
    fn test_nonexistent_zone_dir_does_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::write(
            root.join("forge.toml"),
            r#"
[workspace]
name = "test"

[workspace.zones]
apps = "nonexistent"
"#,
        )
        .unwrap();

        // Should not error, just return no services
        let project = load_project(root).unwrap();
        assert!(project.services.is_empty());
    }
}
