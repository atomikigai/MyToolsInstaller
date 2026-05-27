use anyhow::{Result, bail};

use crate::util::{has, run_shell};

pub fn install() -> Result<()> {
    if !has("cargo") {
        bail!("sqlx-cli needs cargo — install rust first (`custom-tools install rust`)");
    }
    println!("→ installing sqlx-cli via `cargo install`");
    run_shell("cargo install sqlx-cli --no-default-features --features rustls,postgres")
}
