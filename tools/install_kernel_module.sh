#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Build and install the truesense_wmi kernel module with DKMS for EVERY installed kernel (so it also works after you boot another kernel), then load it.
set -Eeuo pipefail
[ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
ROOT=$(realpath "$(dirname "$0")/.."); SRC="$ROOT/kernel/truesense-wmi"; N=truesense-wmi
V=$(sed -n 's/^PACKAGE_VERSION="\(.*\)"/\1/p' "$SRC/dkms.conf")
say() { printf '  %s\n' "$*"; }
echo "== kernel module $N $V"
command -v dkms >/dev/null || { echo "dkms is not installed"; exit 1; }
# idempotent upgrade: drop every registered version of this package first
for ver in $(dkms status "$N" 2>/dev/null | sed -E 's|^[^/]*/([^,: ]+).*|\1|' | sort -u); do dkms remove "$N/$ver" --all >/dev/null 2>&1 || true; done
rm -r -f /usr/src/"$N"-*; install -d "/usr/src/$N-$V"; install -m644 "$SRC"/truesense_wmi.c "$SRC"/Makefile "$SRC"/dkms.conf "/usr/src/$N-$V/"
dkms add "$N/$V" >/dev/null
running=$(uname -r); ok_running=0
for kdir in /lib/modules/*/build; do
  [ -e "$kdir" ] || continue; kver=$(basename "$(dirname "$kdir")")
  if dkms install "$N/$V" -k "$kver" --force >/tmp/truesense-dkms-$kver.log 2>&1; then say "built + installed for $kver"; [ "$kver" = "$running" ] && ok_running=1
  else say "FAILED for $kver (see /tmp/truesense-dkms-$kver.log)"; fi
done
[ "$ok_running" = 1 ] || { echo "the module could not be built for the running kernel $running (are the kernel headers installed?)"; exit 1; }
depmod -a
modprobe -r truesense_wmi 2>/dev/null || true
modprobe truesense_wmi
for _ in $(seq 1 50); do [ -e /dev/truesense ] && break; sleep 0.1; done
[ -e /dev/truesense ] || { echo "module loaded but /dev/truesense did not appear: this is not a PH16-71, or the firmware lacks the Acer WMI interface (dmesg | grep truesense)"; exit 1; }
say "loaded: $(modinfo -F version truesense_wmi) -> /dev/truesense"
