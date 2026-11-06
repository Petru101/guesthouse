// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Shows a GNOME session in the host's viewer by screen cast. Opens a RemoteDesktop and ScreenCast session on Mutter's D-Bus API,
 * takes frames through PipeWire and copies them into exported video memory, then publishes them with the show call.
 * Input lines from a virtio-serial port go to the same session. On KMS the frames need no copy, and the session runs this for its input alone.
 * Run as the session's user. Usage: fwdcast WIDTH HEIGHT [input-port]
 */
#include <errno.h>
#include <fcntl.h>
#include <gio/gio.h>
#include <pthread.h>
#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <spa/param/buffers.h>
#include <spa/debug/types.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>
#include <vulkan/vulkan.h>
#include "nvfwd_proto.h"

#define CHECK(c) do { VkResult r_ = (c); if (r_) { fprintf(stderr, "FAILED %s: %d\n", #c, r_); exit(1); } } while (0)

static uint32_t width, height, pitch;
static int show_fd;
static uint8_t *slot_map[NVFWD_SHOW_SLOTS];
static uint64_t frames, skipped, copy_ns;
static uint32_t frame_stride;

static GDBusConnection *bus;
static char *rd_session, *sc_stream;
static struct pw_thread_loop *loop;
static struct pw_stream *stream;
static struct spa_hook stream_hook;
static struct spa_video_info_raw format;

static uint64_t now_ns(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000000000ull + t.tv_nsec;
}

/* Buffers in video memory the CPU can write and the host can read. */
static void buffers_setup(void)
{
    VkApplicationInfo ai = { VK_STRUCTURE_TYPE_APPLICATION_INFO, .apiVersion = VK_API_VERSION_1_2 };
    VkInstanceCreateInfo ii = { VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &ai };
    VkInstance inst;
    CHECK(vkCreateInstance(&ii, NULL, &inst));
    uint32_t n = 1;
    VkPhysicalDevice gpu;
    vkEnumeratePhysicalDevices(inst, &n, &gpu);
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(gpu, &mp);
    float prio = 1;
    VkDeviceQueueCreateInfo qi = { VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, .queueCount = 1, .pQueuePriorities = &prio };
    const char *ext[] = { VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME };
    VkDeviceCreateInfo di = { VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, .queueCreateInfoCount = 1, .pQueueCreateInfos = &qi,
                              .enabledExtensionCount = 1, .ppEnabledExtensionNames = ext };
    VkDevice dev;
    CHECK(vkCreateDevice(gpu, &di, NULL, &dev));
    PFN_vkGetMemoryFdKHR get_fd = (PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(dev, "vkGetMemoryFdKHR");
    show_fd = open("/dev/nvidiactl", O_RDWR | O_CLOEXEC);
    pitch = (width * 4 + 255) & ~255u;
    for (uint32_t i = 0; i < NVFWD_SHOW_SLOTS; i++) {
        VkExternalMemoryBufferCreateInfo eb = { VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_BUFFER_CREATE_INFO,
                                                .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT };
        VkBufferCreateInfo bi = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, .pNext = &eb, .size = (uint64_t)pitch * height,
                                  .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT };
        VkBuffer b;
        CHECK(vkCreateBuffer(dev, &bi, NULL, &b));
        VkMemoryRequirements m;
        vkGetBufferMemoryRequirements(dev, b, &m);
        VkMemoryPropertyFlags want = VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT | VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT;
        uint32_t t = 0;
        for (; t < mp.memoryTypeCount; t++) {
            if ((m.memoryTypeBits >> t & 1) && (mp.memoryTypes[t].propertyFlags & want) == want) {
                break;
            }
        }
        if (t == mp.memoryTypeCount) {
            fprintf(stderr, "no memory type both exportable and visible\n");
            exit(1);
        }
        VkMemoryDedicatedAllocateInfo ded = { VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO, .buffer = b };
        VkExportMemoryAllocateInfo ex = { VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO, .pNext = &ded,
                                          .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT };
        VkMemoryAllocateInfo ma = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .pNext = &ex, .allocationSize = m.size, .memoryTypeIndex = t };
        VkDeviceMemory mem;
        CHECK(vkAllocateMemory(dev, &ma, NULL, &mem));
        CHECK(vkBindBufferMemory(dev, b, mem, 0));
        CHECK(vkMapMemory(dev, mem, 0, VK_WHOLE_SIZE, 0, (void **)&slot_map[i]));
        VkMemoryGetFdInfoKHR gf = { VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR, .memory = mem, .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT };
        int fd;
        CHECK(get_fd(dev, &gf, &fd));
        struct nvfwd_show_buffer sb = { fd, i, width, height, pitch, 0x34325258, (m.size + 4095) & ~4095ull };
        if (ioctl(show_fd, NVFWD_SHOW_IOC_BUFFER, &sb)) {
            perror("show buffer");
            exit(1);
        }
        close(fd);
    }
    fprintf(stderr, "fwdcast: %u buffers of %ux%u, pitch %u, memory type %d\n", NVFWD_SHOW_SLOTS, width, height, pitch, 0);
}

/* ---- frames ---- */

static void on_process(void *data)
{
    struct pw_buffer *b, *last = NULL;
    while ((b = pw_stream_dequeue_buffer(stream))) { /* only the newest matters */
        if (last) {
            pw_stream_queue_buffer(stream, last);
            skipped++;
        }
        last = b;
    }
    if (!last) {
        return;
    }
    struct spa_data *d = &last->buffer->datas[0];
    uint32_t slot;
    if (d->data && d->chunk->size && !(d->chunk->flags & SPA_CHUNK_FLAG_CORRUPTED) && ioctl(show_fd, NVFWD_SHOW_IOC_NEXT, &slot) == 0) {
        uint64_t t0 = now_ns();
        uint32_t stride = d->chunk->stride ? d->chunk->stride : width * 4;
        const uint8_t *src = (const uint8_t *)d->data + d->chunk->offset;
        frame_stride = stride;
        if (stride == pitch) {
            memcpy(slot_map[slot], src, (size_t)pitch * height);
        } else {
            for (uint32_t y = 0; y < height; y++) {
                memcpy(slot_map[slot] + (size_t)y * pitch, src + (size_t)y * stride, width * 4);
            }
        }
        __sync_synchronize();
        copy_ns += now_ns() - t0;
        frames++;
        uint64_t word = slot | frames << 8;
        ioctl(show_fd, NVFWD_SHOW_IOC_FRAME, &word);
    }
    pw_stream_queue_buffer(stream, last);
}

static void on_param(void *data, uint32_t id, const struct spa_pod *param)
{
    if (!param || id != SPA_PARAM_Format) {
        return;
    }
    spa_format_video_raw_parse(param, &format);
    fprintf(stderr, "fwdcast: format %s %ux%u\n", spa_debug_type_find_name(spa_type_video_format, format.format), format.size.width, format.size.height);
    uint8_t buf[256];
    struct spa_pod_builder p = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
    const struct spa_pod *params[1];
    params[0] = spa_pod_builder_add_object(&p, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
                                           SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int((1 << SPA_DATA_MemFd) | (1 << SPA_DATA_MemPtr)));
    pw_stream_update_params(stream, params, 1);
}

static void on_state(void *data, enum pw_stream_state old, enum pw_stream_state state, const char *error)
{
    fprintf(stderr, "fwdcast: stream %s %s\n", pw_stream_state_as_string(state), error ? error : "");
}

static const struct pw_stream_events events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = on_state, .param_changed = on_param, .process = on_process,
};

static void stream_connect(uint32_t node)
{
    uint8_t buf[1024];
    struct spa_pod_builder p = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
    const struct spa_pod *params[1];
    struct spa_rectangle size = SPA_RECTANGLE(width, height), low = SPA_RECTANGLE(1, 1), high = SPA_RECTANGLE(16384, 16384);
    struct spa_fraction rate = SPA_FRACTION(0, 1), rlow = SPA_FRACTION(0, 1), rhigh = SPA_FRACTION(240, 1), rdef = SPA_FRACTION(240, 1);
    params[0] = spa_pod_builder_add_object(&p, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
        SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format, SPA_POD_CHOICE_ENUM_Id(3, SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRA),
        SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(&size, &low, &high),
        SPA_FORMAT_VIDEO_framerate, SPA_POD_Fraction(&rate),
        /* A maximum above the monitor's rate: at 60, Mutter drops frames that arrive a moment early. */
        SPA_FORMAT_VIDEO_maxFramerate, SPA_POD_CHOICE_RANGE_Fraction(&rdef, &rlow, &rhigh));
    pw_thread_loop_lock(loop);
    stream = pw_stream_new_simple(pw_thread_loop_get_loop(loop), "fwdcast",
                                  pw_properties_new(PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY, "Capture", PW_KEY_MEDIA_ROLE, "Screen", NULL),
                                  &events, NULL);
    pw_stream_connect(stream, PW_DIRECTION_INPUT, node, PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS, params, 1);
    pw_thread_loop_unlock(loop);
}

/* ---- D-Bus ---- */

static GVariant *call(const char *name, const char *path, const char *iface, const char *method, GVariant *args)
{
    GError *e = NULL;
    GVariant *r = g_dbus_connection_call_sync(bus, name, path, iface, method, args, NULL, G_DBUS_CALL_FLAGS_NONE, 10000, NULL, &e);
    if (!r) {
        fprintf(stderr, "%s: %s\n", method, e->message);
        exit(1);
    }
    return r;
}

static void notify(const char *method, GVariant *args)
{
    g_dbus_connection_call(bus, "org.gnome.Mutter.RemoteDesktop", rd_session, "org.gnome.Mutter.RemoteDesktop.Session", method, args,
                           NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
}

static void on_stream_added(GDBusConnection *c, const char *sender, const char *path, const char *iface, const char *signal, GVariant *args, gpointer data)
{
    uint32_t node;
    g_variant_get(args, "(u)", &node);
    fprintf(stderr, "fwdcast: PipeWire node %u\n", node);
    if (!getenv("FWDCAST_INPUT_ONLY")) { /* on KMS the frames go another way: the stream is there only to aim the pointer */
        stream_connect(node);
    }
}

/* Lines: m X Y, b BUTTON 0|1, k EVDEVCODE 0|1, a AXIS STEPS */
static void input_line(const char *line)
{
    if (getenv("FWDCAST_LOG_KEYS") && line[0] == 'k') {
        struct timespec now;
        clock_gettime(CLOCK_REALTIME, &now);
        fprintf(stderr, "key %s at %ld.%09ld\n", line, now.tv_sec, now.tv_nsec);
    }
    double x, y;
    int a, b;
    if (sscanf(line, "m %lf %lf", &x, &y) == 2) {
        notify("NotifyPointerMotionAbsolute", g_variant_new("(sdd)", sc_stream, x, y));
    } else if (sscanf(line, "b %d %d", &a, &b) == 2) {
        notify("NotifyPointerButton", g_variant_new("(ib)", a, b));
    } else if (sscanf(line, "k %d %d", &a, &b) == 2) {
        notify("NotifyKeyboardKeycode", g_variant_new("(ub)", a, b));
    } else if (sscanf(line, "a %d %d", &a, &b) == 2) {
        notify("NotifyPointerAxisDiscrete", g_variant_new("(ui)", a, b));
    }
}

/* A thread of its own: with nobody on the host side the port reads as end of file at once, for ever. */
static void *input_thread(void *data)
{
    char buf[4096];
    size_t have = 0;
    int fd = open(data, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        perror("input port");
        return NULL;
    }
    for (;;) {
        ssize_t n = read(fd, buf + have, sizeof(buf) - have - 1);
        if (n <= 0) {
            usleep(100000);
            continue;
        }
        have += n;
        buf[have] = 0;
        char *start = buf, *end;
        while ((end = strchr(start, '\n'))) {
            *end = 0;
            input_line(start);
            start = end + 1;
        }
        have -= start - buf;
        memmove(buf, start, have);
    }
    return NULL;
}

static gboolean report(gpointer data)
{
    static uint64_t last, last_copy;
    fprintf(stderr, "fwdcast: %llu frames, %llu a 5 s, copy %.2f ms each, %llu skipped, stride %u\n", (unsigned long long)frames, (unsigned long long)(frames - last),
            frames > last ? (copy_ns - last_copy) / 1e6 / (frames - last) : 0.0, (unsigned long long)skipped, frame_stride);
    last = frames;
    last_copy = copy_ns;
    return TRUE;
}

int main(int argc, char **argv)
{
    width = argc > 1 ? atoi(argv[1]) : 1920;
    height = argc > 2 ? atoi(argv[2]) : 1080;
    if (!getenv("FWDCAST_INPUT_ONLY")) {
        buffers_setup();
    }
    pw_init(&argc, &argv);
    loop = pw_thread_loop_new("fwdcast", NULL);
    pw_thread_loop_start(loop);
    bus = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, NULL);
    GVariant *r = call("org.gnome.Mutter.RemoteDesktop", "/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession", NULL);
    g_variant_get(r, "(o)", &rd_session);
    r = call("org.gnome.Mutter.RemoteDesktop", rd_session, "org.freedesktop.DBus.Properties", "Get",
             g_variant_new("(ss)", "org.gnome.Mutter.RemoteDesktop.Session", "SessionId"));
    GVariant *v;
    g_variant_get(r, "(v)", &v);
    const char *id = g_variant_get_string(v, NULL);
    GVariantBuilder o;
    g_variant_builder_init(&o, G_VARIANT_TYPE("a{sv}"));
    g_variant_builder_add(&o, "{sv}", "remote-desktop-session-id", g_variant_new_string(id));
    char *sc_session;
    r = call("org.gnome.Mutter.ScreenCast", "/org/gnome/Mutter/ScreenCast", "org.gnome.Mutter.ScreenCast", "CreateSession", g_variant_new("(a{sv})", &o));
    g_variant_get(r, "(o)", &sc_session);
    g_variant_builder_init(&o, G_VARIANT_TYPE("a{sv}"));
    g_variant_builder_add(&o, "{sv}", "cursor-mode", g_variant_new_uint32(getenv("FWD_NO_CURSOR") ? 0 : 1));
    r = call("org.gnome.Mutter.ScreenCast", sc_session, "org.gnome.Mutter.ScreenCast.Session", "RecordMonitor", g_variant_new("(sa{sv})", "", &o));
    g_variant_get(r, "(o)", &sc_stream);
    g_dbus_connection_signal_subscribe(bus, NULL, "org.gnome.Mutter.ScreenCast.Stream", "PipeWireStreamAdded", sc_stream, NULL, G_DBUS_SIGNAL_FLAGS_NONE, on_stream_added, NULL, NULL);
    call("org.gnome.Mutter.RemoteDesktop", rd_session, "org.gnome.Mutter.RemoteDesktop.Session", "Start", NULL);
    if (argc > 3) {
        pthread_t input;
        pthread_create(&input, NULL, input_thread, argv[3]);
    }
    g_timeout_add_seconds(5, report, NULL);
    g_main_loop_run(g_main_loop_new(NULL, FALSE));
    return 0;
}
