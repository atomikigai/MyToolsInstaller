use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::{has, run};

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            // Prefer yay, fall back to paru
            let aur_helper = if has("yay") {
                "yay"
            } else if has("paru") {
                "paru"
            } else {
                bail!(
                    "no AUR helper found (yay or paru required to install TablePlus). \
                     Install one first: https://github.com/Jguer/yay"
                )
            };

            println!("→ installing TablePlus via {aur_helper} (AUR)");
            run(aur_helper, &["-S", "--needed", "--noconfirm", "tableplus"])
        }
        Distro::Other => {
            bail!("automatic TablePlus install only supported on Arch/CachyOS for now")
        }
    }
}
