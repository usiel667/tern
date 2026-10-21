// A virtual mouse for UI tests in the nested test display (dev/test-display.sh).
// Usage: vpointer WIDTH HEIGHT CMD...   (WAYLAND_DISPLAY = the test display;
// normally run via `dev/test-display.sh pointer CMD...`, which builds it and passes the size)
//   move X Y | glide X Y | down | up | click | sleep MS | drag X1 Y1 X2 Y2
// Coordinates are logical pixels of the (single) output, WIDTH x HEIGHT.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"

#define BTN_LEFT 0x110

static struct zwlr_virtual_pointer_manager_v1 *manager;
static struct wl_seat *seat;
static struct wl_display *display;
static struct zwlr_virtual_pointer_v1 *ptr;
static unsigned W, H;
static int cx, cy;

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t ver) {
    if (!strcmp(iface, zwlr_virtual_pointer_manager_v1_interface.name))
        manager = wl_registry_bind(reg, name, &zwlr_virtual_pointer_manager_v1_interface, 1);
    else if (!strcmp(iface, wl_seat_interface.name) && !seat)
        seat = wl_registry_bind(reg, name, &wl_seat_interface, 1);
}
static void global_remove(void *d, struct wl_registry *r, uint32_t n) {}
static const struct wl_registry_listener reg_listener = {global, global_remove};

static uint32_t now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}
static void flush(void) { wl_display_roundtrip(display); }
static void move(int x, int y) {
    zwlr_virtual_pointer_v1_motion_absolute(ptr, now(), x, y, W, H);
    zwlr_virtual_pointer_v1_frame(ptr);
    flush();
    cx = x; cy = y;
}
static void button(int pressed) {
    zwlr_virtual_pointer_v1_button(ptr, now(), BTN_LEFT, pressed);
    zwlr_virtual_pointer_v1_frame(ptr);
    flush();
}
// Move in small steps so toolkits see a real drag (past their threshold) and enter/leave.
static void glide(int x, int y) {
    int x0 = cx, y0 = cy, steps = 30;
    for (int i = 1; i <= steps; i++) {
        move(x0 + (x - x0) * i / steps, y0 + (y - y0) * i / steps);
        usleep(16000);
    }
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: vpointer W H cmd...\n"); return 2; }
    W = atoi(argv[1]); H = atoi(argv[2]);
    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "no wayland display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(display);
    wl_registry_add_listener(reg, &reg_listener, NULL);
    wl_display_roundtrip(display);
    if (!manager) { fprintf(stderr, "compositor lacks zwlr_virtual_pointer_manager_v1\n"); return 1; }
    ptr = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, seat);
    flush();
    for (int i = 3; i < argc; i++) {
        const char *c = argv[i];
        if (!strcmp(c, "move") && i + 2 < argc) { move(atoi(argv[i + 1]), atoi(argv[i + 2])); i += 2; }
        else if (!strcmp(c, "glide") && i + 2 < argc) { glide(atoi(argv[i + 1]), atoi(argv[i + 2])); i += 2; }
        else if (!strcmp(c, "down")) button(1);
        else if (!strcmp(c, "up")) button(0);
        else if (!strcmp(c, "click")) { button(1); usleep(50000); button(0); }
        else if (!strcmp(c, "sleep") && i + 1 < argc) { usleep(atoi(argv[++i]) * 1000); }
        else if (!strcmp(c, "drag") && i + 4 < argc) {
            move(atoi(argv[i + 1]), atoi(argv[i + 2]));
            usleep(100000);
            button(1);
            usleep(100000);
            glide(atoi(argv[i + 3]), atoi(argv[i + 4]));
            usleep(300000);
            button(0);
            i += 4;
        } else { fprintf(stderr, "bad command: %s\n", c); return 2; }
    }
    usleep(100000);
    zwlr_virtual_pointer_v1_destroy(ptr);
    flush();
    return 0;
}
