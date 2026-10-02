#!/bin/sh
# Builds wikirs in release mode and installs it.
#
#   scripts/install.sh               # binary in ~/.local/bin, config in ~/.config/wikirs
#   scripts/install.sh --bin-dir DIR # install the binary somewhere else
#   scripts/install.sh --headless    # CLI only: no MCP, HTTP or TUI (and no tokio)
#   scripts/install.sh --uninstall   # remove the binary (config is left alone)
#
# The config dir is the one wikirs reads: $XDG_CONFIG_HOME/wikirs if that's
# set, else ~/.config/wikirs. A starter config.toml is written only if none
# exists, and it's never overwritten. Wikis themselves are never touched.
set -eu

bin_dir="${HOME}/.local/bin"
features=""
uninstall=false

usage() {
    sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir)
            [ $# -ge 2 ] || { echo "error: --bin-dir needs a directory" >&2; exit 2; }
            bin_dir="$2"
            shift 2
            ;;
        --headless) features="--no-default-features"; shift ;;
        --uninstall) uninstall=true; shift ;;
        -h | --help) usage; exit 0 ;;
        *) echo "error: unknown option '$1' (try --help)" >&2; exit 2 ;;
    esac
done

case "${XDG_CONFIG_HOME:-}" in
    /*) config_dir="${XDG_CONFIG_HOME}/wikirs" ;;
    *) config_dir="${HOME}/.config/wikirs" ;; # wikirs ignores a relative XDG_CONFIG_HOME too
esac
target="${bin_dir}/wikirs"

if [ "$uninstall" = true ]; then
    if [ -e "$target" ]; then
        rm -f "$target"
        echo "removed ${target}"
    else
        echo "nothing to remove at ${target}"
    fi
    echo "config left in ${config_dir}"
    exit 0
fi

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found: install Rust from https://rustup.rs" >&2
    exit 1
}

repo="$(cd "$(dirname "$0")/.." && pwd)"
echo "building wikirs (release${features:+, headless})..."
# shellcheck disable=SC2086 # $features is empty or one flag
cargo build --release --locked --manifest-path "${repo}/Cargo.toml" $features

mkdir -p "$bin_dir"
# Copy then rename, so a running wikirs is never overwritten in place (macOS
# kills a signed binary whose file changes under it).
tmp="${bin_dir}/.wikirs.install.$$"
cp "${repo}/target/release/wikirs" "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$target"
echo "installed ${target} ($("$target" --version))"

mkdir -p "$config_dir"
if [ ! -e "${config_dir}/config.toml" ]; then
    cat >"${config_dir}/config.toml" <<'EOF'
# wikirs user config (docs/spec/wiki-selection.md).
#
# The Wiki to use when no --wiki is given, WIKIRS_WIKI is unset and the
# current directory isn't inside one:
# default_wiki = "notes"
#
# Named Wikis, so `--wiki notes` works on every machine whatever the path:
# [wikis]
# notes = "~/notes"
EOF
    echo "wrote ${config_dir}/config.toml"
else
    echo "kept ${config_dir}/config.toml"
fi

case ":${PATH}:" in
    *":${bin_dir}:"*) ;;
    *) echo "note: ${bin_dir} is not on your PATH; add it to use 'wikirs' directly" ;;
esac
