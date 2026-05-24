mod config;
mod detect;
mod installer;
mod tui;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

use installer::Tool;

#[derive(Parser)]
#[command(
    name = "custom-tools",
    version,
    about = "Bootstrap your dev environment on a new Linux box"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Interactive TUI (no arg) or install a single tool directly
    Install {
        /// Skip the TUI and install one specific tool
        #[arg(value_enum)]
        tool: Option<Tool>,
    },
    /// Copy bundled configs into ~/.config
    Config,
    /// Report what's installed and what's missing
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Install { tool: None }) {
        Cmd::Install { tool: Some(t) } => t.install(),
        Cmd::Install { tool: None }    => install_interactive(),
        Cmd::Config                    => config::apply(),
        Cmd::Doctor                    => doctor(),
    }
}

fn install_interactive() -> Result<()> {
    let d = detect::distro();
    if d != detect::Distro::Arch {
        eprintln!("warning: non-Arch distro detected; only Arch/CachyOS is fully supported");
    }

    // Cache sudo credentials before entering the TUI so sudo doesn't
    // interrupt the install panel with a password prompt mid-way.
    println!("Some tools require sudo. Caching credentials...");
    let _ = std::process::Command::new("sudo").arg("-v").status();

    tui::run_installer()
}

fn doctor() -> Result<()> {
    println!("distro: {:?}", detect::distro());
    for tool in Tool::ALL {
        let mark = if tool.is_installed() { "✓" } else { "✗" };
        println!("  {mark} {}", tool.display_name());
    }
    Ok(())
}
