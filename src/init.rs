use anyhow::{bail, Result};
use colored::Colorize;
use std::path::PathBuf;

pub struct InitOptions {
    pub path: Option<PathBuf>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub parallel: Option<bool>,
}

pub fn run(opts: InitOptions) -> Result<()> {
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());

    // Determine target directory
    let target_dir = match &opts.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };

    // Resolve workspace name
    let name = match opts.name {
        Some(n) => n,
        None if is_tty => {
            use dialoguer::Input;
            let default_name = target_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "my-project".to_string());
            Input::new()
                .with_prompt("Workspace name")
                .default(default_name)
                .interact_text()?
        }
        None => {
            // Non-interactive: derive from directory name
            target_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "my-project".to_string())
        }
    };

    // Resolve description
    let description = match opts.description {
        Some(d) => d,
        None if is_tty => {
            use dialoguer::Input;
            Input::new()
                .with_prompt("Description (optional)")
                .allow_empty(true)
                .default(String::new())
                .interact_text()?
        }
        None => String::new(),
    };

    // Resolve parallel startup
    let parallel_startup = match opts.parallel {
        Some(p) => p,
        None if is_tty => {
            use dialoguer::Confirm;
            Confirm::new()
                .with_prompt("Enable parallel startup?")
                .default(true)
                .interact()?
        }
        None => true,
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

    // Auto-add .forge/ to .gitignore
    ensure_gitignore(&target_dir);

    if is_tty {
        println!();
    }
    println!(
        "{} Created {}",
        "✓".green().bold(),
        config_path.display().to_string().bold()
    );

    Ok(())
}

/// Ensure .forge/ is in .gitignore, creating the file if needed.
pub fn ensure_gitignore(dir: &std::path::Path) {
    let gitignore = dir.join(".gitignore");
    let entry = ".forge/";

    if let Ok(content) = std::fs::read_to_string(&gitignore) {
        if content.lines().any(|line| line.trim() == entry) {
            return; // Already present
        }
        // Append
        let separator = if content.ends_with('\n') { "" } else { "\n" };
        let _ = std::fs::write(&gitignore, format!("{}{}{}\n", content, separator, entry));
    } else {
        // Create new
        let _ = std::fs::write(&gitignore, format!("{}\n", entry));
    }
}

fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
