#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# truesense: collect the hardware data that needs root (read-only; writes nothing to the machine except files in OUT).
#   sudo tools/collect_hardware_data.sh [outdir]      (default: collected-hardware-data/, git-ignored; contains firmware tables - do not publish)
# Needs (Arch): sudo pacman -S --needed acpica alsa-tools msr-tools
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo"; exit 1; }
OUT=$(realpath -m "${1:-$(dirname "$0")/../collected-hardware-data}"); mkdir -p "$OUT"; cd "$OUT" || exit 1
have() { command -v "$1" >/dev/null || echo "MISSING: $1 (skipping its part)"; }
have acpidump; have iasl; have rdmsr

mkdir -p acpi && cd acpi || exit 1
if command -v acpidump >/dev/null; then
  acpidump -b >/dev/null 2>&1 && ls *.dat >/dev/null 2>&1 && echo "acpidump: $(ls *.dat | wc -l) tables"
  if command -v iasl >/dev/null; then iasl -e ssdt*.dat -d dsdt.dat >/dev/null 2>&1; for f in ssdt*.dat; do iasl -e dsdt.dat ssdt*.dat -d "$f" >/dev/null 2>&1; done; fi
  # find the Acer WMI methods
  grep -n -E "WMBH|WMAA|7A4DDFE7|79772EC5|61EF69EA|1EA0BC57|APGe|\"AcerGaming|BatteryControl" *.dsl > ../acpi_acer_wmi_grep.txt 2>/dev/null
  echo "acer-wmi hits: $(wc -l < ../acpi_acer_wmi_grep.txt)"
fi
cd ..

# WMI bus state, platform profile, acer sysfs
{ for d in /sys/bus/wmi/devices/*; do echo "== $d"; cat "$d/modalias" 2>/dev/null; readlink "$d/driver" 2>/dev/null; done
  echo; ls -R /sys/devices/platform/acer-wmi 2>/dev/null | head -80
  echo; cat /sys/firmware/acpi/platform_profile{,_choices} 2>&1
  echo; for n in /sys/class/hwmon/*/name; do echo "$n: $(cat "$n")"; done; } > wmi_sysfs_state.txt 2>&1

# Codec GPIO / mic-mute LED data (read only)
HW=""; for d in /dev/snd/hwC*D*; do n=${d##*/}; [ "$(cat /sys/class/sound/$n/vendor_id 2>/dev/null)" = "0x10ec0245" ] && HW=$d; done
if [ -n "$HW" ] && command -v hda-verb >/dev/null; then
  { echo "codec $HW"; for v in GET_GPIO_MASK GET_GPIO_DIRECTION GET_GPIO_DATA GET_GPIO_ENABLE_MASK GET_GPIO_UNSOLICITED_RSP_MASK; do echo -n "$v: "; hda-verb "$HW" 0x01 $v 0 2>&1 | tail -1; done
    for c in 0x07 0x10 0x45 0x46 0x47; do echo -n "COEF idx $c: "; hda-verb "$HW" 0x20 SET_COEF_INDEX $c >/dev/null 2>&1; hda-verb "$HW" 0x20 GET_PROC_COEF 0 2>&1 | tail -1; done; } > codec_gpio_readonly.txt 2>&1
fi
cat /sys/class/leds/*mic*/{brightness,trigger} 2>/dev/null > micmute_led_sysfs.txt; ls /sys/class/leds >> micmute_led_sysfs.txt

# CPU power / undervolt facts
{ echo "== RAPL"; for z in /sys/class/powercap/intel-rapl:*; do echo "$z: $(cat $z/name) pl1=$(cat $z/constraint_0_power_limit_uw 2>/dev/null) pl2=$(cat $z/constraint_1_power_limit_uw 2>/dev/null) enabled=$(cat $z/enabled 2>/dev/null)"; done
  echo "== MSR"; modprobe msr 2>&1; command -v rdmsr >/dev/null && { echo -n "MSR_PKG_POWER_LIMIT 0x610: "; rdmsr -p0 0x610; echo -n "MSR_PLATFORM_INFO 0xce: "; rdmsr -p0 0xce; echo -n "MSR_POWER_CTL 0x1fc: "; rdmsr -p0 0x1fc; echo -n "FLEX_RATIO 0x194: "; rdmsr -p0 0x194; }
  echo "== cpu"; lscpu | head -20; } > cpu_power_facts.txt 2>&1

# sound / firmware logs
dmesg | grep -iE "sof|hda|alc245|snd_|acer|wmi|linuwu" > dmesg_audio_acer.txt 2>&1
alsa-info.sh --stdout --no-upload > alsa-info.txt 2>&1
chown -R "${SUDO_USER:-root}": "$OUT"; echo "done -> $OUT"; ls -la "$OUT"
