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

Conventions for contributors and agents are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).

The GUI uses [Slint](https://slint.dev) under the Slint Royalty-free License; Material components are MIT.
