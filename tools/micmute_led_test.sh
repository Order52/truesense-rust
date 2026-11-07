#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Non-interactive F8 mic-mute LED test for ONE codec GPIO (0, 1 or 2). Watch the F8 key light; the label tells you which state is on.
#   sudo tools/micmute_led_test.sh 0 1   (one or more of 0 1 2)
# Sequence: 3 s dark, 8 s "GPIO data bit = 1", 3 s dark, 8 s "GPIO data bit = 0" (output direction), then the pin is restored to its previous state.
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
test_gpio() {
g=$1
[ "$g" -le 2 ] || { echo "GPIO must be 0, 1 or 2"; return; }
HW=""; [ $# -ge 1 ] || { echo "usage: $0 <0|1|2> [...]"; exit 1; }
for d in /dev/snd/hwC*D*; do n=${d##*/}; [ "$(cat /sys/class/sound/$n/vendor_id 2>/dev/null)" = "0x10ec0245" ] && HW=$d; done
[ -n "$HW" ] || { echo "ALC245 not found"; exit 1; }
v() { hda-verb "$HW" 0x01 "$@" 2>&1 | tail -1 | sed 's/.*value = //'; }
m0=$(v GET_GPIO_MASK 0); d0=$(v GET_GPIO_DIRECTION 0); x0=$(v GET_GPIO_DATA 0); b=$((1<<g)); bit=$(printf '0x%02x' $b)
echo "previous: mask=$m0 dir=$d0 data=$x0   testing GPIO$g ($bit)"
restore() { hda-verb "$HW" 0x01 SET_GPIO_DATA "$x0" >/dev/null; hda-verb "$HW" 0x01 SET_GPIO_DIRECTION "$d0" >/dev/null; hda-verb "$HW" 0x01 SET_GPIO_MASK "$m0" >/dev/null; echo "restored"; }
trap restore EXIT
hda-verb "$HW" 0x01 SET_GPIO_MASK $((m0 | b)) >/dev/null; hda-verb "$HW" 0x01 SET_GPIO_DIRECTION $((d0 | b)) >/dev/null
hda-verb "$HW" 0x01 SET_GPIO_DATA $((x0 & ~b)) >/dev/null; echo "--- 3 s: GPIO$g data=0 (baseline)"; sleep 3
hda-verb "$HW" 0x01 SET_GPIO_DATA $((x0 | b)) >/dev/null;  echo "=== NOW: GPIO$g data bit = 1 (8 s). Is the F8 light ON?"; sleep 8
hda-verb "$HW" 0x01 SET_GPIO_DATA $((x0 & ~b)) >/dev/null; echo "--- 3 s: GPIO$g data=0"; sleep 3
echo "=== done; if F8 lit during the '=== NOW' line, GPIO$g is active-high."
restore; trap - EXIT

}
for g in "$@"; do echo; echo "########## GPIO$g ##########"; test_gpio "$g"; done
