use anyhow::Result;

use crate::util::{run_shell, write_fish_snippet};

const FISH_SNIPPET: &str = r#"# starship — installed by custom-tools
if command -q starship
    starship init fish | source
end
"#;

pub fn install() -> Result<()> {
    println!("→ installing starship via official script");
    run_shell("curl -sS https://starship.rs/install.sh | sh -s -- -y")?;
    write_fish_snippet("starship", FISH_SNIPPET)?;
    Ok(())
}
