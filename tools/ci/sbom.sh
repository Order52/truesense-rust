#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# A CycloneDX 1.5 SBOM (JSON) of the program that ships: the Rust crates compiled into bin/truesense, with versions, licences, source hashes and package URLs.
#   tools/ci/sbom.sh X.Y.Z OUT.json
# Why a script and not the cargo-cyclonedx tool: it needs nothing installed beyond cargo and jq, so it cannot drift, cannot be a supply-chain step of its own and works offline.
# What it lists: `cargo tree -e normal` for the musl target (the crates the program is built from, proc-macros included, tests and build tooling excluded), joined with `cargo metadata`
# (licence, repository) and Cargo.lock (the SHA-256 of each crates.io source package). The output is deterministic: sorted, no random serial number, the timestamp is SOURCE_DATE_EPOCH.
# Environment: TARGET (default x86_64-unknown-linux-musl), SOURCE_DATE_EPOCH (default: the commit time).
set -Eeuo pipefail
VER=${1:?usage: sbom.sh X.Y.Z OUT.json}; OUT=${2:?usage: sbom.sh X.Y.Z OUT.json}
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "!! not a version: $VER" >&2; exit 1; }
ROOT=$(realpath "$(dirname "$0")/../..")
TARGET=${TARGET:-x86_64-unknown-linux-musl}
cd "$ROOT"
sde=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct HEAD)}
stamp=$(date -u -d "@$sde" +%Y-%m-%dT%H:%M:%SZ)

want=$(cargo tree --locked -p truesense --target "$TARGET" -e normal --prefix none --format '{p}' | sed -E 's/^([^ ]+) v([^ ]+).*/\1 \2/' | sort -u)
sums=$(awk '/^\[\[package\]\]/ { n = ""; v = "" } /^name = "/ { n = $3; gsub(/"/, "", n) } /^version = "/ { v = $3; gsub(/"/, "", v) } /^checksum = "/ { c = $3; gsub(/"/, "", c); print n " " v " " c }' Cargo.lock)

cargo metadata --locked --format-version 1 --filter-platform "$TARGET" | jq --arg ver "$VER" --arg stamp "$stamp" --arg want "$want" --arg sums "$sums" '
  ($want | split("\n") | map(select(length > 0)) | map({key: ., value: true}) | from_entries) as $in
  | ($sums | split("\n") | map(select(length > 0) | split(" ") | {key: (.[0] + " " + .[1]), value: .[2]}) | from_entries) as $sha
  | def spdx: gsub("\\s*/\\s*"; " OR ");            # old crates write "MIT/Apache-2.0"; CycloneDX wants an SPDX expression
  def component: {
      type: "library",
      "bom-ref": (if .source then "pkg:cargo/\(.name)@\(.version)" else "workspace:\(.name)@\(.version)" end),
      name: .name, version: .version,
      purl: (if .source then "pkg:cargo/\(.name)@\(.version)" else null end),
      licenses: (if .license then [{expression: (.license | spdx)}] else [] end),
      hashes: (($sha[.name + " " + .version]) as $h | if $h then [{alg: "SHA-256", content: $h}] else [] end),
      externalReferences: (if .repository then [{type: "vcs", url: .repository}] else [] end)
    } | with_entries(select(.value != null));
  {
    bomFormat: "CycloneDX", specVersion: "1.5", version: 1,
    metadata: {
      timestamp: $stamp,
      tools: [{vendor: "TrueSense", name: "tools/ci/sbom.sh"}],
      component: {type: "application", "bom-ref": "workspace:truesense@\($ver)", name: "truesense", version: $ver, licenses: [{license: {id: "GPL-3.0-only"}}]}
    },
    components: ([.packages[] | select($in[.name + " " + .version]) | select(.name != "truesense") | component] | sort_by(.name, .version))
  }' > "$OUT.tmp"
jq -e '.bomFormat == "CycloneDX" and (.components | length) > 0' "$OUT.tmp" > /dev/null || { rm -f "$OUT.tmp"; echo "!! the SBOM is empty or not CycloneDX" >&2; exit 1; }
mv "$OUT.tmp" "$OUT"
echo "SBOM: $(jq '.components | length' "$OUT") components -> $OUT"
