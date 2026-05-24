#!/usr/bin/env bash
set -euo pipefail

REPO_URL="https://github.com/atomikigai/MyToolsInstaller.git"
CARGO_BIN="custom-tools"       # binary name produced by cargo build
BIN_NAME="mytools"             # final name installed on the system
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\033[1;31mxx\033[0m  %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }

ensure_rust() {
    if ! command -v cargo >/dev/null 2>&1; then
        log "cargo not found, installing rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -f "$SCRIPT_DIR/Cargo.toml" ]]; then
    SRC_DIR="$SCRIPT_DIR"
    log "Building from local checkout: $SRC_DIR"
else
    need git
    SRC_DIR="$(mktemp -d -t custom-tools.XXXXXX)"
    trap 'rm -rf "$SRC_DIR"' EXIT
    log "Cloning $REPO_URL into $SRC_DIR"
    git clone --depth 1 "$REPO_URL" "$SRC_DIR"
fi

ensure_rust

log "Compiling $CARGO_BIN (release)"
cargo build --release --manifest-path "$SRC_DIR/Cargo.toml"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$SRC_DIR/target/release/$CARGO_BIN" "$INSTALL_DIR/$BIN_NAME"
log "Installed $INSTALL_DIR/$BIN_NAME"

# --- Make $INSTALL_DIR available in the user's shells -----------------------
# Idempotent: each shell config gets touched at most once.
configure_path() {
    local dir="$1"
    local touched=0

    add_posix_line() {
        local file="$1"
        local marker="# added by custom-tools installer ($BIN_NAME)"
        [[ -f "$file" ]] || return 0
        if grep -Fq "$marker" "$file"; then return 0; fi
        {
            printf '\n%s\n' "$marker"
            printf 'case ":$PATH:" in *":%s:"*) ;; *) export PATH="%s:$PATH" ;; esac\n' "$dir" "$dir"
        } >> "$file"
        log "PATH entry appended to $file"
        touched=1
    }

    # bash: prefer .bashrc, fall back to .bash_profile
    if command -v bash >/dev/null 2>&1; then
        if [[ -f "$HOME/.bashrc" ]]; then
            add_posix_line "$HOME/.bashrc"
        elif [[ -f "$HOME/.bash_profile" ]]; then
            add_posix_line "$HOME/.bash_profile"
        fi
    fi

    # zsh
    if command -v zsh >/dev/null 2>&1 && [[ -f "$HOME/.zshrc" ]]; then
        add_posix_line "$HOME/.zshrc"
    fi

    # fish: drop a conf.d snippet (always loaded, no .config edits needed)
    if command -v fish >/dev/null 2>&1; then
        local fish_conf="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d"
        mkdir -p "$fish_conf"
        local snippet="$fish_conf/${BIN_NAME}-path.fish"
        if [[ ! -f "$snippet" ]]; then
            cat > "$snippet" <<FISH
# added by custom-tools installer ($BIN_NAME)
if not contains "$dir" \$PATH
    set -gx PATH "$dir" \$PATH
end
FISH
            log "PATH entry written to $snippet"
            touched=1
        fi
    fi

    if (( touched )); then
        warn "Open a new shell (or 'source' your rc file) for $BIN_NAME to be on PATH."
    fi
}

if ! printf '%s' ":$PATH:" | grep -q ":$INSTALL_DIR:"; then
    configure_path "$INSTALL_DIR"
fi

printf '\n\033[1;32mInstalled!\033[0m  Next steps:\n\n'
cat <<EOF
  $BIN_NAME                 # interactive TUI (pick tools to install)
  $BIN_NAME install <tool>  # install a single tool non-interactively
  $BIN_NAME config          # copy bundled configs into ~/.config
  $BIN_NAME doctor          # report what's installed / missing
  $BIN_NAME --help          # full CLI reference

Tip: run \`$BIN_NAME doctor\` first to see the current state of your box.
EOF
