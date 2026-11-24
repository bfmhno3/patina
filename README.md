# Patina

An identity-driven, event-sourced personal task system with a Slint + Material 3 GUI for Windows, Linux, macOS, and Android.

Design: [docs/design.md](docs/design.md) (Chinese)

## Development

```sh
nix develop          # dev shell (or use direnv)
just                 # list tasks
just check           # format check, clippy, tests
just gui             # run the desktop GUI
nix develop .#android  # Android shell, then: just android-run
```

## Front ends

- `patina`: the CLI (`just cli`), including `review`, `rhythm`, `break`, and `identity target`.
- `patina-gui`: the desktop and Android GUI (`just gui`).
- `patina-mcp`: a read-only MCP server over stdio; set `[mcp] visible_identities` in `config.toml` (default `["life", "study"]`) to choose what it may show.

## CLI data

`patina` stores events in `<data root>/events/<device>.jsonl` (override the root with `PATINA_DIR`) and optional settings in `<data root>/config.toml`. The device id lives outside the data root where the platform allows it (`PATINA_DEVICE_ID` overrides). To sync between machines, sync only `events/` and `config.toml`, never the device id file.

Conventions for contributors and agents are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).

The GUI uses [Slint](https://slint.dev) under the Slint Royalty-free License; Material components are MIT.
