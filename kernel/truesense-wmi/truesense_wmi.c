// SPDX-License-Identifier: (MIT OR GPL-2.0-only)
/*
 * truesense_wmi - TrueSense kernel helper for the Acer Predator Helios 16 (PH16-71)
 *
 * Binds the three Acer gaming WMI interfaces and exposes /dev/truesense. Write ONE text command, read the
 * reply on the same file descriptor. Every open file has its own reply buffer, so several clients (daemon,
 * CLI, GUI) never see each other's answers; commands are serialised with one lock.
 *
 * This is NOT a generic "call any WMI method" interface: only the commands in ts_cmds[] exist and every
 * argument is validated. It binds only on the PH16-71 unless loaded with force=1.
 *
 * Interfaces (GUID, role):
 *   7A4DDFE7-...  gaming function: power mode, fans, sensors, GPU mode, boot sound, overdrive, rear light
 *   61EF69EA-...  APGeAction:      USB power-off charging level, keyboard backlight timeout
 *   79772EC5-...  battery control: 80 % charge limit, calibration
 * Method numbers and values come from observing PredatorSense on the same laptop; see docs/research.
 *
 * Commands (reply is "ok key=value ..." or "err message"):
 *   version | caps | sensors
 *   get_mode | get_modes | set_mode <0|1|4|5|6>        (0 quiet, 1 balanced, 4 performance, 5 turbo, 6 eco)
 *   get_fan | set_fan_auto | set_fan_max | set_fan_manual <cpu%> <gpu%>   (manual duty per fan 20..100, or 0 = that fan stopped)
 *   get_gpu | set_gpu <1|2|3>                          (1 optimus, 2 nvidia only, 3 automatic; needs reboot)
 *   get_boot | set_boot <0|1>                          (boot animation and sound)
 *   get_overdrive | set_overdrive <0|1>                (LCD overdrive)
 *   get_misc <1..16>                                   (read-only firmware settings)
 *   get_rear | get_slot | set_rear <32 hex> | set_color <RRGGBB> [zone 1|2|4]
 *   get_usb | set_usb <0|10|20|30>                     (power-off USB charging stop level, 0 = off)
 *   get_kbtimeout | set_kbtimeout <0|1>                (keyboard backlight off after 30 s)
 *   get_battery <1|2> | set_battery <1|2> <0|1>        (1 = 80 % limit, 2 = calibration)
 * On unbind/unload the fans are returned to firmware Auto.
 */
#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/acpi.h>
#include <linux/ctype.h>
#include <linux/dmi.h>
#include <linux/fs.h>
#include <linux/kernel.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <linux/version.h>
#include <linux/wmi.h>
#if __has_include(<linux/hex.h>)
#include <linux/hex.h>
#endif
#if __has_include(<linux/kstrtox.h>)
#include <linux/kstrtox.h>
#endif

#define TS_VERSION	"0.5.0"

#define GUID_GAME	"7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56"
#define GUID_APGE	"61EF69EA-865C-4BC3-A502-A0DEBA0CB531"
#define GUID_BATT	"79772EC5-04B1-4BFD-843C-61E7F77B6CC9"

enum { TS_GAME, TS_APGE, TS_BATT, TS_NDEV };

/* gaming-function methods */
#define M_SET_PROFILE	1	/* SetGamingProfile, u64 (overdrive) */
#define M_GET_PROFILE	3	/* GetGamingProfile, u64 */
#define M_GET_SYSINFO	5	/* GetGamingSysInfo, u64 */
#define M_SET_RGB	6	/* SetGamingRgbKb, u64 */
#define M_FAN_BEHAVIOR	14	/* SetGamingFanBehavior, u64 */
#define M_GET_FANBEH	15
#define M_FAN_SPEED	16	/* SetGamingFanSpeed, u64 */
#define M_GET_FANSPEED	17
#define M_SET_KB	20	/* SetGamingKBBacklight (rear light), u8[16] */
#define M_GET_KB	21	/* GetGamingKBBacklight, u32 */
#define M_SET_MISC	22	/* SetGamingMiscSetting, u64 */
#define M_GET_MISC	23	/* GetGamingMiscSetting, u64 */
/* APGeAction methods */
#define M_APGE_SET	1
#define M_APGE_GET	2
/* battery-control methods */
#define M_BATT_GET	20
#define M_BATT_SET	21

#define MISC_GPU	2
#define MISC_BOOT	6
#define MISC_MODES	0x0A
#define MISC_MODE	0x0B

#define FAN_AUTO	0x410009ULL
#define FAN_MAX		0x820009ULL
#define FAN_MANUAL_BOTH	0xC30009ULL

static bool force;
module_param(force, bool, 0444);
MODULE_PARM_DESC(force, "bind on models other than the Acer Predator PH16-71 (unsupported; can misbehave)");

static const struct dmi_system_id ts_dmi_table[] = {
	{ .ident = "Acer Predator PH16-71",
	  .matches = { DMI_MATCH(DMI_SYS_VENDOR, "Acer"), DMI_MATCH(DMI_PRODUCT_NAME, "Predator PH16-71") } },
	{ }
};
MODULE_DEVICE_TABLE(dmi, ts_dmi_table);

static struct wmi_device *ts_wdev[TS_NDEV];
static const int ts_dev_idx[TS_NDEV] = { TS_GAME, TS_APGE, TS_BATT };
static DEFINE_MUTEX(ts_lock);		/* serialises commands and (un)binding */
static bool ts_registered;

struct ts_file {
	struct mutex lk;
	size_t len;
	char buf[256];
};

/* ---- WMI plumbing ------------------------------------------------------------------------------------------------------------ */

static int ts_eval(int dev, u32 method, const void *in, size_t in_len, union acpi_object **out)
{
	struct acpi_buffer inb = { in_len, (void *)in };
	struct acpi_buffer outb = { ACPI_ALLOCATE_BUFFER, NULL };
	acpi_status st;

	lockdep_assert_held(&ts_lock);
	if (!ts_wdev[dev])
		return -ENODEV;
	st = wmidev_evaluate_method(ts_wdev[dev], 0, method, &inb, &outb);
	if (ACPI_FAILURE(st))
		return -EIO;
	*out = outb.pointer;
	return 0;
}

/* integer or little-endian buffer (4/8 bytes) to u64 */
static u64 ts_obj_u64(const union acpi_object *obj)
{
	u64 v = 0;

	if (!obj)
		return 0;
	if (obj->type == ACPI_TYPE_INTEGER)
		return obj->integer.value;
	if (obj->type == ACPI_TYPE_BUFFER && obj->buffer.length)
		memcpy(&v, obj->buffer.pointer, min_t(size_t, obj->buffer.length, sizeof(v)));
	return le64_to_cpu(v);
}

static int ts_call64(int dev, u32 method, u64 in, u64 *res)
{
	__le64 le = cpu_to_le64(in);
	union acpi_object *obj = NULL;
	int ret = ts_eval(dev, method, &le, sizeof(le), &obj);

	if (ret)
		return ret;
	if (res)
		*res = ts_obj_u64(obj);
	kfree(obj);
	return 0;
}

static int ts_misc_get(u8 idx, u8 *val, u8 *status)
{
	u64 r;
	int ret = ts_call64(TS_GAME, M_GET_MISC, idx, &r);

	if (ret)
		return ret;
	*status = r & 0xff;
	*val = (r >> 8) & 0xff;
	return 0;
}

static int ts_misc_set(u8 idx, u8 val)
{
	u64 r;
	int ret = ts_call64(TS_GAME, M_SET_MISC, ((u64)val << 8) | idx, &r);

	if (ret)
		return ret;
	return (r & 0xff) ? -EIO : 0;
}

/* ---- replies ------------------------------------------------------------------------------------------------------------------ */

static __printf(2, 3) void ts_reply(struct ts_file *f, const char *fmt, ...)
{
	va_list ap;
	int n;

	va_start(ap, fmt);
	n = vsnprintf(f->buf, sizeof(f->buf), fmt, ap);
	va_end(ap);
	f->len = min_t(size_t, n < 0 ? 0 : n, sizeof(f->buf) - 1);
}

static int ts_err(struct ts_file *f, int err, const char *what)
{
	ts_reply(f, "err %s (%d)\n", what, err);
	return err;
}

/* ---- commands: each returns 0 or -errno; the reply is already set on success ---------------------------------------------------- */

static int c_version(struct ts_file *f, char *a) { ts_reply(f, "ok version=" TS_VERSION "\n"); return 0; }

static int c_caps(struct ts_file *f, char *a)
{
	ts_reply(f, "ok game=%d apge=%d battery=%d\n", !!ts_wdev[TS_GAME], !!ts_wdev[TS_APGE], !!ts_wdev[TS_BATT]);
	return 0;
}

static int c_sensors(struct ts_file *f, char *a)
{
	static const u16 ids[5] = { 0x0101, 0x0301, 0x0A01, 0x0201, 0x0601 };	/* cpu, system, gpu temp; cpu, gpu fan */
	u64 r[5];
	int i, ret;

	for (i = 0; i < 5; i++) {
		ret = ts_call64(TS_GAME, M_GET_SYSINFO, ids[i], &r[i]);
		if (ret)
			return ts_err(f, ret, "sensor read failed");
	}
	ts_reply(f, "ok cpu=%u sys=%u gpu=%u cpu_rpm=%u gpu_rpm=%u\n", (u32)((r[0] >> 8) & 0xff), (u32)((r[1] >> 8) & 0xff),
		 (u32)((r[2] >> 8) & 0xff), (u32)((r[3] >> 8) & 0x1fff), (u32)((r[4] >> 8) & 0x1fff));
	return 0;
}

static int c_get_mode(struct ts_file *f, char *a)
{
	u8 v, st;
	int ret = ts_misc_get(MISC_MODE, &v, &st);

	if (ret || st)
		return ts_err(f, ret ?: -EIO, "get_mode failed");
	ts_reply(f, "ok mode=%u\n", v);
	return 0;
}

static int c_get_modes(struct ts_file *f, char *a)
{
	u8 v, st;
	int ret = ts_misc_get(MISC_MODES, &v, &st);

	if (ret || st)
		return ts_err(f, ret ?: -EIO, "get_modes failed");
	ts_reply(f, "ok mask=0x%02x\n", v);
	return 0;
}

static int c_set_mode(struct ts_file *f, char *a)
{
	u8 m;
	int ret;

	if (!a || kstrtou8(a, 10, &m) || !(m == 0 || m == 1 || m == 4 || m == 5 || m == 6))
		return -EINVAL;
	ret = ts_misc_set(MISC_MODE, m);
	if (ret)
		return ts_err(f, ret, "set_mode failed");
	ts_reply(f, "ok mode=%u\n", m);
	return 0;
}

static int c_get_gpu(struct ts_file *f, char *a)
{
	u8 v, st;
	int ret = ts_misc_get(MISC_GPU, &v, &st);

	if (ret || st)
		return ts_err(f, ret ?: -EIO, "get_gpu failed");
	ts_reply(f, "ok gpu=%u\n", v);
	return 0;
}

static int c_set_gpu(struct ts_file *f, char *a)
{
	u8 m;
	int ret;

	if (!a || kstrtou8(a, 10, &m) || m < 1 || m > 3)
		return -EINVAL;
	ret = ts_misc_set(MISC_GPU, m);
	if (ret)
		return ts_err(f, ret, "set_gpu failed");
	ts_reply(f, "ok gpu=%u\n", m);
	return 0;
}

static int c_get_boot(struct ts_file *f, char *a)
{
	u8 v, st;
	int ret = ts_misc_get(MISC_BOOT, &v, &st);

	if (ret || st)
		return ts_err(f, ret ?: -EIO, "get_boot failed");
	ts_reply(f, "ok boot=%u\n", v ? 1 : 0);
	return 0;
}

static int c_set_boot(struct ts_file *f, char *a)
{
	u8 m;
	int ret;

	if (!a || kstrtou8(a, 10, &m) || m > 1)
		return -EINVAL;
	ret = ts_misc_set(MISC_BOOT, m);
	if (ret)
		return ts_err(f, ret, "set_boot failed");
	ts_reply(f, "ok boot=%u\n", m);
	return 0;
}

static int c_get_misc(struct ts_file *f, char *a)
{
	u8 i, v, st;
	int ret;

	if (!a || kstrtou8(a, 10, &i) || i < 1 || i > 16)
		return -EINVAL;
	ret = ts_misc_get(i, &v, &st);
	if (ret)
		return ts_err(f, ret, "get_misc failed");
	ts_reply(f, "ok idx=%u status=%u value=%u\n", i, st, v);
	return 0;
}

static int c_get_overdrive(struct ts_file *f, char *a)
{
	u64 r;
	int ret = ts_call64(TS_GAME, M_GET_PROFILE, 0, &r);

	if (ret)
		return ts_err(f, ret, "get_overdrive failed");
	ts_reply(f, "ok overdrive=%d raw=0x%llx\n", r == 0x1000001000000ULL ? 1 : r == 0x1000000ULL ? 0 : -1, r);
	return 0;
}

static int c_set_overdrive(struct ts_file *f, char *a)
{
	u8 m;
	int ret;

	if (!a || kstrtou8(a, 10, &m) || m > 1)
		return -EINVAL;
	ret = ts_call64(TS_GAME, M_SET_PROFILE, m ? 0x1000000000010ULL : 0x10ULL, NULL);
	if (ret)
		return ts_err(f, ret, "set_overdrive failed");
	ts_reply(f, "ok overdrive=%u\n", m);
	return 0;
}

static u64 fan_speed_arg(unsigned int pct, unsigned int fan)	/* fan: CPU 1, GPU 4 */
{
	return (((u64)pct * 25600 / 100) & 0xFF00) + fan;
}

static int c_get_fan(struct ts_file *f, char *a)
{
	u64 beh, c, g;
	int ret = ts_call64(TS_GAME, M_GET_FANBEH, 0x09, &beh);

	if (!ret)
		ret = ts_call64(TS_GAME, M_GET_FANSPEED, 1, &c);
	if (!ret)
		ret = ts_call64(TS_GAME, M_GET_FANSPEED, 4, &g);
	if (ret)
		return ts_err(f, ret, "get_fan failed");
	ts_reply(f, "ok behavior=0x%x cpu=%u gpu=%u\n", (u32)((beh >> 8) & 0xFFFFFF), (u32)((c >> 8) & 0xff), (u32)((g >> 8) & 0xff));
	return 0;
}

static int c_set_fan_auto(struct ts_file *f, char *a)
{
	int ret = ts_call64(TS_GAME, M_FAN_BEHAVIOR, FAN_AUTO, NULL);

	if (ret)
		return ts_err(f, ret, "set_fan_auto failed");
	ts_reply(f, "ok fan=auto\n");
	return 0;
}

static int c_set_fan_max(struct ts_file *f, char *a)
{
	int ret = ts_call64(TS_GAME, M_FAN_BEHAVIOR, FAN_MAX, NULL);

	if (ret)
		return ts_err(f, ret, "set_fan_max failed");
	ts_reply(f, "ok fan=max\n");
	return 0;
}

static int c_set_fan_manual(struct ts_file *f, char *a)
{
	unsigned int cpu, gpu;
	int ret;

	/* per fan: 20..100, or 0 = that fan stopped (the daemon guards this with a temperature watchdog and returns the fans to Auto when it gets warm) */
	if (!a || sscanf(a, "%u %u", &cpu, &gpu) != 2)
		return -EINVAL;
	if ((cpu && (cpu < 20 || cpu > 100)) || (gpu && (gpu < 20 || gpu > 100)))
		return -EINVAL;
	ret = ts_call64(TS_GAME, M_FAN_BEHAVIOR, FAN_MANUAL_BOTH, NULL);
	if (!ret)
		ret = ts_call64(TS_GAME, M_FAN_SPEED, fan_speed_arg(cpu, 1), NULL);
	if (!ret)
		ret = ts_call64(TS_GAME, M_FAN_SPEED, fan_speed_arg(gpu, 4), NULL);
	if (ret) {
		ts_call64(TS_GAME, M_FAN_BEHAVIOR, FAN_AUTO, NULL);	/* never leave a half-set state */
		return ts_err(f, ret, "set_fan_manual failed, fans returned to auto");
	}
	ts_reply(f, "ok fan=manual cpu=%u gpu=%u\n", cpu, gpu);
	return 0;
}

static int ts_hex_reply(struct ts_file *f, const union acpi_object *obj)
{
	u32 n = 0;

	if (!obj || obj->type != ACPI_TYPE_BUFFER)
		return -EIO;
	n = min_t(u32, obj->buffer.length, 40);
	ts_reply(f, "ok raw=%*phN\n", (int)n, obj->buffer.pointer);
	return 0;
}

static int c_get_rear_common(struct ts_file *f, u32 which)
{
	__le32 le = cpu_to_le32(which);
	union acpi_object *obj = NULL;
	int ret = ts_eval(TS_GAME, M_GET_KB, &le, sizeof(le), &obj);

	if (ret)
		return ts_err(f, ret, "rear read failed");
	ret = ts_hex_reply(f, obj);
	kfree(obj);
	return ret ? ts_err(f, ret, "unexpected rear reply") : 0;
}

static int c_get_rear(struct ts_file *f, char *a) { return c_get_rear_common(f, 2); }
static int c_get_slot(struct ts_file *f, char *a) { return c_get_rear_common(f, 1); }

static int c_set_rear(struct ts_file *f, char *a)
{
	u8 b[16];
	union acpi_object *obj = NULL;
	int ret;

	if (!a || strlen(a) != 32 || hex2bin(b, a, 16))
		return -EINVAL;
	/* modes: 0 static, 1 breathing, 2 neon, 3 wave, 7 twinkling; speed 1..5; brightness 0..100; byte 8/9 fixed */
	if (!(b[0] <= 3 || b[0] == 7) || b[1] < 1 || b[1] > 5 || b[2] > 100 || b[3] || b[4] > 1 || b[8] != 0x03 || b[9] != 0x02 ||
	    memchr_inv(&b[10], 0, 6))
		return -EINVAL;
	ret = ts_eval(TS_GAME, M_SET_KB, b, sizeof(b), &obj);
	if (ret)
		return ts_err(f, ret, "set_rear failed");
	kfree(obj);
	ts_reply(f, "ok\n");
	return 0;
}

static int c_set_color(struct ts_file *f, char *a)
{
	u8 c[3];
	unsigned int zone = 1;
	int ret;

	/* "RRGGBB" or "RRGGBB Z": Z = 1, 2 or 4 picks one of the three light-bar zones (default 1) */
	if (!a || strlen(a) < 6 || hex2bin(c, a, 3))
		return -EINVAL;
	if (a[6]) {
		if (a[6] != ' ' || kstrtouint(a + 7, 10, &zone))
			return -EINVAL;
		if (zone != 1 && zone != 2 && zone != 4)
			return -EINVAL;
	}
	/* (zone bit << 40) | (rear light bar index 8 << 32) | B << 24 | G << 16 | R << 8 */
	ret = ts_call64(TS_GAME, M_SET_RGB, ((u64)zone << 40) | (8ULL << 32) | ((u64)c[2] << 24) | ((u64)c[1] << 16) | ((u64)c[0] << 8), NULL);
	if (ret)
		return ts_err(f, ret, "set_color failed");
	ts_reply(f, "ok\n");
	return 0;
}

static int c_get_usb(struct ts_file *f, char *a)
{
	u64 r;
	int ret = ts_call64(TS_APGE, M_APGE_GET, 0x4, &r);

	if (ret)
		return ts_err(f, ret, "get_usb failed");
	ts_reply(f, "ok usb=%d raw=%llu\n", r == 663296 ? 0 : r == 659200 ? 10 : r == 1314560 ? 20 : r == 1969920 ? 30 : -1, r);
	return 0;
}

static int c_set_usb(struct ts_file *f, char *a)
{
	u8 v;
	u64 code;
	int ret;

	if (!a || kstrtou8(a, 10, &v) || !(v == 0 || v == 10 || v == 20 || v == 30))
		return -EINVAL;
	code = v == 0 ? 663300 : v == 10 ? 659204 : v == 20 ? 1314564 : 1969924;
	ret = ts_call64(TS_APGE, M_APGE_SET, code, NULL);
	if (ret)
		return ts_err(f, ret, "set_usb failed");
	ts_reply(f, "ok usb=%u\n", v);
	return 0;
}

static int c_get_kbtimeout(struct ts_file *f, char *a)
{
	u64 r;
	int ret = ts_call64(TS_APGE, M_APGE_GET, 0x88401, &r);

	if (ret)
		return ts_err(f, ret, "get_kbtimeout failed");
	ts_reply(f, "ok kbtimeout=%d raw=0x%llx\n", r == 0x1E0000080000ULL ? 1 : r == 0x80000ULL ? 0 : -1, r);
	return 0;
}

static int c_set_kbtimeout(struct ts_file *f, char *a)
{
	u8 v;
	int ret;

	if (!a || kstrtou8(a, 10, &v) || v > 1)
		return -EINVAL;
	ret = ts_call64(TS_APGE, M_APGE_SET, v ? 0x1E0000088402ULL : 0x88402ULL, NULL);
	if (ret)
		return ts_err(f, ret, "set_kbtimeout failed");
	ts_reply(f, "ok kbtimeout=%u\n", v);
	return 0;
}

static int c_get_battery(struct ts_file *f, char *a)
{
	static const u8 in[4] = { 0x01, 0x01, 0x00, 0x00 };	/* battery 1, query, reserved */
	union acpi_object *obj = NULL;
	u8 which;
	int ret;

	if (!a || kstrtou8(a, 10, &which) || which < 1 || which > 2)
		return -EINVAL;
	ret = ts_eval(TS_BATT, M_BATT_GET, in, sizeof(in), &obj);
	if (ret)
		return ts_err(f, ret, "get_battery failed");
	if (!obj || obj->type != ACPI_TYPE_BUFFER || obj->buffer.length != 8) {
		kfree(obj);
		return ts_err(f, -EIO, "unexpected battery reply");
	}
	/* layout: [0] function list, [1..2] return, [3..7] function status (3 = 80 % limit, 4 = calibration) */
	ts_reply(f, "ok battery=%u list=%u\n", ((u8 *)obj->buffer.pointer)[2 + which] ? 1 : 0, ((u8 *)obj->buffer.pointer)[0]);
	kfree(obj);
	return 0;
}

static int c_set_battery(struct ts_file *f, char *a)
{
	u8 in[8] = { 0x01 };
	union acpi_object *obj = NULL;
	unsigned int which, val;
	int ret;

	if (!a || sscanf(a, "%u %u", &which, &val) != 2 || which < 1 || which > 2 || val > 1)
		return -EINVAL;
	in[1] = which;
	in[2] = val;
	ret = ts_eval(TS_BATT, M_BATT_SET, in, sizeof(in), &obj);
	if (ret)
		return ts_err(f, ret, "set_battery failed");
	if (!obj || obj->type != ACPI_TYPE_BUFFER || obj->buffer.length < 1 || ((u8 *)obj->buffer.pointer)[0]) {
		kfree(obj);
		return ts_err(f, -EIO, "firmware refused the battery setting");
	}
	kfree(obj);
	ts_reply(f, "ok battery=%u value=%u\n", which, val);
	return 0;
}

static const struct ts_cmd {
	const char *name;
	bool has_arg;
	int (*fn)(struct ts_file *f, char *arg);
} ts_cmds[] = {
	{ "version", false, c_version },		{ "caps", false, c_caps },
	{ "sensors", false, c_sensors },		{ "get_mode", false, c_get_mode },
	{ "get_modes", false, c_get_modes },		{ "set_mode", true, c_set_mode },
	{ "get_gpu", false, c_get_gpu },		{ "set_gpu", true, c_set_gpu },
	{ "get_boot", false, c_get_boot },		{ "set_boot", true, c_set_boot },
	{ "get_misc", true, c_get_misc },		{ "get_overdrive", false, c_get_overdrive },
	{ "set_overdrive", true, c_set_overdrive },	{ "get_fan", false, c_get_fan },
	{ "set_fan_auto", false, c_set_fan_auto },	{ "set_fan_max", false, c_set_fan_max },
	{ "set_fan_manual", true, c_set_fan_manual },	{ "get_rear", false, c_get_rear },
	{ "get_slot", false, c_get_slot },		{ "set_rear", true, c_set_rear },
	{ "set_color", true, c_set_color },		{ "get_usb", false, c_get_usb },
	{ "set_usb", true, c_set_usb },			{ "get_kbtimeout", false, c_get_kbtimeout },
	{ "set_kbtimeout", true, c_set_kbtimeout },	{ "get_battery", true, c_get_battery },
	{ "set_battery", true, c_set_battery },
};

/* ---- /dev/truesense ------------------------------------------------------------------------------------------------------------- */

static int ts_open(struct inode *inode, struct file *file)
{
	struct ts_file *f = kzalloc(sizeof(*f), GFP_KERNEL);

	if (!f)
		return -ENOMEM;
	mutex_init(&f->lk);
	ts_reply(f, "err no command yet\n");
	file->private_data = f;
	return stream_open(inode, file);
}

static int ts_release(struct inode *inode, struct file *file)
{
	kfree(file->private_data);
	return 0;
}

static ssize_t ts_write(struct file *file, const char __user *ubuf, size_t len, loff_t *off)
{
	struct ts_file *f = file->private_data;
	char buf[96], *cmd, *arg;
	const struct ts_cmd *c = NULL;
	int ret = -EINVAL, i;

	if (!len || len >= sizeof(buf))
		return -EINVAL;
	if (copy_from_user(buf, ubuf, len))
		return -EFAULT;
	buf[len] = '\0';
	cmd = strim(buf);
	arg = strchr(cmd, ' ');
	if (arg) {
		*arg++ = '\0';
		arg = skip_spaces(arg);
	}
	for (i = 0; i < ARRAY_SIZE(ts_cmds); i++)
		if (!strcmp(cmd, ts_cmds[i].name)) {
			c = &ts_cmds[i];
			break;
		}

	mutex_lock(&f->lk);
	mutex_lock(&ts_lock);
	if (!c)
		ts_reply(f, "err unknown command\n");
	else if (c->has_arg && !arg)
		ts_reply(f, "err missing argument\n");
	else if (!c->has_arg && arg && *arg)
		ts_reply(f, "err unexpected argument\n");
	else
		ret = c->fn(f, arg);
	if (ret == -EINVAL)
		ts_reply(f, "err invalid argument\n");
	mutex_unlock(&ts_lock);
	mutex_unlock(&f->lk);

	return ret ? ret : len;
}

static ssize_t ts_read(struct file *file, char __user *ubuf, size_t len, loff_t *off)
{
	struct ts_file *f = file->private_data;
	size_t n;

	mutex_lock(&f->lk);
	n = min(len, f->len);
	if (n && copy_to_user(ubuf, f->buf, n)) {
		mutex_unlock(&f->lk);
		return -EFAULT;
	}
	f->len = 0;			/* one reply per command; a second read returns EOF */
	mutex_unlock(&f->lk);
	return n;
}

static const struct file_operations ts_fops = {
	.owner = THIS_MODULE,
	.open = ts_open,
	.release = ts_release,
	.read = ts_read,
	.write = ts_write,
};

static struct miscdevice ts_misc = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "truesense",
	.fops = &ts_fops,
	.mode = 0660,
};

/* ---- WMI driver ----------------------------------------------------------------------------------------------------------------- */

static int ts_probe(struct wmi_device *wdev, const void *context)
{
	int idx = *(const int *)context, ret = 0;

	if (!force && !dmi_check_system(ts_dmi_table)) {
		dev_warn_once(&wdev->dev, "not an Acer Predator PH16-71: not binding (load with force=1 at your own risk)\n");
		return -ENODEV;
	}
	mutex_lock(&ts_lock);
	ts_wdev[idx] = wdev;
	dev_set_drvdata(&wdev->dev, (void *)&ts_dev_idx[idx]);
	if (idx == TS_GAME && !ts_registered) {		/* the gaming interface is the mandatory one */
		ret = misc_register(&ts_misc);
		if (ret)
			ts_wdev[idx] = NULL;
		else
			ts_registered = true;
	}
	mutex_unlock(&ts_lock);
	if (!ret)
		dev_info(&wdev->dev, "truesense " TS_VERSION " bound (interface %d)\n", idx);
	return ret;
}

static void ts_remove(struct wmi_device *wdev)
{
	int idx = *(const int *)dev_get_drvdata(&wdev->dev);

	mutex_lock(&ts_lock);
	if (idx == TS_GAME) {
		ts_call64(TS_GAME, M_FAN_BEHAVIOR, FAN_AUTO, NULL);	/* fans back to firmware control */
		if (ts_registered) {
			misc_deregister(&ts_misc);
			ts_registered = false;
		}
	}
	ts_wdev[idx] = NULL;
	mutex_unlock(&ts_lock);
}

static const struct wmi_device_id ts_ids[] = {
	{ GUID_GAME, &ts_dev_idx[TS_GAME] },
	{ GUID_APGE, &ts_dev_idx[TS_APGE] },
	{ GUID_BATT, &ts_dev_idx[TS_BATT] },
	{ }
};
MODULE_DEVICE_TABLE(wmi, ts_ids);

static struct wmi_driver ts_driver = {
	.driver = { .name = "truesense-wmi" },
	.id_table = ts_ids,
	.probe = ts_probe,
	.remove = ts_remove,
};
module_wmi_driver(ts_driver);

MODULE_AUTHOR("TrueSense contributors");
MODULE_DESCRIPTION("TrueSense: whitelisted Acer gaming WMI interface for the Predator Helios 16 (PH16-71)");
MODULE_VERSION(TS_VERSION);
MODULE_LICENSE("Dual MIT/GPL");
