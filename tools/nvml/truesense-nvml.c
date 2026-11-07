// SPDX-License-Identifier: GPL-3.0-only
/*
 * truesense-nvml: the one place TrueSense talks to NVIDIA's management library.
 *
 * The daemon is a static Rust program and cannot load the driver's library itself; it runs this
 * helper instead, with a timeout, so a call that hangs inside the NVIDIA driver can never block
 * the root service. The helper does exactly one thing per run for the GPU at index 0 and prints
 * one line: "ok [numbers]" or "err <what>". It always exits 0 once it has answered.
 *
 *   truesense-nvml offset              ok OFFSET_MHZ       current graphics clock curve offset
 *   truesense-nvml range               ok MIN_MHZ MAX_MHZ  offsets the driver allows
 *   truesense-nvml set-offset MHZ      ok                  shift the clock/voltage curve
 *   truesense-nvml lock MIN_MHZ MAX_MHZ ok                 limit the graphics clock
 *   truesense-nvml reset-clocks        ok                  lift the clock limit
 *
 * Errors: "err code N" (an NVML return code), "err library" (no NVIDIA driver library),
 * "err init", "err nodevice", "err usage".
 */
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef int (*fn_init)(void);
typedef int (*fn_handle)(unsigned int, void **);
typedef int (*fn_get_offset)(void *, int *);
typedef int (*fn_get_range)(void *, int *, int *);
typedef int (*fn_set_offset)(void *, int);
typedef int (*fn_lock)(void *, unsigned int, unsigned int);
typedef int (*fn_reset)(void *);
typedef int (*fn_shutdown)(void);

#define NVML_NOT_SUPPORTED 3

static int parse_long(const char *s, long lo, long hi, long *out)
{
	char *end;

	if (!s || !*s)
		return -1;
	errno = 0;
	*out = strtol(s, &end, 10);
	if (errno || *end || *out < lo || *out > hi)
		return -1;
	return 0;
}

static int code(int rc)
{
	if (rc == 0) {
		puts("ok");
		return 0;
	}
	printf("err code %d\n", rc);
	return 0;
}

int main(int argc, char **argv)
{
	void *lib, *dev = NULL;
	fn_init init;
	fn_handle handle;
	fn_shutdown shutdown;
	const char *cmd = argc > 1 ? argv[1] : "";
	long a = 0, b = 0;
	int rc, off, lo, hi;

	/* check the arguments before touching the driver */
	if (!strcmp(cmd, "offset") || !strcmp(cmd, "range") || !strcmp(cmd, "reset-clocks")) {
		if (argc != 2)
			goto usage;
	} else if (!strcmp(cmd, "set-offset")) {
		if (argc != 3 || parse_long(argv[2], -100000, 100000, &a))
			goto usage;
	} else if (!strcmp(cmd, "lock")) {
		if (argc != 4 || parse_long(argv[2], 0, 100000, &a) || parse_long(argv[3], 0, 100000, &b))
			goto usage;
	} else {
		goto usage;
	}

	lib = dlopen("libnvidia-ml.so.1", RTLD_NOW);
	if (!lib)
		lib = dlopen("libnvidia-ml.so", RTLD_NOW);
	if (!lib) {
		puts("err library");
		return 0;
	}
	init = (fn_init)dlsym(lib, "nvmlInit_v2");
	handle = (fn_handle)dlsym(lib, "nvmlDeviceGetHandleByIndex_v2");
	if (!init || !handle || init() != 0) {
		puts("err init");
		return 0;
	}
	if (handle(0, &dev) != 0) {
		puts("err nodevice");
		return 0;
	}

	if (!strcmp(cmd, "offset")) {
		fn_get_offset f = (fn_get_offset)dlsym(lib, "nvmlDeviceGetGpcClkVfOffset");

		if (!f)
			return code(NVML_NOT_SUPPORTED);
		rc = f(dev, &off);
		if (rc)
			code(rc);
		else
			printf("ok %d\n", off);
	} else if (!strcmp(cmd, "range")) {
		fn_get_range f = (fn_get_range)dlsym(lib, "nvmlDeviceGetGpcClkMinMaxVfOffset");

		if (!f)
			return code(NVML_NOT_SUPPORTED);
		rc = f(dev, &lo, &hi);
		if (rc)
			code(rc);
		else
			printf("ok %d %d\n", lo, hi);
	} else if (!strcmp(cmd, "set-offset")) {
		fn_set_offset f = (fn_set_offset)dlsym(lib, "nvmlDeviceSetGpcClkVfOffset");

		code(f ? f(dev, (int)a) : NVML_NOT_SUPPORTED);
	} else if (!strcmp(cmd, "lock")) {
		fn_lock f = (fn_lock)dlsym(lib, "nvmlDeviceSetGpuLockedClocks");

		code(f ? f(dev, (unsigned int)a, (unsigned int)b) : NVML_NOT_SUPPORTED);
	} else {
		fn_reset f = (fn_reset)dlsym(lib, "nvmlDeviceResetGpuLockedClocks");

		code(f ? f(dev) : NVML_NOT_SUPPORTED);
	}
	shutdown = (fn_shutdown)dlsym(lib, "nvmlShutdown");
	if (shutdown)
		shutdown();
	return 0;

usage:
	puts("err usage");
	return 0;
}
