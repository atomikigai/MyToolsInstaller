use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::run;

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing fd via pacman");
            run("sudo", &["pacman", "-S", "--needed", "--noconfirm", "fd"])
        }
        Distro::Other => bail!("automatic fd install only supported on Arch/CachyOS for now"),
    }
}
