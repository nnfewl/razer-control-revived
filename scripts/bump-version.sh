#!/usr/bin/env bash
# Bump the release version in every place it must match.
# CI's check-version job fails the release if these ever drift.
#
# Usage: scripts/bump-version.sh 0.3.0-rc7
set -euo pipefail

NEW="${1:?usage: scripts/bump-version.sh <version, e.g. 0.3.0-rc7>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# 1. Cargo.toml (package version — first match only)
sed -i "0,/^version = /s/^version = \".*\"/version = \"$NEW\"/" \
    "$ROOT/razer_control_gui/Cargo.toml"

# 2. Release workflow artifact version
sed -i "s/^  APP_VERSION: \".*\"/  APP_VERSION: \"$NEW\"/" \
    "$ROOT/.github/workflows/release.yml"

# 3. RPM spec (rpm uses ~ for pre-release ordering)
sed -i "s/^Version:.*/Version:        ${NEW/-rc/~rc}/" \
    "$ROOT/packaging/fedora/razercontrol.spec"

# 4. Sync Cargo.lock
(cd "$ROOT/razer_control_gui" && cargo check -q)

echo
git -C "$ROOT" diff --stat
echo
echo "Version bumped to $NEW. Release with:"
echo "  git commit -am 'chore: bump version to $NEW'"
echo "  git tag v$NEW"
echo "  git push origin main v$NEW"
