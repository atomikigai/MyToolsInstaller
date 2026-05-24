use anyhow::Result;

use crate::util::{run_shell, write_fish_snippet};

const FISH_SNIPPET: &str = r#"# fnm — installed by custom-tools
if test -d "$HOME/.local/share/fnm"
    fish_add_path "$HOME/.local/share/fnm"
end
if command -q fnm
    fnm env --use-on-cd --shell fish | source
end
"#;

pub fn install() -> Result<()> {
    println!("→ installing fnm via official script");
    run_shell("curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell")?;

    println!("→ installing latest Node LTS via fnm");
    run_shell(
        r#"
        FNM_BIN="$HOME/.local/share/fnm/fnm"
        "$FNM_BIN" install --lts
        "$FNM_BIN" default lts-latest
        "#,
    )?;

    write_fish_snippet("fnm", FISH_SNIPPET)?;
    Ok(())
}
