use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::run;

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing ripgrep (rg) via pacman");
            run("sudo", &["pacman", "-S", "--needed", "--noconfirm", "ripgrep"])
        }
        Distro::Other => bail!("automatic ripgrep install only supported on Arch/CachyOS for now"),
    }
}
