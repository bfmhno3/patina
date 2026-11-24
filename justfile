set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments := true

material_version := "1.1.0"

# List recipes
default:
    @just --list

# Format Rust, Slint, and Nix sources
fmt:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s globstar nullglob
    cargo fmt --all
    slint_files=(crates/**/*.slint)
    if (( ${#slint_files[@]} )); then slint-lsp format -i "${slint_files[@]}"; fi
    nixfmt flake.nix

# Fail if any Rust, Slint, or Nix source is unformatted
fmt-check:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s globstar nullglob
    cargo fmt --all --check
    for f in crates/**/*.slint; do slint-lsp format "$f" | diff -u "$f" -; done
    nixfmt --check flake.nix

# Run clippy on every crate and target, warnings are errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run all tests
test:
    cargo test --workspace

# Everything that must pass before a commit
check: fmt-check lint test

# Run the CLI, e.g. `just cli --version`
cli *args:
    cargo run -q -p patina-cli -- "$@"

# Run the desktop GUI
gui:
    cargo run -p patina-gui

# Run the GUI with Slint live preview (reloads .slint files on save)
gui-preview:
    SLINT_LIVE_PREVIEW=1 cargo run -p patina-gui --features slint/live-preview

# Build a debug APK (needs `nix develop .#android`)
[working-directory: 'crates/patina-gui']
android-build target="aarch64-linux-android":
    cargo apk build --lib --target {{target}}

# Build, install, and launch on the connected device (needs `nix develop .#android`)
[working-directory: 'crates/patina-gui']
android-run target="aarch64-linux-android":
    cargo apk run --lib --target {{target}}

# Fetch a Material Symbols Outlined icon, e.g. `just icon check_circle`
icon name:
    mkdir -p crates/patina-gui/ui/icons
    curl -fsSL "https://raw.githubusercontent.com/google/material-design-icons/master/symbols/web/{{name}}/materialsymbolsoutlined/{{name}}_24px.svg" -o "crates/patina-gui/ui/icons/{{name}}.svg"

# Replace vendor/slint-material with an upstream release
vendor-material version=material_version:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "https://material.slint.dev/zip/material-{{version}}.zip" -o "$tmp/material.zip"
    unzip -q "$tmp/material.zip" -d "$tmp"
    rm -rf vendor/slint-material
    mkdir -p vendor
    mv "$tmp/material-{{version}}" vendor/slint-material
