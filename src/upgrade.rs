use anyhow::{bail, Result};
use colored::Colorize;

const GITHUB_API: &str = "https://api.github.com/repos/liuxin231/forge/releases/latest";
const GITHUB_RELEASES: &str = "https://github.com/liuxin231/forge/releases";

/// Current binary version from Cargo.toml
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(check_only: bool) -> Result<()> {
    let current = format!("v{}", CURRENT_VERSION);
    eprintln!("Current version: {}", current.bold());

    eprintln!("Checking for updates...");

    let release = fetch_latest_release().await?;
    let latest = &release.tag_name;

    if latest == &current || latest.trim_start_matches('v') == CURRENT_VERSION {
        eprintln!("{}", "Already up to date.".green());
        return Ok(());
    }

    eprintln!("Latest version:  {}", latest.bold().cyan());

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

    eprintln!("Downloading {}...", asset_name);

    let binary = download_and_extract(asset_url).await?;

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

/// Download .tar.gz archive, extract and return binary bytes
async fn download_and_extract(url: &str) -> Result<Vec<u8>> {
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

    // Decompress: .tar.gz → tar → find "fr" entry
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

        if name == "fr" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }

    bail!("Binary 'fr' not found in downloaded archive")
}

/// Atomically replace current executable with new binary bytes
fn atomic_replace(current_exe: &std::path::Path, new_bytes: &[u8]) -> Result<()> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine parent directory of executable"))?;

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

    // Atomic rename
    std::fs::rename(&tmp_path, current_exe).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::anyhow!("Failed to replace binary (try sudo?): {}", e)
    })?;

    Ok(())
}

fn detect_platform() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let os_part = match os {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => bail!("Unsupported OS: {}", other),
    };

    let arch_part = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("Unsupported architecture: {}", other),
    };

    Ok(format!("{}-{}", arch_part, os_part))
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
