use anyhow::Result;

use crate::util::{run_shell, write_fish_snippet};

const FISH_SNIPPET: &str = r#"# xh — installed by custom-tools
fish_add_path "$HOME/.local/bin"
"#;

pub fn install() -> Result<()> {
    println!("→ installing xh via official script");
    run_shell("curl -sfL https://raw.githubusercontent.com/ducaale/xh/master/install.sh | sh")?;
    write_fish_snippet("xh", FISH_SNIPPET)?;
    Ok(())
}
