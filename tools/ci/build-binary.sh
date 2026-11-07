#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Build the release program the one way every pipeline builds it: one static program, the same bytes from the same commit on any machine and in any directory.
#   tools/ci/build-binary.sh [SRC_DIR [OUT_FILE]]      SRC_DIR defaults to this repository; OUT_FILE (if given) receives a copy of the program
# What makes it reproducible:
#   --remap-path-prefix   the source tree, CARGO_HOME and RUSTUP_HOME are renamed in debug info and panic messages, so the checkout path never reaches the binary
#   SOURCE_DATE_EPOCH     the build date compiled in (TRUESENSE_BUILD_DATE) comes from the commit time, never from the clock (default: the commit time of SRC_DIR's git HEAD)
#   --locked              Cargo.lock decides every dependency version; a stale lock is an error
#   a fixed toolchain     rust-toolchain.toml names `stable`: record `rustc -V` with the release (the SBOM and the release notes do) when you compare checksums
# Environment: TARGET (default x86_64-unknown-linux-musl: one static-pie program with no libc to install), CARGO_TARGET_DIR (default SRC_DIR/target),
#              TRUESENSE_COMMIT (default: git rev-parse --short=12 HEAD in SRC_DIR, if it is a git tree).
set -Eeuo pipefail
ROOT=$(realpath "$(dirname "$0")/../..")
SRC=$(realpath "${1:-$ROOT}"); OUT=${2:-}
TARGET=${TARGET:-x86_64-unknown-linux-musl}
[ -f "$SRC/Cargo.toml" ] || { echo "!! $SRC is not the TrueSense source tree (no Cargo.toml)" >&2; exit 1; }
cd "$SRC"

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then SOURCE_DATE_EPOCH=$(git log -1 --format=%ct HEAD 2>/dev/null || true); fi
[ -n "${SOURCE_DATE_EPOCH:-}" ] || { echo "!! SOURCE_DATE_EPOCH is not set and $SRC is not a git tree: a build date from the clock would make the program differ on every build" >&2; exit 1; }
export SOURCE_DATE_EPOCH TZ=UTC LC_ALL=C CARGO_INCREMENTAL=0
export TRUESENSE_BUILD_DATE; TRUESENSE_BUILD_DATE=$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y-%m-%d)
export TRUESENSE_COMMIT=${TRUESENSE_COMMIT:-$(git rev-parse --short=12 HEAD 2>/dev/null || true)}

cargo_home=${CARGO_HOME:-$HOME/.cargo}; rustup_home=${RUSTUP_HOME:-$HOME/.rustup}
# one flag per word, joined by the unit separator that CARGO_ENCODED_RUSTFLAGS uses (a path with a space in it stays one flag)
sep=$'\x1f'
flags="--remap-path-prefix=$SRC=/build/truesense${sep}--remap-path-prefix=$cargo_home=/build/cargo${sep}--remap-path-prefix=$rustup_home=/build/rustup"
case $TARGET in *-gnu) flags="$flags${sep}-C${sep}target-feature=+crt-static";; esac     # glibc needs asking for a static program; musl is static by default
export CARGO_ENCODED_RUSTFLAGS=$flags
unset RUSTFLAGS

cargo build --release --locked -p truesense --target "$TARGET"
bin="${CARGO_TARGET_DIR:-$SRC/target}/$TARGET/release/truesense"
[ -x "$bin" ] || { echo "!! cargo did not produce $bin" >&2; exit 1; }
if [ -n "$OUT" ]; then install -D -m755 "$bin" "$OUT"; fi
echo "$bin"
