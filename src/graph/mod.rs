use crate::config::ProjectConfig;
use anyhow::{bail, Result};
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet, VecDeque};

/// Dependency graph for services
#[derive(Debug)]
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    node_map: HashMap<String, NodeIndex>,
}

impl DependencyGraph {
    /// Build dependency graph from project config
    pub fn build(project: &ProjectConfig) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut node_map = HashMap::new();

        for name in project.services.keys() {
            let idx = graph.add_node(name.clone());
            node_map.insert(name.clone(), idx);
        }

        for (name, svc) in &project.services {
            let dependent_idx = node_map[name];
            for dep in &svc.config.depends_on {
                let dep_idx = node_map.get(dep).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Service '{}' depends on '{}', but '{}' is not defined",
                        name, dep, dep
                    )
                })?;
                graph.add_edge(*dep_idx, dependent_idx, ());
            }
        }

        if toposort(&graph, None).is_err() {
            // Find the cycle participants for a useful error message
            let cycle_info = find_cycle_members(&graph);
            bail!(
                "Circular dependency detected in service graph: {}",
                cycle_info
            );
        }

        Ok(Self { graph, node_map })
    }

    /// Get topological start order as a flat list
    pub fn topological_order_for(&self, targets: &[String]) -> Result<Vec<String>> {
        let levels = self.topological_levels_for(targets)?;
        Ok(levels.into_iter().flatten().collect())
    }

    /// Get topological start order grouped by level.
    /// Services within the same level have no dependencies on each other and can start in parallel.
    pub fn topological_levels_for(&self, targets: &[String]) -> Result<Vec<Vec<String>>> {
        let needed = self.expand_dependencies(targets)?;

        // Build subgraph in-degree map (only for needed services)
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for name in &needed {
            in_degree.insert(name.clone(), 0);
        }
        for name in &needed {
            let idx = self.node_map[name];
            for dep_idx in self
                .graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
            {
                let dep_name = &self.graph[dep_idx];
                if needed.contains(dep_name) {
                    *in_degree.get_mut(name).unwrap() += 1;
                }
            }
        }

        // Kahn's algorithm with level tracking
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(name, _)| name.clone())
            .collect();

        // Sort initial queue for deterministic order
        let mut sorted_queue: Vec<String> = queue.drain(..).collect();
        sorted_queue.sort();
        queue.extend(sorted_queue);

        let mut levels: Vec<Vec<String>> = Vec::new();

        while !queue.is_empty() {
            // All services in the current queue form one level
            let mut level: Vec<String> = queue.drain(..).collect();
            level.sort(); // deterministic order within level

            let mut next_queue = Vec::new();
            for name in &level {
                let idx = self.node_map[name];
                // Reduce in-degree of dependents
                for dependent_idx in self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                {
                    let dep_name = &self.graph[dependent_idx];
                    if let Some(deg) = in_degree.get_mut(dep_name) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            next_queue.push(dep_name.clone());
                        }
                    }
                }
            }

            levels.push(level);
            next_queue.sort();
            queue.extend(next_queue);
        }

        Ok(levels)
    }

    /// Get reverse topological order (for stopping)
    #[allow(dead_code)]
    pub fn reverse_topological_order_for(&self, targets: &[String]) -> Result<Vec<String>> {
        let mut order = self.topological_order_for(targets)?;
        order.reverse();
        Ok(order)
    }

    /// Expand targets to include all transitive dependencies
    fn expand_dependencies(&self, targets: &[String]) -> Result<HashSet<String>> {
        let mut needed = HashSet::new();
        let mut stack: Vec<String> = targets.to_vec();

        while let Some(name) = stack.pop() {
            if needed.contains(&name) {
                continue;
            }
            let idx = self.node_map.get(&name).ok_or_else(|| {
                anyhow::anyhow!("Service '{}' not found in dependency graph", name)
            })?;
            needed.insert(name.clone());

            for neighbor in self
                .graph
                .neighbors_directed(*idx, petgraph::Direction::Incoming)
            {
                let dep_name = &self.graph[neighbor];
                if !needed.contains(dep_name) {
                    stack.push(dep_name.clone());
                }
            }
        }

        Ok(needed)
    }
}

/// Find services involved in cycles for error reporting
fn find_cycle_members(graph: &DiGraph<String, ()>) -> String {
    use petgraph::algo::kosaraju_scc;

    let sccs = kosaraju_scc(graph);
    let cycle_components: Vec<Vec<String>> = sccs
        .into_iter()
        .filter(|scc| scc.len() > 1 || {
            // Single node with self-edge
            let idx = scc[0];
            graph.contains_edge(idx, idx)
        })
        .map(|scc| {
            let mut names: Vec<String> = scc.iter().map(|idx| graph[*idx].clone()).collect();
            names.sort();
            names
        })
        .collect();

    if cycle_components.is_empty() {
        "unknown cycle".to_string()
    } else {
        cycle_components
            .iter()
            .map(|c| c.join(" -> "))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        service::ServiceConfig, workspace::{WorkspaceConfig, WorkspaceSection},
        ProjectConfig, ResolvedService,
    };
    use std::path::PathBuf;

    fn make_svc(name: &str, deps: Vec<&str>) -> ResolvedService {
        ResolvedService {
            name: name.to_string(),
            config: ServiceConfig {
                port: None,
                groups: vec![],
                depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
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
                commands: HashMap::new(),
            },
            dir: PathBuf::from("/tmp"),
        }
    }

    fn make_project(svcs: Vec<(&str, Vec<&str>)>) -> ProjectConfig {
        let services: HashMap<String, ResolvedService> = svcs
            .into_iter()
            .map(|(name, deps)| (name.to_string(), make_svc(name, deps)))
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
    fn test_simple_graph() {
        let project = make_project(vec![
            ("api", vec!["db"]),
            ("db", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let order = graph.topological_order_for(&["api".to_string()]).unwrap();
        assert_eq!(order, vec!["db", "api"]);
    }

    #[test]
    fn test_diamond_dependency() {
        // db -> api, db -> worker, api -> gateway, worker -> gateway
        let project = make_project(vec![
            ("gateway", vec!["api", "worker"]),
            ("api", vec!["db"]),
            ("worker", vec!["db"]),
            ("db", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let levels = graph.topological_levels_for(&["gateway".to_string()]).unwrap();
        assert_eq!(levels[0], vec!["db"]);
        assert_eq!(levels[1], vec!["api", "worker"]);
        assert_eq!(levels[2], vec!["gateway"]);
    }

    #[test]
    fn test_circular_dependency() {
        let project = make_project(vec![
            ("a", vec!["b"]),
            ("b", vec!["a"]),
        ]);
        let result = DependencyGraph::build(&project);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Circular dependency"));
        assert!(err_msg.contains("a"));
        assert!(err_msg.contains("b"));
    }

    #[test]
    fn test_self_dependency() {
        let project = make_project(vec![
            ("a", vec!["a"]),
        ]);
        let result = DependencyGraph::build(&project);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Circular dependency"));
    }

    #[test]
    fn test_independent_services() {
        let project = make_project(vec![
            ("a", vec![]),
            ("b", vec![]),
            ("c", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let levels = graph.topological_levels_for(&[
            "a".to_string(), "b".to_string(), "c".to_string()
        ]).unwrap();
        // All in one level since no dependencies
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn test_empty_targets() {
        let project = make_project(vec![
            ("a", vec![]),
            ("b", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let levels = graph.topological_levels_for(&[]).unwrap();
        assert!(levels.is_empty());
    }

    #[test]
    fn test_transitive_dependencies() {
        let project = make_project(vec![
            ("c", vec!["b"]),
            ("b", vec!["a"]),
            ("a", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let order = graph.topological_order_for(&["c".to_string()]).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_nonexistent_target() {
        let project = make_project(vec![
            ("a", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let result = graph.topological_order_for(&["nonexistent".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_reverse_order() {
        let project = make_project(vec![
            ("api", vec!["db"]),
            ("db", vec![]),
        ]);
        let graph = DependencyGraph::build(&project).unwrap();
        let order = graph.reverse_topological_order_for(&["api".to_string()]).unwrap();
        assert_eq!(order, vec!["api", "db"]);
    }
}
