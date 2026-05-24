use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::{has, run_shell};

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            if !has("cargo") {
                bail!("paru is written in Rust — install rust first (`custom-tools install rust`)");
            }
            println!("→ installing paru (Rust-based AUR helper) from AUR");
            run_shell(
                r#"
                sudo pacman -S --needed --noconfirm git base-devel
                git clone https://aur.archlinux.org/paru.git /tmp/paru-install
                cd /tmp/paru-install && makepkg -si --noconfirm
                rm -rf /tmp/paru-install
                "#,
            )
        }
        Distro::Other => bail!("paru install only supported on Arch/CachyOS"),
    }
}
