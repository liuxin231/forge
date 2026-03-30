use crate::config::ProjectConfig;
use anyhow::{bail, Result};

/// Resolve user-specified targets into concrete service names
///
/// Resolution rules:
/// - "iam" → domain match: all services under iam/ (e.g., iam/api, iam/web)
/// - "iam/api" → exact match
/// - "postgres" → exact match (infra services)
/// - [] (empty) → all services
pub fn resolve_targets(project: &ProjectConfig, targets: &[String]) -> Result<Vec<String>> {
    if targets.is_empty() {
        // Return all services
        let mut result: Vec<String> = project
            .services
            .keys()
            .cloned()
            .collect();
        result.sort();
        return Ok(result);
    }

    let mut resolved = Vec::new();

    for target in targets {
        let matches = resolve_single_target(project, target)?;
        if matches.is_empty() {
            let mut available: Vec<String> = project.services.keys().cloned().collect();
            available.sort();
            bail!(
                "No service found matching '{}'. Available services: {}",
                target,
                available.join(", ")
            );
        }
        for m in matches {
            if !resolved.contains(&m) {
                resolved.push(m);
            }
        }
    }

    Ok(resolved)
}

fn resolve_single_target(project: &ProjectConfig, target: &str) -> Result<Vec<String>> {
    // 1. Exact match
    if project.services.contains_key(target) {
        return Ok(vec![target.to_string()]);
    }

    // 2. Domain-level match: "iam" → "iam/api", "iam/web"
    let domain_prefix = format!("{}/", target);
    let mut domain_matches: Vec<String> = project
        .services
        .keys()
        .filter(|name| name.starts_with(&domain_prefix))
        .cloned()
        .collect();

    if !domain_matches.is_empty() {
        domain_matches.sort();
        return Ok(domain_matches);
    }

    // 3. No match found
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        service::{ServiceConfig, ServiceMode}, workspace::{WorkspaceConfig, WorkspaceSection},
        ProjectConfig, ResolvedService,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_svc(name: &str) -> ResolvedService {
        ResolvedService {
            name: name.to_string(),
            config: ServiceConfig {
                port: None,
                groups: vec![],
                depends_on: vec![],
                health: None,
                env: HashMap::new(),
                env_file: None,
                up: Some("echo".to_string()),
                down: None,
                build: None,
                dev: None,
                logs: None,
                cwd: None,
                args: None,
                autorestart: true,
                max_restarts: 10,
                restart_delay: 3,
                kill_timeout: 10,
                treekill: true,
                attach: false,
                max_memory: None,
                mode: ServiceMode::Service,
                commands: HashMap::new(),
            },
            dir: PathBuf::from("/tmp"),
        }
    }

    fn make_project(svcs: Vec<&str>) -> ProjectConfig {
        let services: HashMap<String, ResolvedService> = svcs
            .into_iter()
            .map(|name| (name.to_string(), make_svc(name)))
            .collect();
        ProjectConfig {
            workspace: WorkspaceConfig {
                workspace: WorkspaceSection {
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
            },
            services,
            root: PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn test_empty_targets_returns_all_services() {
        let project = make_project(vec!["api", "web"]);
        let result = resolve_targets(&project, &[]).unwrap();
        assert_eq!(result, vec!["api", "web"]);
    }

    #[test]
    fn test_exact_match() {
        let project = make_project(vec!["api", "web"]);
        let result = resolve_targets(&project, &["api".to_string()]).unwrap();
        assert_eq!(result, vec!["api"]);
    }

    #[test]
    fn test_domain_match() {
        let project = make_project(vec!["iam/api", "iam/web", "gateway/api"]);
        let result = resolve_targets(&project, &["iam".to_string()]).unwrap();
        assert_eq!(result, vec!["iam/api", "iam/web"]);
    }

    #[test]
    fn test_nonexistent_target() {
        let project = make_project(vec!["api"]);
        let result = resolve_targets(&project, &["nonexistent".to_string()]);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No service found matching 'nonexistent'"));
        assert!(err_msg.contains("api"));
    }

    #[test]
    fn test_duplicate_targets_deduped() {
        let project = make_project(vec!["api"]);
        let result = resolve_targets(&project, &["api".to_string(), "api".to_string()]).unwrap();
        assert_eq!(result, vec!["api"]);
    }

    #[test]
    fn test_exact_match_takes_priority_over_domain() {
        let project = make_project(vec!["iam", "iam/api", "iam/web"]);
        let result = resolve_targets(&project, &["iam".to_string()]).unwrap();
        assert_eq!(result, vec!["iam"]);
    }

    #[test]
    fn test_error_message_sorted() {
        let project = make_project(vec!["z-svc", "a-svc", "m-svc"]);
        let result = resolve_targets(&project, &["nonexistent".to_string()]);
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("a-svc, m-svc, z-svc"));
    }
}
