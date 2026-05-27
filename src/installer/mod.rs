use anyhow::Result;

use crate::detect::distro;
use crate::util::has;

pub mod cargo_watch;
pub mod docker;
pub mod fd;
pub mod fish;
pub mod fnm;
pub mod gh;
pub mod paru;
pub mod pnpm;
pub mod postgres;
pub mod ripgrep;
pub mod rust;
pub mod sqlx_cli;
pub mod starship;
pub mod tableplus;
pub mod xh;
pub mod yay;
pub mod zoxide;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum Tool {
    Fish,
    Rust,
    Starship,
    Fnm,
    Pnpm,
    Xh,
    Rg,
    Fd,
    Yay,
    Paru,
    Postgres,
    Docker,
    TablePlus,
    Zoxide,
    Gh,
    CargoWatch,
    SqlxCli,
}

impl Tool {
    /// All tools in recommended install order.
    pub const ALL: &'static [Tool] = &[
        Tool::Fish,
        Tool::Rust,
        Tool::Starship,
        Tool::Fnm,
        Tool::Pnpm,
        Tool::Xh,
        Tool::Rg,
        Tool::Fd,
        Tool::Yay,
        Tool::Paru,
        Tool::Postgres,
        Tool::Docker,
        Tool::TablePlus,
        Tool::Zoxide,
        Tool::Gh,
        Tool::CargoWatch,
        Tool::SqlxCli,
    ];

    /// Tools that must be installed before this one.
    pub fn deps(self) -> &'static [Tool] {
        match self {
            Tool::Paru       => &[Tool::Rust],
            Tool::TablePlus  => &[Tool::Yay],
            Tool::CargoWatch => &[Tool::Rust],
            Tool::SqlxCli    => &[Tool::Rust],
            _               => &[],
        }
    }

    /// The string clap accepts for `custom-tools install <arg>`.
    /// Must match the kebab-case ValueEnum name (PascalCase → kebab-case).
    pub fn arg_name(self) -> &'static str {
        match self {
            Tool::Fish      => "fish",
            Tool::Rust      => "rust",
            Tool::Starship  => "starship",
            Tool::Fnm       => "fnm",
            Tool::Pnpm      => "pnpm",
            Tool::Xh        => "xh",
            Tool::Rg        => "rg",
            Tool::Fd        => "fd",
            Tool::Yay       => "yay",
            Tool::Paru      => "paru",
            Tool::Postgres  => "postgres",   // bin is "psql", arg is "postgres"
            Tool::Docker    => "docker",
            Tool::TablePlus => "table-plus", // ValueEnum kebab-cases "TablePlus"
            Tool::Zoxide    => "zoxide",
            Tool::Gh        => "gh",
            Tool::CargoWatch => "cargo-watch",
            Tool::SqlxCli   => "sqlx-cli",
        }
    }

    /// True if installing this tool acquires the pacman lock.
    /// These tools must NOT run concurrently with each other.
    pub fn needs_pacman_lock(self) -> bool {
        matches!(
            self,
            Tool::Fish | Tool::Rg | Tool::Fd
                | Tool::Postgres | Tool::Docker  // direct pacman
                | Tool::Yay | Tool::Paru         // makepkg → pacman
                | Tool::TablePlus                // yay/paru → pacman
                | Tool::Gh                       // direct pacman
        )
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Tool::Fish      => "fish        — shell",
            Tool::Rust      => "rust        — toolchain via rustup",
            Tool::Starship  => "starship    — cross-shell prompt",
            Tool::Fnm       => "fnm         — Node version manager + Node LTS",
            Tool::Pnpm      => "pnpm        — fast Node package manager",
            Tool::Xh        => "xh          — friendly HTTP client (curl alternative)",
            Tool::Rg        => "rg          — ripgrep, fast recursive search",
            Tool::Fd        => "fd          — fast and friendly find alternative",
            Tool::Yay       => "yay         — AUR helper (Go)",
            Tool::Paru      => "paru        — AUR helper (Rust, needs rust first)",
            Tool::Postgres  => "postgresql  — database server (latest)",
            Tool::Docker    => "docker      — container runtime + compose",
            Tool::TablePlus => "tableplus   — database GUI (AUR, needs yay or paru)",
            Tool::Zoxide    => "zoxide      — smarter cd",
            Tool::Gh        => "gh          — GitHub CLI (browser device-flow login)",
            Tool::CargoWatch => "cargo-watch — watch & rerun cargo commands (needs rust)",
            Tool::SqlxCli   => "sqlx-cli    — SQLx CLI for Postgres migrations (needs rust)",
        }
    }

    pub fn bin_name(self) -> &'static str {
        match self {
            Tool::Fish      => "fish",
            Tool::Rust      => "cargo",
            Tool::Starship  => "starship",
            Tool::Fnm       => "fnm",
            Tool::Pnpm      => "pnpm",
            Tool::Xh        => "xh",
            Tool::Rg        => "rg",
            Tool::Fd        => "fd",
            Tool::Yay       => "yay",
            Tool::Paru      => "paru",
            Tool::Postgres  => "psql",
            Tool::Docker    => "docker",
            Tool::TablePlus => "tableplus",
            Tool::Zoxide    => "zoxide",
            Tool::Gh        => "gh",
            Tool::CargoWatch => "cargo-watch",
            Tool::SqlxCli   => "sqlx",
        }
    }

    /// True if the tool is installed. Uses `which` first, then falls back to
    /// well-known install paths for tools (fnm, pnpm, rustup, etc.) whose
    /// official installers don't add themselves to PATH unless the user opts
    /// in to shell-config edits.
    pub fn is_installed(self) -> bool {
        if has(self.bin_name()) { return true; }
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return false;
        };
        let fallbacks: &[&str] = match self {
            Tool::Fnm      => &[".local/share/fnm/fnm"],
            Tool::Pnpm     => &[".local/share/pnpm/pnpm"],
            Tool::Rust     => &[".cargo/bin/cargo", ".cargo/bin/rustc"],
            Tool::Starship => &[".local/bin/starship"],
            Tool::Xh       => &[".local/bin/xh", ".cargo/bin/xh"],
            Tool::Zoxide   => &[".local/bin/zoxide", ".cargo/bin/zoxide"],
            Tool::CargoWatch => &[".cargo/bin/cargo-watch"],
            Tool::SqlxCli   => &[".cargo/bin/sqlx"],
            _              => return false,
        };
        fallbacks.iter().any(|p| home.join(p).exists())
    }

    pub fn install(self) -> Result<()> {
        if self.is_installed() {
            println!("✓ {} already installed, skipping", self.bin_name());
            return Ok(());
        }
        let d = distro();
        match self {
            Tool::Fish      => fish::install(d),
            Tool::Rust      => rust::install(),
            Tool::Starship  => starship::install(),
            Tool::Fnm       => fnm::install(),
            Tool::Pnpm      => pnpm::install(),
            Tool::Xh        => xh::install(),
            Tool::Rg        => ripgrep::install(d),
            Tool::Fd        => fd::install(d),
            Tool::Yay       => yay::install(d),
            Tool::Paru      => paru::install(d),
            Tool::Postgres  => postgres::install(d),
            Tool::Docker    => docker::install(d),
            Tool::TablePlus => tableplus::install(d),
            Tool::Zoxide    => zoxide::install(),
            Tool::Gh        => gh::install(d),
            Tool::CargoWatch => cargo_watch::install(),
            Tool::SqlxCli   => sqlx_cli::install(),
        }
    }

    /// Tools that show a post-install configuration prompt in the TUI.
    pub fn has_post_install(self) -> bool {
        matches!(self, Tool::Postgres)
    }
}

