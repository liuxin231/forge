use anyhow::{bail, Result};
use colored::Colorize;

const GITHUB_API: &str = "https://api.github.com/repos/liuxin231/forge/releases/latest";
const GITHUB_RELEASES: &str = "https://github.com/liuxin231/forge/releases";

/// Current binary version from Cargo.toml
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(check_only: bool, allow_unsigned: bool) -> Result<()> {
    let current = format!("v{}", CURRENT_VERSION);
    eprintln!("Current version: {}", current.bold());

    eprintln!("Checking for updates...");

    let release = fetch_latest_release().await?;
    let latest = &release.tag_name;

    let current_semver = semver::Version::parse(CURRENT_VERSION)
        .map_err(|e| anyhow::anyhow!("Failed to parse current version '{}': {}", CURRENT_VERSION, e))?;
    let latest_semver = semver::Version::parse(latest.trim_start_matches('v'))
        .map_err(|e| anyhow::anyhow!("Failed to parse latest version '{}': {}", latest, e))?;

    eprintln!("Latest version:  {}", latest.bold().cyan());

    if current_semver >= latest_semver {
        eprintln!("{}", "Already up to date.".green());
        return Ok(());
    }

    if check_only {
        eprintln!();
        eprintln!("Run {} to upgrade.", "fr upgrade".bold());
        return Ok(());
    }

    let platform = detect_platform()?;
    let asset_name = format!("fr-{}.tar.gz", platform);

    let asset_url = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .map(|a| a.browser_download_url.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No release asset found for platform: {}\nSee: {}/tag/{}",
                platform,
                GITHUB_RELEASES,
                latest
            )
        })?;

    // Find checksums.txt asset URL
    let checksums_url = release
        .assets
        .iter()
        .find(|a| a.name == "checksums.txt")
        .map(|a| a.browser_download_url.as_str());

    eprintln!("Downloading {}...", asset_name);

    let binary = download_and_extract(asset_url, &asset_name, checksums_url, allow_unsigned).await?;

    let current_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot determine current executable path: {}", e))?;

    atomic_replace(&current_exe, &binary)?;

    eprintln!(
        "{} Upgraded to {}",
        "✓".green().bold(),
        latest.bold().cyan()
    );

    Ok(())
}

/// Fetch latest release metadata from GitHub API
async fn fetch_latest_release() -> Result<GithubRelease> {
    let client = reqwest::Client::builder()
        .user_agent(format!("forge-cli/{}", CURRENT_VERSION))
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .get(GITHUB_API)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to reach GitHub API: {}", e))?;

    if !resp.status().is_success() {
        bail!(
            "GitHub API returned {}: {}",
            resp.status(),
            GITHUB_RELEASES
        );
    }

    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse GitHub API response: {}", e))?;

    Ok(release)
}

/// Download .tar.gz archive, verify checksum, extract and return binary bytes.
/// If checksums.txt is unavailable, the upgrade is aborted unless `allow_unsigned` is true.
async fn download_and_extract(
    url: &str,
    asset_name: &str,
    checksums_url: Option<&str>,
    allow_unsigned: bool,
) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent(format!("forge-cli/{}", CURRENT_VERSION))
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Download failed: {}", e))?
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

    // Verify SHA256 checksum. By default, absence of checksums.txt is a hard error:
    // this prevents silent MITM / corruption. Use --allow-unsigned to override
    // (e.g. for offline/private releases that do not publish checksums).
    match checksums_url {
        Some(url) => verify_checksum(&client, url, asset_name, &bytes).await?,
        None if allow_unsigned => {
            eprintln!(
                "{}",
                "Warning: checksums.txt not found; skipping integrity check (--allow-unsigned)"
                    .yellow()
            );
        }
        None => bail!(
            "Release has no checksums.txt — refusing to install unverified binary.\n\
             If this is expected (e.g. private/offline release), re-run with --allow-unsigned."
        ),
    }

    // Decompress: .tar.gz → tar → find "fr" or "fr.exe" entry
    use std::io::Read;
    let gz = flate2::read::GzDecoder::new(bytes.as_ref());
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if name == "fr" || name == "fr.exe" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }

    bail!("Binary 'fr' not found in downloaded archive")
}

/// Verify SHA256 checksum of downloaded bytes against checksums.txt
async fn verify_checksum(
    client: &reqwest::Client,
    checksums_url: &str,
    asset_name: &str,
    bytes: &[u8],
) -> Result<()> {
    use sha2::{Digest, Sha256};

    let checksums_text = client
        .get(checksums_url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to download checksums.txt: {}", e))?
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read checksums.txt: {}", e))?;

    // Format: "<sha256>  <filename>" (sha256sum output format)
    let expected_hash = checksums_text
        .lines()
        .find_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 && parts[1] == asset_name {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No checksum found for '{}' in checksums.txt",
                asset_name
            )
        })?;

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != expected_hash {
        bail!(
            "Checksum mismatch!\n  Expected: {}\n  Actual:   {}\nThe downloaded file may be corrupted or tampered with.",
            expected_hash,
            actual_hash
        );
    }

    eprintln!("{} Checksum verified (SHA256)", "✓".green());
    Ok(())
}

/// Atomically replace current executable with new binary bytes (with backup)
fn atomic_replace(current_exe: &std::path::Path, new_bytes: &[u8]) -> Result<()> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine parent directory of executable"))?;

    // Backup current binary before replacing
    if current_exe.exists() {
        let forge_home = dirs::home_dir()
            .map(|h| h.join(".forge"))
            .unwrap_or_else(|| parent.to_path_buf());
        let backup_dir = forge_home.join("backup");
        if let Err(e) = std::fs::create_dir_all(&backup_dir) {
            eprintln!(
                "{} Could not create backup dir: {}",
                "Warning:".yellow(),
                e
            );
        } else {
            let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S");
            let bak_path = backup_dir.join(format!("fr.{}.bak", timestamp));
            if let Err(e) = std::fs::copy(current_exe, &bak_path) {
                eprintln!("{} Could not backup current binary: {}", "Warning:".yellow(), e);
            } else {
                eprintln!("Backed up current version to {}", bak_path.display());
                // Keep only last 5 backups
                cleanup_old_backups(&backup_dir, 5);
            }
        }
    }

    // Write to a temp file in same directory (ensures same filesystem for atomic mv)
    let tmp_path = parent.join(format!(".fr.upgrade.{}", std::process::id()));

    std::fs::write(&tmp_path, new_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to write upgrade binary: {}", e))?;

    // Set executable bit
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| anyhow::anyhow!("Failed to set permissions: {}", e))?;
    }

    install_binary(&tmp_path, current_exe)?;

    Ok(())
}

/// Move `tmp_path` into `current_exe`. On Unix this is a simple atomic rename.
/// On Windows, rename can't overwrite an executable that's currently mapped into
/// the running process — but it *can* move that executable aside. So we first
/// rename the running binary to a sibling `.old` name, then rename the new
/// binary into place. The `.old` file can't be deleted while still in use;
/// best-effort cleanup handles the case where it can (e.g. next upgrade).
#[cfg(not(windows))]
fn install_binary(tmp_path: &std::path::Path, current_exe: &std::path::Path) -> Result<()> {
    std::fs::rename(tmp_path, current_exe).map_err(|e| {
        let _ = std::fs::remove_file(tmp_path);
        anyhow::anyhow!("Failed to replace binary (try sudo?): {}", e)
    })
}

#[cfg(windows)]
fn install_binary(tmp_path: &std::path::Path, current_exe: &std::path::Path) -> Result<()> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine parent directory of executable"))?;

    // Clean up any leftover .old files from previous upgrades that weren't in use.
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with(".fr.old.")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    if current_exe.exists() {
        let old_path = parent.join(format!(".fr.old.{}", std::process::id()));
        // Stale .old with our PID shouldn't exist, but guard anyway.
        let _ = std::fs::remove_file(&old_path);

        std::fs::rename(current_exe, &old_path).map_err(|e| {
            let _ = std::fs::remove_file(tmp_path);
            anyhow::anyhow!("Failed to move running binary aside: {}", e)
        })?;

        if let Err(e) = std::fs::rename(tmp_path, current_exe) {
            // Restore the original so the user isn't left without a working binary.
            let _ = std::fs::rename(&old_path, current_exe);
            let _ = std::fs::remove_file(tmp_path);
            return Err(anyhow::anyhow!("Failed to install new binary: {}", e));
        }

        // The old binary is still mapped by this process; deletion is best-effort.
        let _ = std::fs::remove_file(&old_path);
    } else {
        std::fs::rename(tmp_path, current_exe).map_err(|e| {
            let _ = std::fs::remove_file(tmp_path);
            anyhow::anyhow!("Failed to install new binary: {}", e)
        })?;
    }

    Ok(())
}

fn cleanup_old_backups(backup_dir: &std::path::Path, keep: usize) {
    let mut entries: Vec<_> = std::fs::read_dir(backup_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("fr.") && n.ends_with(".bak"))
                .unwrap_or(false)
        })
        .collect();

    // Sort by name descending (timestamp in name ensures chronological order)
    entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

    for old in entries.into_iter().skip(keep) {
        let _ = std::fs::remove_file(old.path());
    }
}

fn detect_platform() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("macos", "x86_64") => Ok("x86_64-apple-darwin".to_string()),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin".to_string()),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu".to_string()),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu".to_string()),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc".to_string()),
        (os, arch) => bail!("Unsupported platform: {}-{}", arch, os),
    }
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}
