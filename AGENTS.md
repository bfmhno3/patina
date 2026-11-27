# Agent Instructions: `patina`

Patina is the owner's personal task system (identity × form model, project DAGs, habits, weekly review) built as a Rust workspace with a Slint + Material 3 GUI for Windows, Linux, macOS, and Android. This file may lag the code; where they disagree, trust the code and update this file.

## Project Overview

- `.cargo/config.toml`: Windows MSVC stack size for debug builds.
- `crates/patina-core/`: domain library: commands, queries, event log, projections. Pure; see invariants.
- `crates/patina-cli/`: `patina` binary; thin adapter; human-readable and JSON output.
- `crates/patina-mcp/`: `patina-mcp` binary; MCP server over stdio; only identities in `[mcp] visible_identities` are visible (`patina_core::visibility`). What a client may write depends on its level in `[mcp]` (`default_level`, `[mcp.clients]`): `read_only`, `propose` (capture and proposals the owner reviews), or `trusted` (direct commands, undone in bulk with `patina undo --source mcp:<client>`); see `patina_core::access`.
- `crates/patina-gui/`: Slint GUI; `lib` + `cdylib` (Android) + `patina-gui` bin; `build.rs` compiles `ui/` via `slint-project.json`.
  - Rust: `controller.rs` (commands, queries, refresh into the `App` global), `convert.rs` (pure view-model and form conversions), `wire.rs` (callbacks), `notify.rs` (OS notifications), `tests.rs` (headless end-to-end).
  - `ui/main.slint` (only `MainWindow`), `ui/types.slint` (data contract), `ui/app.slint` (the `App` global), `ui/labels.slint` (stable keys to `@tr` text), `ui/pages/`, `ui/components/`, `ui/icons/` + `ui/icons.slint`.
  - `translations/<lang>/LC_MESSAGES/patina-gui.po` (`zh_CN`, `zh_TW`; English is the source): bundled by `build.rs`, switched live from Settings (`src/language.rs`, preference in `gui.toml` next to `device-id`). After adding or changing an `@tr` string run `just tr-update`, translate the new entries; `just tr-check` (part of `just check`) must pass. Text the Rust code shows goes through `Labels` (`gui_error`, `break_due`), never hard-coded English; core errors stay English.
- `docs/design.md`: product design (Chinese), the source of truth for behavior.
- `docs/code-style.md`: code and Markdown style.
- `vendor/slint-material/`: unmodified Slint Material Components 1.1.0 (MIT). Never edit; upgrade with `just vendor-material <version>` and check its Slint minimum.
- `flake.nix`, `justfile`, `rust-toolchain.toml`: environment and tasks.
- `.github/`: CI (`ci.yml`: `just check` in Nix, tests on Windows and macOS, PR commit check), tag releases (`release.yml`, notes from `cliff.toml`), issue/PR templates, Dependabot.
- New front ends (MCP server, TUI) are new crates `crates/patina-<name>/` that depend on `patina-core` only.

## Design Doc

Read the relevant section of `docs/design.md` before changing domain behavior. It is about 400 lines; find sections with `grep -n '^#' docs/design.md` and read ranges.

- `## 实现约束` overrides earlier sections.
- Items marked `[建议]` are proposals the owner has not confirmed: follow them by default, and ask before building something expensive to reverse on top of one.
- Respect the phase table in `### 分阶段范围`: do not build later-phase features unless asked (phase 3 is built: habit graduation, identity-switch suggestions, DAG view, Syncthing-based multi-device merge rules and Android shared storage, MCP proposals and trusted writes).
- `## 开放问题` items are undecided; ask instead of picking.

## Architecture Invariants

- The append-only event log is the only source of truth; item lists, DAG, habit scores, and review data are projections.
- Every write goes through a `patina-core` command; adapters never write the log or re-implement validation (e.g. cycle detection).
- Commands and queries are serializable types; the same types are the CLI JSON, MCP, and future IPC protocol.
- Projections are pure functions of (events, `now`); never read the system clock in core. Time-triggered changes (scheduled-date rollover, follow-up due, habit day scored 0, auto "away") are derived, never stored as events.
- Projections must tolerate invalid logs from offline merges (cycles, two running timers): skip and report, never panic.
- Every event carries the full envelope from day one: UUIDv7 id, device id, effective time, recorded time, source (GUI/CLI/MCP client), command id, type version, category (domain vs observation).
- Old event versions are upgraded on read; the log file itself is never rewritten, except hard delete (tombstone).
- No UI-language text in the log: states, sizes, abandon reasons, and event types are stable keys; only user-entered text is stored verbatim.
- Storage: one append-only JSONL file per device; quick capture appends one line without replaying.
- No daemon in phases 0 and 1: each command starts, replays, executes, exits.
- When an agent runs `patina` against real data, set `PATINA_ACTOR=agent:<name>`; the CLI must treat that source as proposal-level.

## Build and Test Commands

- Nix (default): `nix develop` or direnv (`.envrc` = `use flake`). Android: `nix develop .#android` (x86_64-linux only; pulls the unfree Android SDK + NDK; license accepted in `flake.nix`).
- Without Nix (Windows, macOS): rustup picks up `rust-toolchain.toml`; install `just`; Windows needs Git Bash on `PATH` (recipes use bash); install the editor server with `cargo install slint-lsp --version <slint version> --locked`.

```sh
just                          # List recipes
just fmt                      # Format Rust, Slint, and Nix sources
just check                    # Everything that must pass before a commit
just cli --version            # Run the CLI
just gui                      # Run the desktop GUI
just gui-preview              # Run the GUI with Slint live preview (reloads .slint files on save)
just android-build            # Build a debug APK (needs `nix develop .#android`)
just android-run              # Build, install, and launch on the connected device
just icon <name>              # Fetch a Material Symbols Outlined icon
just vendor-material <version> # Replace vendor/slint-material with an upstream release
```

- Before finishing any change: `just check`. UI changes: run `just gui` (or `just gui-preview`) and look at the result. Changes touching Android code or metadata: `just android-build` in the Android shell. Flake changes: `nix flake check`.
- Live preview reloads `.slint` on save and keeps Rust-set state; rebuild after renaming a property or callback used from Rust. Never put `live-preview` into `Cargo.toml`.

## GUI: Slint and Material 3

- If the Slint agent plugin from `slint-ui/ai-plugins` is available, use it for `.slint` work; otherwise check APIs against the Slint 1.19 docs (https://docs.slint.dev) and https://material.slint.dev. Never guess properties.
- Follow the Slint and Material rules in [`docs/code-style.md#slint`](docs/code-style.md#slint).
- The root window inherits `MaterialWindow`. Use `std-widgets.slint` only for `AboutSlint`.
- `AboutSlint` must stay visible somewhere in the app (Slint Royalty-free License attribution); currently in the Settings page.
- The verdigris color scheme lives in `ui/theme.slint` (generated from seed `#43B3AE`; regenerate, never hand-edit) and is set once, in `MainWindow` `init`, through `MaterialPalette.schemes`. Layout breakpoints (`App.wide` at 600 px, `App.two_pane` at 1000 px) live in `ui/app.slint`.
- Popups (`Dialog`, `FullscreenDialog`, `SnackBar`) can only be driven through properties of their owning component, not through elements inside them; each dialog component is a zero-size anchor that sizes the popup to the window with `App.window_width/height`.
- Material `ListTile` loops inside scroll views; use `components/tile.slint`. `RadioButtonTile` loops inside dialogs; use `components/option_row.slint`.
- Icons: the Material library has no public icon set. Use only Material Symbols Outlined 24 px: `just icon <name>` downloads to `crates/patina-gui/ui/icons/<name>.svg`; register it as an `out property <image>` in `export global AppIcons` in `ui/icons.slint` (create the file with the first icon); render with Material `Icon`.
- Layouts must work from a 360 px wide phone in portrait to a desktop window; touch targets are at least 48 px.
- Android: entry point `android_main` in `src/lib.rs` (`cdylib`), min SDK 26, hover disabled through `MaterialWindowAdapter`. Keep Slint's default FemtoVG renderer; enabling `renderer-skia*` downloads prebuilt Skia that needs NDK r30 or newer (`FORCE_SKIA_BUILD=1` builds from source).

## Code Style

Read [`docs/code-style.md`](docs/code-style.md) before writing code or docs.

### Git Commits

Atomic commits, Conventional Commits. Types and scopes are listed in `CONTRIBUTING.md` (scopes `core`, `cli`, `gui`, `mcp`, `nix`, `build`, `docs`, `design`, `vendor`; CI changes use type `ci`), e.g. `feat(core): append events to the device log`, `build(nix): add android dev shell`. Commit `Cargo.lock` and `flake.lock`.

## Security and Privacy

- Never commit real event logs or personal data; fixtures are synthetic.
- Work-identity content may be confidential under the owner's internship employer: never send log contents to external services.
- Hard delete is the only operation allowed to rewrite history.
- No secrets or signing keys in the repo (Android release keystore included).

## Additional Notes

- Read files before writing; do not re-read unchanged ones.
- Do not guess APIs, versions, flags, or crate names; verify in code or docs.
- Think thoroughly, keep output concise; no emojis or em dashes.
