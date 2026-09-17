#!/usr/bin/env bash
# vendor/mailparse must be the published mailparse crate plus exactly upstream.patch.
#
# The copy is a permanent carry (#217): upstream declined the change, so every
# mailparse release is a hand-merge, and PATCH.md's four-step sync recipe is the
# moment either half of the delta is most likely to be lost or half-applied.
#
# Nothing else would catch that. The vendored suite still passes on unpatched
# upstream -- it tests behaviour, and the patch does not change behaviour -- so
# the only downstream signal would be the benchmark gate reading a 4-10x
# regression with no explanation, on a PR that has nothing to do with it.
#
# Offline: set MAILPARSE_CRATE_FILE to a local .crate to skip the download.
set -euo pipefail

# Read out of the [package] section with awk rather than a TOML parser: this
# script has to run on whatever python a contributor's machine has, and tomllib
# is 3.11+.
VER=$(awk '/^\[package\]/{p=1;next} /^\[/{p=0} p&&/^version = /{gsub(/[",]/,"",$3);print $3;exit}' vendor/mailparse/Cargo.toml)
if [ -z "$VER" ]; then
  echo "::error::could not read the version from vendor/mailparse/Cargo.toml"
  exit 1
fi
# Bump together with the version. From crates.io; cross-check the index `cksum`.
EXPECT_SHA256="60819a97ddcb831a5614eb3b0174f3620e793e97e09195a395bfa948fd68ed2f"

# Registry/tarball metadata the vendored copy deliberately dropped, plus our own
# two documentation files, which have no upstream counterpart by design.
DROPPED=(PATCH.md upstream.patch Cargo.toml.orig .cargo_vcs_info.json Cargo.lock .cargo-ok)

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

crate="${MAILPARSE_CRATE_FILE:-$tmp/mailparse-$VER.crate}"
if [ -z "${MAILPARSE_CRATE_FILE:-}" ]; then
  curl -sSfL "https://static.crates.io/crates/mailparse/mailparse-$VER.crate" -o "$crate"
fi
# `sha256sum` on Linux, `shasum -a 256` on macOS -- so this is runnable where
# the patch gets regenerated, not only in CI.
if command -v sha256sum >/dev/null; then
  echo "$EXPECT_SHA256  $crate" | sha256sum -c -
else
  echo "$EXPECT_SHA256  $crate" | shasum -a 256 -c -
fi

tar xzf "$crate" -C "$tmp"
upstream="$tmp/mailparse-$VER"

patch -p1 --fuzz=0 --no-backup-if-mismatch -d "$upstream" < vendor/mailparse/upstream.patch

# Compare like with like: our copy without the files upstream never had, the
# tarball without the files we deliberately dropped.
cp -R vendor/mailparse "$tmp/ours"
for f in "${DROPPED[@]}"; do rm -f "$upstream/$f" "$tmp/ours/$f"; done
rm -rf "$tmp/ours/target"

if ! diff -r "$upstream" "$tmp/ours"; then
  echo "::error::vendor/mailparse is not mailparse $VER + upstream.patch."
  echo "::error::Regenerate the patch (see its header) and update PATCH.md to match."
  exit 1
fi

# `[patch.crates-io]` only applies while the patched version satisfies the
# requirement, so a Dependabot bump of either root manifest alone would silently
# switch the build back to the registry crate. Only the benchmark gate would
# notice, and only as an unexplained regression.
for manifest in Cargo.toml fuzz/Cargo.toml; do
  if ! grep -qE "^mailparse = \"$VER\"" "$manifest"; then
    echo "::error::$manifest does not require mailparse \"$VER\"; [patch.crates-io] would stop applying"
    exit 1
  fi
done

# A lock entry with a `source =` line is the signature of the patch NOT being in
# effect: a patched path dependency has no source.
if awk '/^name = "mailparse"$/{f=1;next} f&&/^source =/{exit 1} f&&/^\[\[package\]\]/{exit 0}' Cargo.lock; then
  echo "vendor/mailparse is mailparse $VER + upstream.patch, and the patch is in effect."
else
  echo "::error::Cargo.lock records a source for mailparse -- the vendored copy is NOT being used"
  exit 1
fi
