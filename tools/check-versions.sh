#!/usr/bin/env bash
#
# check-versions.sh -- enforce the RTICX version generation.
#
# All crates of the root workspace must share the same major.minor pair
# ("generation"); micro (patch) versions are per-crate. See COMPATIBILITY.md.
#
# The riscv/rp2040 distributions are not root-workspace members (they live in
# their own mini-workspaces and lock distro+macro together via
# `version.workspace = true`), so they are not covered by this check.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

metadata="$(cargo metadata --no-deps --format-version 1 \
    --manifest-path "${ROOT_DIR}/Cargo.toml")"

version_table="$(printf '%s' "${metadata}" | python3 -c '
import json, sys
d = json.load(sys.stdin)
for p in d["packages"]:
    print(p["name"] + " " + p["version"])
')"

# major.minor ("generation") of every member, grouped by generation.
gens="$(printf '%s\n' "${version_table}" | awk '{ split($2, v, "."); print v[1] "." v[2] "  " $1 }' | sort)"

unique="$(printf '%s\n' "${gens}" | awk '{print $1}' | sort -u)"

if [[ "$(printf '%s\n' "${unique}" | wc -l)" -ne 1 ]]; then
    echo "ERROR: root-workspace crates do not share a version generation (major.minor):"
    printf '%s\n' "${gens}"
    echo ""
    echo "Breaking (minor) changes must bump ALL crates together; patch versions"
    echo "may differ per crate. See COMPATIBILITY.md."
    exit 1
fi

echo "OK: all root-workspace crates share generation $(printf '%s\n' "${unique}" | head -n1)"
