use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::run;

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing fish via pacman");
            run("sudo", &["pacman", "-S", "--needed", "--noconfirm", "fish"])?;
        }
        Distro::Other => bail!("automatic fish install only supported on Arch/CachyOS for now"),
    }

    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        println!("⚠  USER not set, skipping chsh");
        return Ok(());
    }

    println!("→ setting fish as default login shell for {user}");
    // chsh under sudo writes /etc/passwd directly; the cached sudo password
    // makes this non-interactive.
    if let Err(e) = run("sudo", &["chsh", "-s", "/usr/bin/fish", &user]) {
        println!("⚠  could not chsh: {e}");
    }
    Ok(())
}
