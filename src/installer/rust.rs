use anyhow::Result;

use crate::util::{run_shell, write_fish_snippet};

const FISH_SNIPPET: &str = r#"# rust / cargo — installed by custom-tools
if test -f "$HOME/.cargo/env.fish"
    source "$HOME/.cargo/env.fish"
end
"#;

pub fn install() -> Result<()> {
    println!("→ installing rust via rustup-init");
    run_shell(
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable",
    )?;
    write_fish_snippet("rustup", FISH_SNIPPET)?;
    Ok(())
}
