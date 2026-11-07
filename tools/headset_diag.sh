#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Read-only ALC245 headset diagnostics. Run with a 4-pole headset plugged in:
#   sudo tools/headset_diag.sh
# Step 1 prints pin sense / pin config / COEFs; step 2 waits 12 s (UNPLUG and REPLUG the headset); step 3 prints again.
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
HW=""; for d in /dev/snd/hwC*D*; do n=${d##*/}; [ "$(cat /sys/class/sound/$n/vendor_id 2>/dev/null)" = "0x10ec0245" ] && HW=$d && HN=$n; done
[ -n "$HW" ] || { echo "ALC245 not found"; exit 1; }
OUT=$(dirname "$0")/../docs/hardware-data/headset_diag.txt; : > "$OUT"
v() { hda-verb "$HW" "$@" 2>&1 | tail -1 | sed 's/.*value = //'; }
snap() {
  echo "=== $1 $(date +%T)"
  for n in 0x19 0x21 0x1a 0x1b; do echo "NID $n pin_sense=$(v $n GET_PIN_SENSE 0) widget_ctl=$(v $n GET_PIN_WIDGET_CONTROL 0) config=$(v $n GET_CONFIG_DEFAULT 0)"; done
  for i in 0x07 0x10 0x1b 0x45 0x46 0x47 0x4a 0x63; do hda-verb "$HW" 0x20 SET_COEF_INDEX $i >/dev/null 2>&1; echo -n "COEF[$i]=$(v 0x20 GET_PROC_COEF 0) "; done; echo
  echo "driver_pin_configs: $(tr '\n' ' ' < /sys/class/sound/$HN/driver_pin_configs)"
  amixer -c1 contents | grep -A2 -E "Jack'|Mic Boost|Capture Switch" | grep -E "name|values" | tr -s ' ' | paste - - | sed 's/ ; type=[A-Z]*,access=[a-z-]*,values=[0-9]*//'
  pactl list sources 2>/dev/null | grep -E "Mic2|Headset" | head -4
}
{ snap "BEFORE (headset plugged in)"; echo ">>> UNPLUG and REPLUG the headset now (12 s)"; sleep 12; snap "AFTER replug"; } 2>&1 | tee -a "$OUT"
chown "${SUDO_USER:-root}": "$OUT"; echo "saved $OUT"
