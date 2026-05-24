use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::detect::home;

const STARSHIP_TOML: &str = include_str!("../assets/starship.toml");
const FISH_CONFIG: &str = include_str!("../assets/fish/config.fish");
const FISH_RUSTUP: &str = include_str!("../assets/fish/conf.d/rustup.fish");

pub fn apply() -> Result<()> {
    let home = home()?;
    let config_dir = home.join(".config");

    write_with_backup(&config_dir.join("starship.toml"), STARSHIP_TOML)?;
    write_with_backup(&config_dir.join("fish/config.fish"), FISH_CONFIG)?;
    write_with_backup(&config_dir.join("fish/conf.d/rustup.fish"), FISH_RUSTUP)?;

    println!("✓ configs written under {}", config_dir.display());
    Ok(())
}

fn write_with_backup(dest: &Path, content: &str) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    if dest.exists() {
        let existing = fs::read_to_string(dest).with_context(|| format!("reading {}", dest.display()))?;
        if existing == content {
            println!("= {} already up to date", dest.display());
            return Ok(());
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = dest.with_extension(format!(
            "{}.bak.{ts}",
            dest.extension().and_then(|e| e.to_str()).unwrap_or("")
        ));
        fs::rename(dest, &backup)
            .with_context(|| format!("backing up {} -> {}", dest.display(), backup.display()))?;
        println!("↻ backed up existing {} → {}", dest.display(), backup.display());
    }

    fs::write(dest, content).with_context(|| format!("writing {}", dest.display()))?;
    println!("→ wrote {}", dest.display());
    Ok(())
}
