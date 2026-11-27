<!-- PRs need an issue the owner has agreed to; see CONTRIBUTING.md. -->

Closes #

## What and why

## How it was checked

- [ ] `just check` passes
- [ ] UI change: ran `just gui` and looked at the result (screenshots below, synthetic data only)
- [ ] Android change: `just android-build` in `nix develop .#android`
- [ ] Flake change: `nix flake check`
- [ ] New or changed `@tr` strings: ran `just tr-update` and translated `zh_CN` and `zh_TW`

## Checklist

- [ ] Commits follow the Conventional Commits rules in CONTRIBUTING.md
- [ ] Behavior matches docs/design.md, or this PR updates it
- [ ] No real event logs or personal data in code, fixtures, or screenshots
