# Hardware interface notes (PH16-71)

Unofficial, independently observed facts about how this laptop's firmware and keyboard controller behave. They were found by reading public Linux drivers, probing the laptop's own interfaces, and checking every value on the machine. They are written down so the code can be reviewed and other people can verify them. Nothing here comes from, and no file here contains, vendor software. These interfaces are not documented by the vendor and may differ on other models or BIOS versions.

## Firmware (ACPI-WMI)

Three WMI interfaces are used, each with one instance (`/dev/truesense`, see `kernel/truesense-wmi/truesense_wmi.c`):

| Interface | GUID | Methods used |
|---|---|---|
| gaming | `7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56` | 1 set profile (also overdrive), 3 get profile, 5 system info and sensors, 6 set RGB, 14 / 15 set and get fan behaviour, 16 / 17 set and get fan speed, 20 / 21 set and get the rear light, 22 / 23 set and get miscellaneous settings |
| APGe | `61EF69EA-865C-4BC3-A502-A0DEBA0CB531` | 1 set, 2 get: USB charging while off (`0x4`), keyboard backlight timeout flag (`0x88401` / `0x88402`) |
| battery | `79772EC5-04B1-4BFD-843C-61E7F77B6CC9` | 20 get, 21 set: health (80 % limit) and calibration |

Encodings seen on the laptop:

- **Power mode** (misc setting index `0x0B`): 0 quiet, 1 balanced, 4 performance, 5 turbo, 6 eco; index `0x0A` is a mask of the supported modes.
- **GPU mode** (misc index 2): 1 Optimus, 2 NVIDIA only, 3 automatic; takes effect after a reboot.
- **Boot animation and sound**: misc index 6.
- **Fans**: behaviour `0x410009` auto, `0x820009` max, `0xC30009` manual for both; the manual speed argument is `((percent * 25600 / 100) & 0xFF00) + fan` with fan 1 = CPU and 4 = GPU; manual duty 20 to 100 % per fan, and 0 (stop that fan, either one or both) only through TrueSense's guarded path with a read-back of the duty register; whether the firmware really stops a single fan is UNTESTED.
- **Sensors**: `0x0101` CPU temperature, `0x0301` board, `0x0A01` GPU, `0x0201` / `0x0601` fan RPM (a 13-bit field, mask `0x1FFF`).
- **LCD overdrive**: read `0x1000001000000` on, `0x1000000` off; write `0x1000000000010` on, `0x10` off.
- **USB charging while off**: stop level 0 / 10 / 20 / 30 % as `663296/663300`, `659200/659204`, `1314560/1314564`, `1969920/1969924`.
- **Battery**: query 1; health is status byte 0 and calibration status byte 1.
- **Rear light**: a 16-byte buffer `mode, speed (1..5), brightness (0..100), 0, direction, R, G, B, 3, 2, 0...` with modes 0 static, 1 breathing, 2 neon, 3 wave, 7 twinkling. The static colour of each of the three parts is a separate call: `(zone << 40) | (8 << 32) | (B << 24) | (G << 16) | (R << 8)` with zone bits 1, 2, 4. A brightness of 0 turns the light off.

## Keyboard controller (USB HID `04F2:0117`)

- **Lighting interface**: usage page `0xFF02`, feature reports of 8 bytes; the last byte is `0xFF - (sum of the first seven) & 0xFF`.
- **Effect**: `B1 00 00 00 00 00 00 4E`, then colour `14 00 00 R G B 00 chk`, then `08 02 <effect> <speed byte> <brightness> 01 <direction + 1> chk` (speed byte 0 fastest .. 11 slowest; the keyboard reads it back as byte 3 of the `88` report and refuses 12 and above, storing 6 instead; TrueSense's scale is 12 - speed, 1 slowest .. 12 fastest). Effect ids: static 1, breathing 2, wave 3, snake 5, ripple 6, neon 8, raindrop 10, lightning 18, spot 37, star 38, fireball 39, snow 40, heartbeat 41; "operating mode" is `0x22`. Brightness is 0 to 50; speed 1 to 11.
- **Per-key colours**: feature `88 00 00 00 00 00 00 77` (then a GET), feature `12 00 00 08 00 00 00 E5`, then 8 output reports of 64 bytes (written as 65 bytes with a leading report id of 0, paced about 16 ms apart) carrying 128 cells of `00 R G B`, then `08 02 33 05 <brightness> 08 01 chk`. Cell number is `column * 6 + row`.
- **Brightness read-back**: send `88 ...` and read the feature report; byte 4 is the brightness.
- **Backlight timeout**: set `30 01 <0|1> 00 00 00 00 chk`; get by sending `B0 00 00 00 00 00 00 4F` and reading byte 2. The factory state is on.
- **Fn and PREDATOR keys**: usage page `0x88`, report id 4: `04 80 FF` Fn+F10, `04 8A FF` Fn+Esc, `04 81 FF` PREDATOR press, `04 00 00` release (also sent after the other Fn keys). Fn+F11 and Fn+F12 change brightness inside the keyboard.

## Audio (Realtek ALC245)

Speakers and headphones share one analogue stream, so they appear as two exclusive card profiles. The headset microphone is only classified by the codec at plug time. **Never write the codec's `reconfig` file** (it deadlocked the kernel on the test laptop).

## Measured behaviour

Fan duty to RPM, sensor ranges and similar measurements are in [PH16-71-NOTES.md](PH16-71-NOTES.md) and the evidence table [STATUS.md](STATUS.md).
