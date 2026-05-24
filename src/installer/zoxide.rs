use anyhow::Result;

use crate::util::{run_shell, write_fish_snippet};

const FISH_SNIPPET: &str = r#"# zoxide — installed by custom-tools
fish_add_path "$HOME/.local/bin"
if command -q zoxide
    zoxide init fish | source
end
"#;

pub fn install() -> Result<()> {
    println!("→ installing zoxide via official script");
    run_shell("curl -sSfL https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh | sh")?;
    write_fish_snippet("zoxide", FISH_SNIPPET)?;
    Ok(())
}
