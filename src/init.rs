use anyhow::{bail, Result};
use colored::Colorize;
use dialoguer::{Confirm, Input};
use std::path::PathBuf;

pub fn run(path: Option<PathBuf>) -> Result<()> {
    println!("{}", "Initializing a new forge workspace...".bold());
    println!();

    // Workspace name — default to path arg or current directory name
    let default_name = if let Some(p) = &path {
        p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "my-project".to_string())
    } else {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-project".to_string())
    };

    let name: String = Input::new()
        .with_prompt("Workspace name")
        .default(default_name)
        .interact_text()?;

    // Description — optional
    let description: String = Input::new()
        .with_prompt("Description (optional)")
        .allow_empty(true)
        .default(String::new())
        .interact_text()?;

    // Parallel startup
    let parallel_startup = Confirm::new()
        .with_prompt("Enable parallel startup?")
        .default(true)
        .interact()?;

    // Determine target directory: explicit path > name-based subdirectory > current dir
    let target_dir = if let Some(p) = path {
        p
    } else {
        PathBuf::from(&name)
    };

    if target_dir.join("forge.toml").exists() {
        bail!("forge.toml already exists in {}", target_dir.display());
    }

    std::fs::create_dir_all(&target_dir)?;

    // Build forge.toml content
    let mut content = String::new();
    content.push_str("[workspace]\n");
    content.push_str(&format!("name = \"{}\"\n", escape_toml(&name)));

    if !description.is_empty() {
        content.push_str(&format!(
            "description = \"{}\"\n",
            escape_toml(&description)
        ));
    }

    content.push_str(&format!("parallel_startup = {}\n", parallel_startup));

    let config_path = target_dir.join("forge.toml");
    std::fs::write(&config_path, &content)?;

    println!();
    println!(
        "{} Created {}",
        "✓".green().bold(),
        config_path.display().to_string().bold()
    );

    Ok(())
}

fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
