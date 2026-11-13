// scap X Y W H OUT.ppm [damage]: one wlr-screencopy capture of a region of the first output
// (tools/qtprofile/stale.py builds it on first use). With "damage" it asks for copy_with_damage, so
// the compositor is not asked to repaint the whole output for it (whether it does anyway is up to
// the compositor). A plain copy (and grim) may get a freshly repainted output.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>
#include "sc.h"
static struct wl_shm *shm; static struct wl_output *output; static struct zwlr_screencopy_manager_v1 *mgr;
static uint32_t fmt, bw, bh, bstride; static int got_buffer, done, failed, buffer_done;
static void reg_global(void *d, struct wl_registry *r, uint32_t name, const char *iface, uint32_t v) {
  if (!strcmp(iface, wl_shm_interface.name)) shm = wl_registry_bind(r, name, &wl_shm_interface, 1);
  else if (!strcmp(iface, wl_output_interface.name) && !output) output = wl_registry_bind(r, name, &wl_output_interface, 1);
  else if (!strcmp(iface, zwlr_screencopy_manager_v1_interface.name)) mgr = wl_registry_bind(r, name, &zwlr_screencopy_manager_v1_interface, v < 3 ? v : 3);
}
static void reg_remove(void *d, struct wl_registry *r, uint32_t n) {}
static const struct wl_registry_listener reg_l = { reg_global, reg_remove };
static void f_buffer(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t format, uint32_t w, uint32_t h, uint32_t s) { fmt = format; bw = w; bh = h; bstride = s; got_buffer = 1; }
static void f_flags(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t fl) {}
static void f_ready(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t a, uint32_t b, uint32_t c) { done = 1; }
static void f_failed(void *d, struct zwlr_screencopy_frame_v1 *f) { failed = 1; }
static void f_damage(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t x, uint32_t y, uint32_t w, uint32_t h) { fprintf(stderr, "damage %u,%u %ux%u\n", x, y, w, h); }
static void f_dmabuf(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t a, uint32_t b, uint32_t c) {}
static void f_bdone(void *d, struct zwlr_screencopy_frame_v1 *f) { buffer_done = 1; }
static const struct zwlr_screencopy_frame_v1_listener fl = { f_buffer, f_flags, f_ready, f_failed, f_damage, f_dmabuf, f_bdone };
int main(int argc, char **argv) {
  if (argc < 6) return 2;
  int x = atoi(argv[1]), y = atoi(argv[2]), w = atoi(argv[3]), h = atoi(argv[4]); int with_damage = argc > 6 && !strcmp(argv[6], "damage");
  struct wl_display *dpy = wl_display_connect(NULL); if (!dpy) return 3;
  struct wl_registry *reg = wl_display_get_registry(dpy); wl_registry_add_listener(reg, &reg_l, NULL); wl_display_roundtrip(dpy);
  if (!shm || !output || !mgr) { fprintf(stderr, "missing globals\n"); return 4; }
  struct zwlr_screencopy_frame_v1 *fr = zwlr_screencopy_manager_v1_capture_output_region(mgr, 0, output, x, y, w, h);
  zwlr_screencopy_frame_v1_add_listener(fr, &fl, NULL);
  while (!buffer_done && !failed) if (wl_display_dispatch(dpy) < 0) return 5;
  if (failed || !got_buffer) { fprintf(stderr, "failed before buffer\n"); return 6; }
  size_t size = (size_t)bstride * bh; int fd = memfd_create("scap", 0); ftruncate(fd, size);
  void *data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, size);
  struct wl_buffer *buf = wl_shm_pool_create_buffer(pool, 0, bw, bh, bstride, fmt);
  if (with_damage) zwlr_screencopy_frame_v1_copy_with_damage(fr, buf); else zwlr_screencopy_frame_v1_copy(fr, buf);
  while (!done && !failed) if (wl_display_dispatch(dpy) < 0) return 7;
  if (failed) { fprintf(stderr, "copy failed\n"); return 8; }
  FILE *o = fopen(argv[5], "wb"); fprintf(o, "P6\n%u %u\n255\n", bw, bh);
  for (uint32_t r = 0; r < bh; r++) { unsigned char *row = (unsigned char *)data + (size_t)r * bstride;
    for (uint32_t c = 0; c < bw; c++) { unsigned char *p = row + c * 4; unsigned char rgb[3] = { p[2], p[1], p[0] }; fwrite(rgb, 1, 3, o); } }
  fclose(o); fprintf(stderr, "fmt 0x%x %ux%u\n", fmt, bw, bh); return 0;
}
