#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Build the release bundle deterministically: truesense-X.Y.Z.tar.gz = `git archive` of the checked-out commit plus the prebuilt static program bin/truesense.
# The same commit always gives the same bytes (sorted names, fixed mtime, root owner, gzip -n), so anyone can rebuild the bundle and compare checksums.
#   tools/ci/build-bundle.sh X.Y.Z OUTDIR      writes OUTDIR/truesense-X.Y.Z.tar.gz and OUTDIR/truesense (the program on its own, for the SBOM and the smoke test)
# The program is built TWICE, from two fresh copies of the archived tree in two different directories with two empty target directories, and the script stops unless the two
# are byte-identical: that proves the result depends on the commit and on nothing else (not the checkout path, not a build cache, not the clock). tools/ci/build-binary.sh sets
# the flags (--remap-path-prefix, SOURCE_DATE_EPOCH, --locked). Kernel build leftovers that were committed by mistake (*.ko, *.o, ...) are left out of the bundle.
# Environment: SOURCE_DATE_EPOCH (default: the commit time), TARGET (default x86_64-unknown-linux-musl: the only platform the bundle supports).
set -Eeuo pipefail
VER=${1:?usage: build-bundle.sh X.Y.Z OUTDIR}; OUT=${2:?usage: build-bundle.sh X.Y.Z OUTDIR}
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "!! not a version: $VER" >&2; exit 1; }
ROOT=$(realpath "$(dirname "$0")/../..")
cd "$ROOT"
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct HEAD)}
export TZ=UTC TARGET=${TARGET:-x86_64-unknown-linux-musl}
export TRUESENSE_COMMIT; TRUESENSE_COMMIT=$(git rev-parse --short=12 HEAD)
mkdir -p "$OUT"; OUT=$(realpath "$OUT")

stage=$(mktemp -d); trap 'chmod -R u+w "$stage" 2>/dev/null; rm -r -f "$stage"' EXIT

# 1. the tree exactly as git archives it, unpacked twice in different places
for n in a b; do
  mkdir -p "$stage/$n/src"
  git archive --format=tar HEAD | tar -x -C "$stage/$n/src"
done

# 2. the program, built from each copy with an empty target directory (a build that reuses the other's cache would prove nothing)
for n in a b; do
  CARGO_TARGET_DIR="$stage/$n/target" "$ROOT/tools/ci/build-binary.sh" "$stage/$n/src" "$stage/$n/truesense" > /dev/null
done
if ! cmp -s "$stage/a/truesense" "$stage/b/truesense"; then
  echo "::error::two builds of the same commit differ: the build is not reproducible ($(sha256sum "$stage/a/truesense" | cut -c1-12) vs $(sha256sum "$stage/b/truesense" | cut -c1-12))" >&2; exit 1
fi
cp "$stage/a/truesense" "$OUT/truesense"

# 3. the bundle: the archived tree with bin/truesense added, repacked in a fixed order
pack="$stage/pack"; mkdir -p "$pack"
git archive --format=tar --prefix="truesense-$VER/" HEAD | tar -x -C "$pack"
find "$pack/truesense-$VER/kernel" \( -name '*.ko' -o -name '*.o' -o -name '*.mod' -o -name '*.mod.c' -o -name '.*.cmd' -o -name 'Module.symvers' -o -name 'modules.order' \) -type f -delete 2>/dev/null || true
install -D -m755 "$OUT/truesense" "$pack/truesense-$VER/bin/truesense"
( cd "$pack" && umask 022 && tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner --format=gnu -cf - "truesense-$VER" ) | gzip -n -9 > "$OUT/truesense-$VER.tar.gz"
sha256sum "$OUT/truesense" "$OUT/truesense-$VER.tar.gz"
