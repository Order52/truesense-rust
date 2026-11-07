#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Run the benchmark (tools/bench/bench.py) in a private world and print the table of docs/PERFORMANCE.md.
#
#   tools/bench/run.sh [--seconds N] [--runs N] [--bin PATH] [--qt PATH] [--only daemon,keys,...] [--json FILE]
#
# --bin defaults to $TRUESENSE_BIN or target/x86_64-unknown-linux-musl/release/truesense, --qt to
# $TRUESENSE_QT_BIN or target/release/truesense-qt when that exists. A full run takes about six
# minutes (60 s idle windows); `--seconds 15 --runs 8` is a quick look.
#
# Why a namespace: the daemon publishes /tmp/truesense-state, and the key service looks at /dev and
# /sys/class/hidraw. On a machine with a real TrueSense those belong to the real services, so the
# benchmark runs in a user and mount namespace (unshare, no privileges needed) with a private /tmp,
# a /dev that only keeps the harmless devices, a fake Fn-key interface backed by a pipe and a fake
# CPU sensor. Nothing
# outside the namespace is written. Without unshare the benchmark refuses to run.
set -Eeuo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
args=("$@")
bin=${TRUESENSE_BIN:-$root/target/x86_64-unknown-linux-musl/release/truesense}
qt=${TRUESENSE_QT_BIN:-}
[ -n "$qt" ] || { [ -x "$root/target/release/truesense-qt" ] && qt=$root/target/release/truesense-qt || true; }
[ -n "${CARGO_TARGET_DIR:-}" ] && {
  [ -x "$CARGO_TARGET_DIR/x86_64-unknown-linux-musl/release/truesense" ] && bin=${TRUESENSE_BIN:-$CARGO_TARGET_DIR/x86_64-unknown-linux-musl/release/truesense}
  [ -z "${TRUESENSE_QT_BIN:-}" ] && [ -x "$CARGO_TARGET_DIR/release/truesense-qt" ] && qt=$CARGO_TARGET_DIR/release/truesense-qt
}
command -v unshare >/dev/null || { echo "!! unshare is not installed; the benchmark needs it for its private /tmp" >&2; exit 1; }
export TRUESENSE_BIN=$bin TRUESENSE_QT_BIN=$qt TS_BENCH_PRIVATE=1

# Inside the namespace (the user is mapped to root there, with no power over the real machine).
inner() {
  mount -t tmpfs tmpfs /tmp
  # /dev: a fresh tmpfs with only the devices a program needs, plus a fake Fn-key node (a FIFO).
  mkdir /tmp/olddev && mount --rbind /dev /tmp/olddev
  mount -t tmpfs tmpfs /dev
  for n in null zero urandom random tty full; do : > "/dev/$n"; mount --bind "/tmp/olddev/$n" "/dev/$n"; done
  mkdir /dev/pts && mount -t devpts -o newinstance,ptmxmode=0666,mode=0620 devpts /dev/pts && ln -s pts/ptmx /dev/ptmx
  mkfifo /dev/hidraw9
  # /sys/class/hidraw: one fake interface that looks like the keyboard's Fn-key interface.
  mkdir /tmp/hidraw && mount -t tmpfs tmpfs /tmp/hidraw && mount --bind /tmp/hidraw /sys/class/hidraw
  mkdir -p /sys/class/hidraw/hidraw9/device
  printf 'HID_ID=0003:000004F2:00000117\n' > /sys/class/hidraw/hidraw9/device/uevent
  printf '\x06\x88\x00\x09\x01\xa1\x01\xc0' > /sys/class/hidraw/hidraw9/device/report_descriptor
  # /sys/class/hwmon: one fake coretemp with a package sensor, which bench.py moves every second so
  # that the daemon and the windows see changing temperatures, as on a real machine.
  mkdir /tmp/hwmon && mount -t tmpfs tmpfs /tmp/hwmon && mount --bind /tmp/hwmon /sys/class/hwmon
  mkdir /sys/class/hwmon/hwmon0
  printf 'coretemp\n' > /sys/class/hwmon/hwmon0/name
  printf 'Package id 0\n' > /sys/class/hwmon/hwmon0/temp1_label
  printf '55000\n' > /sys/class/hwmon/hwmon0/temp1_input
  exec python3 "$here/bench.py" "$@"
}
if [ "${1:-}" = "--inner" ]; then shift; inner "$@"; fi
exec unshare --user --map-root-user --mount "$0" --inner --bin "$bin" ${qt:+--qt "$qt"} "${args[@]}"
