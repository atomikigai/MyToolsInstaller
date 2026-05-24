use anyhow::{Result, bail};

use crate::detect::Distro;
use crate::util::run_shell;

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing yay (AUR helper) from AUR");
            run_shell(
                r#"
                sudo pacman -S --needed --noconfirm git base-devel
                git clone https://aur.archlinux.org/yay.git /tmp/yay-install
                cd /tmp/yay-install && makepkg -si --noconfirm
                rm -rf /tmp/yay-install
                "#,
            )
        }
        Distro::Other => bail!("yay install only supported on Arch/CachyOS"),
    }
}
