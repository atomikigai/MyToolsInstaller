use anyhow::{Context, Result};

use crate::detect::Distro;
use crate::util::run;

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing PostgreSQL via pacman");
            run("sudo", &["pacman", "-S", "--needed", "--noconfirm", "postgresql"])?;

            println!("→ initializing PostgreSQL cluster");
            run(
                "sudo",
                &[
                    "-u", "postgres",
                    "initdb",
                    "--locale", "en_US.UTF-8",
                    "-D", "/var/lib/postgres/data",
                ],
            )?;

            println!("→ enabling and starting postgresql service");
            run("sudo", &["systemctl", "enable", "--now", "postgresql"])
        }
        Distro::Other => {
            anyhow::bail!("automatic PostgreSQL install only supported on Arch/CachyOS for now")
        }
    }
}

/// Set the password for the `postgres` superuser.
/// Uses piped output so nothing bleeds into the TUI terminal.
pub fn set_password(password: &str) -> Result<()> {
    let sql = format!("ALTER USER postgres WITH PASSWORD '{}';", password.replace('\'', "''"));
    let out = std::process::Command::new("sudo")
        .args(["-u", "postgres", "psql", "-c", &sql])
        .output()
        .context("failed to run psql")?;

    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("{}", msg.trim());
    }
    Ok(())
}
