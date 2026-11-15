#!/usr/bin/env bash
# Build, install and scaffold rune-tui plugins (see docs/PLUGINS.md).
#
#   scripts/plugin.sh new NAME           scaffold plugins/NAME from the template
#   scripts/plugin.sh build DIR          build DIR's module next to its plugin.json
#   scripts/plugin.sh install DIR [TO]   build, then copy into the plugin directory
#                                        (TO, default $XDG_DATA_HOME/rune/plugins)
#
# Installing only copies files; rune-tui runs a plugin once you enable it in
# Settings › Plugins and approve its permissions. A running rune-tui notices
# the copied files within seconds (a rebuilt module asks for approval again,
# unless its page says to load changed versions automatically).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/plugins}"

die() { echo "plugin.sh: $*" >&2; exit 1; }

# The value of a top-level string field of a plugin.json (no jq needed).
field() {
    sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1/plugin.json" | head -n1
}

build() {
    local dir="$1"
    [[ -f "$dir/plugin.json" && -f "$dir/Cargo.toml" ]] || die "$dir needs plugin.json and Cargo.toml"
    rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown \
        || die "run: rustup target add wasm32-unknown-unknown"
    local entry crate
    entry="$(field "$dir" entry)"
    entry="${entry:-plugin.wasm}"
    crate="$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$dir/Cargo.toml" | head -n1)"
    (cd "$dir" && cargo build --release --target wasm32-unknown-unknown)
    install -m 0644 "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/${crate//-/_}.wasm" "$dir/$entry"
    echo "built $dir/$entry"
}

install_plugin() {
    local dir="$1" to="${2:-${XDG_DATA_HOME:-$HOME/.local/share}/rune/plugins}"
    build "$dir"
    local id entry
    id="$(field "$dir" id)"
    entry="$(field "$dir" entry)"
    entry="${entry:-plugin.wasm}"
    [[ -n "$id" ]] || die "$dir/plugin.json has no id"
    install -d -m 0755 "$to/$id"
    install -m 0644 "$dir/plugin.json" "$dir/$entry" "$to/$id/"
    echo "installed to $to/$id — rune-tui picks it up; enable it in Settings › Plugins"
}

scaffold() {
    local name="$1" dir="$root/plugins/$1"
    [[ "$name" =~ ^[a-z][a-z0-9-]{0,31}$ ]] || die "NAME: lowercase letters, digits and -"
    [[ ! -e "$dir" ]] || die "$dir already exists"
    mkdir -p "$dir/src"
    cat >"$dir/Cargo.toml" <<EOF
[package]
name = "$name"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
rune-plugin = { path = "../../plugin-sdk" }
serde_json = "1"

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true

[workspace]
EOF
    cat >"$dir/plugin.json" <<EOF
{
  "id": "$name",
  "name": "$name",
  "version": "0.1.0",
  "api_version": "1.0",
  "description": "",
  "entry": "$name.wasm",
  "permissions": [],
  "commands": [{ "name": "$name", "help": "say hello" }]
}
EOF
    cat >"$dir/src/lib.rs" <<EOF
use rune_plugin::prelude::*;

#[derive(Default)]
pub struct Plugin_;

impl Plugin for Plugin_ {
    fn handle(&mut self, _cx: &Context, event: Event) -> Vec<Action> {
        match event {
            Event::Command { args, chat, .. } => {
                let place = chat.map_or("nowhere".to_owned(), |c| c.name);
                vec![Action::status(format!("hello from $name in {place}: {args}"))]
            }
            _ => Vec::new(),
        }
    }
}

rune_plugin::export_plugin!(Plugin_);
EOF
    echo "created $dir — next: scripts/plugin.sh install plugins/$name"
}

case "${1:-}" in
    new) [[ $# -eq 2 ]] || die "usage: new NAME"; scaffold "$2" ;;
    build) [[ $# -eq 2 ]] || die "usage: build DIR"; build "$2" ;;
    install) [[ $# -ge 2 && $# -le 3 ]] || die "usage: install DIR [TO]"; install_plugin "$2" "${3:-}" ;;
    *) sed -n '2,10s/^# \{0,1\}//p' "$0"; exit 2 ;;
esac
