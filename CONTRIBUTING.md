# Contributing to Patina

Patina is the owner's personal task system. The owner decides scope and design. [`docs/design.md`](docs/design.md) (Chinese) is the source of truth for behavior, [`AGENTS.md`](AGENTS.md) holds the architecture invariants, and [`docs/code-style.md`](docs/code-style.md) holds the code style.

## Issues

- Use the bug or feature form; blank issues are off.
- Write in English or Chinese.
- One problem per issue; search existing issues first.
- Never paste real event logs, configs, or personal data. Give a synthetic, minimal reproduction.
- Feature requests outside the phase table of the design, or on its "明确不做" list, are likely declined.

## Pull requests

- Open or comment on an issue first and wait until the owner agrees in the issue. PRs without an agreed issue are closed, except typo and broken-link fixes.
- One topic per PR, atomic commits.
- Rebase on `main`; no merge commits. PRs are merged with "Rebase and merge", so every commit lands on `main` as written.
- All CI jobs must pass.
- Fill in the PR template.

## Development

Run `nix develop` (or use direnv), then `just check`. Without Nix, see "Build and Test Commands" in [`AGENTS.md`](AGENTS.md). By change type:

- `just check` passes
- UI change: ran `just gui` and looked at the result
- Android change: `just android-build` in `nix develop .#android`
- Flake change: `nix flake check`
- New or changed `@tr` strings: ran `just tr-update` and translated `zh_CN` and `zh_TW`

## Commit messages

Format: `<type>(<scope>): <subject>`. The scope is optional; put `!` before `:` for breaking changes. The subject is imperative, lower-case, and has no trailing period.

- Types: `feat fix docs style refactor perf test build ci chore revert`
- Scopes: `core cli gui mcp` (crates), `nix build docs design vendor`

Examples: `feat(core): append events to the device log`, `build(nix): add android dev shell`, `ci: cache the Nix store`.

CI checks every PR commit with `.github/scripts/check-commits.sh`. Run it locally with `bash .github/scripts/check-commits.sh origin/main..HEAD`.

## CI

The `CI` workflow runs on every push to `main` and every PR:

- `just check` and `nix flake check` in the Nix shell on Linux
- `cargo test --workspace` on Windows and macOS
- the commit check on PRs

## Releasing (owner)

One-time Android signing setup:

1. Run `keytool -genkeypair -v -keystore patina-release.keystore -alias patina -keyalg RSA -keysize 4096 -validity 10000`. The keystore is PKCS12, so the key password equals the store password.
2. Keep the keystore outside the repo and back it up; losing it means users must uninstall to upgrade.
3. Add repository secrets `ANDROID_KEYSTORE_BASE64` (`base64 -w0 patina-release.keystore`) and `ANDROID_KEYSTORE_PASSWORD`.

Per release:

1. Set `version` in root `Cargo.toml` `[workspace.package]`.
2. Run `cargo check --workspace` to refresh `Cargo.lock`.
3. Commit `chore: release vX.Y.Z`.
4. `git tag -a vX.Y.Z -m vX.Y.Z`
5. `git push origin main vX.Y.Z`

The `Release` workflow verifies the tag matches `Cargo.toml`, reruns CI, builds all assets, and publishes the GitHub release with git-cliff notes. A pre-release suffix (`0.2.0-rc.1`, tag `v0.2.0-rc.1`) publishes as a pre-release.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as MIT OR Apache-2.0, without any additional terms or conditions.
