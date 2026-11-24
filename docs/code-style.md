# Code Style

This document applies to all code and docs in this repository. The tools named per language are the enforcement; `just fmt` and `just check` run them.

## General rules

- Files are UTF-8 with LF line endings and a final newline (`.editorconfig`).
- Identifiers and code comments are in English. The product design doc `docs/design.md` stays Chinese.
- No unrelated formatting churn. Restyle in separate commits.
- Quoted upstream code keeps its formatting. Never edit `vendor/`.

## Rust

- **Formatting**: `rustfmt` defaults (edition 2024, width 100). Never hand-format against it.
- **Lints**: `cargo clippy --workspace --all-targets -- -D warnings` is clean. `#[allow]` only with a comment saying why, scoped to the smallest item.
- **Naming**: Rust API Guidelines (RFC 430). `snake_case` for functions, variables, and modules; `UpperCamelCase` for types, traits, and variants; `SCREAMING_SNAKE_CASE` for constants. Acronyms are one word (`Uuid`, `Jsonl`). Conversions use `as_`, `to_`, `into_`; getters have no `get_` prefix. Crates are `patina-<name>` (directory and package); library names are `patina_<name>`.
- **Errors**: `patina-core` exposes typed error enums (`thiserror` when the first error type is added). Binaries (`patina-cli`, `patina-gui`) may use `anyhow` at the top level. No `unwrap`/`expect` in `patina-core` outside tests. `expect("<what failed>")` is allowed only at process entry points (`main`, `android_main`).
- **`unsafe`**: `patina-core` and `patina-cli` use `#![forbid(unsafe_code)]`. Elsewhere every `unsafe` block has a `// SAFETY:` comment and every `unsafe fn` documents `# Safety`.
- **Docs**: `//!` on every crate root. `///` on public items, with `# Errors`, `# Panics`, `# Safety` where they apply.
- **Time and I/O in core**: functions that depend on the current time take `now` as a parameter. Projections never touch the filesystem or the clock.
- **Tests**: unit tests in `#[cfg(test)] mod tests` beside the code; cross-module behavior in `crates/<crate>/tests/`. Use fixed `now` values. Fixtures are synthetic, never copied from a real log.
- **Dependencies**: declare versions once in root `[workspace.dependencies]`; crates use `dep.workspace = true`.

**Verification**: `just fmt-check`, `just lint`, `just test`.

## Slint

- **Formatting**: `slint-lsp format -i` (via `just fmt`).
- **Naming**: files are `snake_case.slint`. Components, structs, enums, and globals are `PascalCase`. Properties, callbacks, element ids, and built-in properties are written `snake_case` (for example `preferred_width`). Slint treats `-` and `_` as the same; the vendored Material library and the generated Rust API use snake_case, so one spelling stays greppable across Rust and Slint.
- **Layout of `crates/patina-gui/ui/`**: `main.slint` holds only `MainWindow`; screens go in `pages/`, reusable pieces in `components/`, one exported component per file; icons in `icons/`, registered in `icons.slint`.
- **Member order inside a component**: `in`, `out`, `in-out` properties, private properties, callbacks, functions, then child elements.
- **Presentation only**: no domain rules or data transformation in `.slint`. Rust computes view models and passes them in.
- **Material 3**: components come only from `@material`; colors, type, and spacing come from `MaterialPalette`, `MaterialTypography`, and `MaterialStyleMetrics`; no literal colors or font sizes; every user-visible string uses `@tr()`.

**Verification**: `just fmt-check`, `cargo check -p patina-gui` (compiles `.slint` through `build.rs`).

## Nix

`nixfmt` (RFC style), 2-space indent.

**Verification**: `nixfmt --check flake.nix`.

## just

- Recipe names are `kebab-case`.
- A `#` comment sits above every recipe.
- Recipes that need more than one shell line of state use a `#!/usr/bin/env bash` body with `set -euo pipefail`.
- Never hard-code absolute paths.

## Markdown

- GFM. Lists use `-` only.
- One space between Chinese and English or numbers.
- Punctuation follows the language of the sentence: full-width `，。：；？！、（）` in Chinese, half-width in English and code.
- Chinese text uses `“”` and `‘’`.
- No `—` or `——`.
- Inline math `$...$`; block math `$$` on separate lines. The design doc's existing ` ```latex ` blocks stay as they are.
- Diagrams are fenced `mermaid`. Admonitions are GitHub Alerts.
