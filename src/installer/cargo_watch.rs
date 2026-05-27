use anyhow::{Result, bail};

use crate::util::{has, run_shell};

pub fn install() -> Result<()> {
    if !has("cargo") {
        bail!("cargo-watch needs cargo — install rust first (`custom-tools install rust`)");
    }
    println!("→ installing cargo-watch via `cargo install`");
    run_shell("cargo install cargo-watch --locked")
}
