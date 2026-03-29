use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Cache entry stored on disk
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CacheEntry {
    pub hash: String,
}

/// Root cache directory: {workspace_root}/.forge/cache/
pub fn cache_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".forge").join("cache")
}

/// Compute SHA256 over all files matched by `inputs` glob patterns relative to `service_dir`.
/// Returns None if inputs is empty (caching disabled).
pub fn compute_inputs_hash(service_dir: &Path, inputs: &[String]) -> Result<Option<String>> {
    if inputs.is_empty() {
        return Ok(None);
    }

    let mut matched: Vec<PathBuf> = Vec::new();
    for pattern in inputs {
        let full = service_dir.join(pattern);
        let pattern_str = full.to_string_lossy();
        for entry in glob::glob(&pattern_str)? {
            let path = entry?;
            if path.is_file() {
                matched.push(path);
            }
        }
    }

    // Deterministic ordering
    matched.sort();

    let mut hasher = Sha256::new();
    if matched.is_empty() {
        // No files matched — hash the patterns themselves so a change in pattern invalidates cache
        hasher.update(inputs.join("\n").as_bytes());
    } else {
        for path in &matched {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            let contents = std::fs::read(path)?;
            hasher.update(&contents);
        }
    }

    Ok(Some(format!("{:x}", hasher.finalize())))
}

fn entry_path(cache_root: &Path, service: &str, command: &str) -> PathBuf {
    let sanitized = service.replace('/', "-").replace(['\\', ':'], "-");
    cache_root.join(sanitized).join(format!("{}.json", command))
}

pub fn read_cache(cache_root: &Path, service: &str, command: &str) -> Option<CacheEntry> {
    let path = entry_path(cache_root, service, command);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_cache(cache_root: &Path, service: &str, command: &str, hash: &str) -> Result<()> {
    let path = entry_path(cache_root, service, command);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let entry = CacheEntry {
        hash: hash.to_string(),
    };
    std::fs::write(path, serde_json::to_string(&entry)?)?;
    Ok(())
}

/// Returns Some(hash) if inputs are defined and the hash differs from cached; None if cache hit.
/// Returns Err on I/O failure.
pub fn check_cache(
    cache_root: &Path,
    service_dir: &Path,
    service: &str,
    command: &str,
    inputs: &[String],
) -> Result<CacheCheckResult> {
    let hash = match compute_inputs_hash(service_dir, inputs)? {
        None => return Ok(CacheCheckResult::Disabled),
        Some(h) => h,
    };

    match read_cache(cache_root, service, command) {
        Some(entry) if entry.hash == hash => Ok(CacheCheckResult::Hit),
        _ => Ok(CacheCheckResult::Miss { hash }),
    }
}

pub enum CacheCheckResult {
    /// inputs not defined — caching disabled
    Disabled,
    /// hash matches cached entry
    Hit,
    /// hash differs (or no cached entry)
    Miss { hash: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_hash_empty_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let result = compute_inputs_hash(dir.path(), &[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_hash_no_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        let result = compute_inputs_hash(dir.path(), &["*.rs".to_string()]).unwrap();
        // Returns Some (patterns hashed)
        assert!(result.is_some());
    }

    #[test]
    fn test_hash_file_change_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, b"fn main() {}").unwrap();

        let h1 = compute_inputs_hash(dir.path(), &["*.rs".to_string()])
            .unwrap()
            .unwrap();

        std::fs::write(&file, b"fn main() { println!(\"hi\"); }").unwrap();

        let h2 = compute_inputs_hash(dir.path(), &["*.rs".to_string()])
            .unwrap()
            .unwrap();

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_same_content_stable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), b"foo").unwrap();

        let h1 = compute_inputs_hash(dir.path(), &["*.rs".to_string()])
            .unwrap()
            .unwrap();
        let h2 = compute_inputs_hash(dir.path(), &["*.rs".to_string()])
            .unwrap()
            .unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_cache_write_read_hit() {
        let dir = tempfile::tempdir().unwrap();
        let svc_dir = dir.path().join("svc");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(svc_dir.join("main.rs"), b"fn main(){}").unwrap();

        let cache = dir.path().join("cache");
        let inputs = vec!["*.rs".to_string()];

        // First check: miss
        match check_cache(&cache, &svc_dir, "api", "build", &inputs).unwrap() {
            CacheCheckResult::Miss { hash } => {
                write_cache(&cache, "api", "build", &hash).unwrap();
            }
            _ => panic!("expected miss"),
        }

        // Second check: hit
        match check_cache(&cache, &svc_dir, "api", "build", &inputs).unwrap() {
            CacheCheckResult::Hit => {}
            _ => panic!("expected hit"),
        }
    }
}
