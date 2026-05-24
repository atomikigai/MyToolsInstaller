use anyhow::Result;

use crate::util::{run_shell, write_fish_snippet};

const FISH_SNIPPET: &str = r#"# pnpm — installed by custom-tools
set -gx PNPM_HOME "$HOME/.local/share/pnpm"
if test -d "$PNPM_HOME"
    fish_add_path "$PNPM_HOME"
end
"#;

pub fn install() -> Result<()> {
    println!("→ installing pnpm via official standalone script");
    // The standalone installer bundles its own Node runtime,
    // so it works even before the system node is on PATH.
    run_shell("curl -fsSL https://get.pnpm.io/install.sh | sh -")?;
    write_fish_snippet("pnpm", FISH_SNIPPET)?;
    Ok(())
}
