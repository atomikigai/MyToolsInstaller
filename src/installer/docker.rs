use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::run;

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing Docker via pacman");
            run("sudo", &["pacman", "-S", "--needed", "--noconfirm", "docker", "docker-compose"])?;

            println!("→ enabling and starting Docker service");
            run("sudo", &["systemctl", "enable", "--now", "docker"])?;

            // Add current user to the docker group so sudo isn't needed
            let user = std::env::var("USER").unwrap_or_else(|_| "$(whoami)".into());
            println!("→ adding '{user}' to the docker group");
            run("sudo", &["usermod", "-aG", "docker", &user])?;

            println!("ℹ  log out and back in (or run `newgrp docker`) for group change to take effect");
            Ok(())
        }
        Distro::Other => bail!("automatic Docker install only supported on Arch/CachyOS for now"),
    }
}
