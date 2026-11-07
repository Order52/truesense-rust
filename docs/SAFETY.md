# Safety

TrueSense changes how a real laptop runs: fans, power, voltages, battery. This page lists what protects you, what can still go wrong, and how to recover.

## What protects you

| Area | Safeguard |
|---|---|
| Every firmware setting | read the value, write, **read it back**; a mismatch is an error, never silently accepted |
| Fans | return to the firmware's own control on service stop, crash or exit; the module hands them back when it unloads; the service restarts itself (systemd watchdog) |
| Other power managers | only a fixed list (auto-cpufreq, tlp, tuned, tuned-ppd, power-profiles-daemon, system76-power) is stopped, only when you switch *TrueSense manages power* on; what was stopped and the CPU settings before are written down first, and `power managers restore` undoes it |
| Manual fans | duty 20 to 100 % per fan, or 0 to stop that fan (both together or only one): needs confirmation, only below 60 °C CPU / 55 °C GPU, and the service gives ALL fans back to the firmware at 70 °C CPU / 65 °C GPU. After a stop the duty register is read back; if the firmware did not take it the fans go back to Auto. On the tested PH16-71 the firmware takes the 0 % command but keeps the fan at its minimum speed (about 1700 rpm), so this does not actually silence a fan |
| Fans **off** (0 %) | needs a typed-in confirmation; refused unless the CPU is under 60 °C and the GPU under 55 °C; the service gives the fans back to the firmware at 70 °C CPU or 65 °C GPU, or if the device stops answering. **Not yet tried on real hardware.** |
| Smart fan | the curve must reach 100 % by 95 °C (CPU) and 85 °C (GPU) and may only rise with temperature; hot (92 °C CPU / 82 °C GPU) means full speed; fan changes only rise at once and fall slowly |
| CPU power limits | 25 to 120 W; above 80 W sustained or 100 W burst is "past the safety line", above 100 W / 115 W needs an explicit force |
| CPU undervolt | 0 to -120 mV only; past -60 mV core / -50 mV cache is "past the safety line"; beyond -90 / -70 you must type `APPLY` (window) or pass `--force` (CLI); needs `--confirm` or a dialog every time; a reboot always returns to stock; an offset that does not read back is rolled back |
| GPU undervolt | curve offset -300 to +250 MHz, cap 0 or 600 to 3300 MHz; past +150 MHz is "past the safety line", beyond +200 MHz needs typed confirmation or `--force`; a reboot returns to stock |
| Undervolt at boot | opt-in; if the laptop does not stay up for 10 minutes after applying it, the next boot skips it and turns the option off |
| Battery | only the firmware's own 80 % limit is switched; "charge to 100 % once" can only start on AC with the limit on and always brings the limit back (unplug, 5 minutes full, or 24 hours) |
| GPU mode | firmware setting that applies after a reboot; confirmed first; pick the old mode again before rebooting to cancel |
| Firmware | no flashing, no boot-logo writes, no embedded-controller access outside documented WMI calls; the kernel module whitelists every command and validates every value |
| Updates | checking sends one request for the public release number; installing downloads only a **version tag** of the configured repository over HTTPS, refuses unsafe archive paths, confirms the version inside matches the tag, and then runs that release's installer as root **only when you press Update** (password prompt). The repository is taken from the root-owned system config, never from your user config. It is the same trust as `curl | sudo bash`: releases are not signed yet, so update only from a repository you trust |
| Access | the service socket checks the caller's user id (root and the configured user only); the service runs with systemd sandboxing |

## What can still go wrong

- **An undervolt can freeze or reboot the laptop.** The numbers shipped are conservative starting points, not validated on your unit. Test under load before trusting a profile; unsaved work can be lost.
- **Fans off or a very low manual speed with a heavy load can overheat the laptop.** The watchdog helps but is software.
- **A kernel update can break the module** until DKMS rebuilds it (the health timer does this; `truesense doctor` tells you).
- **Other models.** Nothing here is tested on other hardware.

## Recovery

| Problem | Do this |
|---|---|
| Fans stuck or loud | `truesense fan auto` (or stop the service: `sudo systemctl stop truesense`; the fans return to the firmware) |
| Freeze or reboot after an undervolt | it is already gone after the reboot; if "apply at boot" was on it switches itself off. Or `truesense cpu undervolt reset` / `truesense gpu undervolt reset` |
| Power limits feel wrong | `truesense cpu limits --auto` |
| Wrong lighting | Lighting, Profiles, Reset to default (the `native` profile) or `truesense look apply native` |
| Everything | Settings, Reset all, or `truesense profile apply` of a known-good profile; `sudo tools/uninstall.sh` removes TrueSense completely (its modprobe block on the old drivers too) |
| The codec | **never write `/sys/class/sound/hwC*D*/reconfig`**; on the test laptop it deadlocked the kernel and needed a hard reboot |

## Reporting a safety problem

See [SECURITY.md](../SECURITY.md).
