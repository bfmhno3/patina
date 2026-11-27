#!/usr/bin/env bash
# Fail if any non-merge commit subject in a revision range breaks the
# Conventional Commits rules in CONTRIBUTING.md.
# Usage: bash .github/scripts/check-commits.sh <base>..<head>
set -euo pipefail

range="${1:?usage: check-commits.sh <base>..<head>}"
types='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'
scopes='core|cli|gui|mcp|nix|build|docs|design|vendor'
pattern="^(${types})(\((${scopes})\))?!?: [^ ].*$"

status=0
while IFS= read -r line; do
    sha="${line%% *}"
    subject="${line#* }"
    if [[ ! "$subject" =~ $pattern && ! "$subject" =~ ^Revert\ \" ]]; then
        echo "::error::${sha:0:7} \"${subject}\" does not follow Conventional Commits (see CONTRIBUTING.md)"
        status=1
    fi
done < <(git log --no-merges --format='%H %s' "$range")
exit "$status"
