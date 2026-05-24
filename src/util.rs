use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{program}`"))?;
    if !status.success() {
        bail!("`{program} {}` exited with {status}", args.join(" "));
    }
    Ok(())
}

pub fn run_shell(script: &str) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .status()
        .context("failed to spawn sh")?;
    if !status.success() {
        bail!("shell script exited with {status}");
    }
    Ok(())
}

pub fn has(bin: &str) -> bool {
    which::which(bin).is_ok()
}

/// Write a self-contained fish snippet to `~/.config/fish/conf.d/<name>.fish`.
/// Fish auto-loads every file in `conf.d/` at shell startup, so the tool's
/// PATH / init takes effect in any new fish session — no `config.fish` edits.
pub fn write_fish_snippet(name: &str, content: &str) -> Result<()> {
    let home = std::env::var("HOME").context("HOME unset")?;
    let dir = PathBuf::from(&home).join(".config/fish/conf.d");
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{name}.fish"));
    fs::write(&path, content)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("→ wrote fish snippet {}", path.display());
    Ok(())
}
