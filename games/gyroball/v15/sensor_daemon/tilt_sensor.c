/*
 * tilt_sensor.c — raw accelerometer daemon for ball_maze.py
 *
 * Reads X/Y/Z acceleration from the IIO subsystem via libiio and streams
 * them to stdout as:   "ACCEL x y z\n"   at ~50 Hz.
 *
 * ball_maze.py reads this instead of monitor-sensor, getting smooth
 * continuous float values on all three axes instead of coarse discrete states.
 *
 * Build on PostmarketOS / Alpine:
 *   apk add libiio-dev gcc
 *   gcc -O2 -o tilt_sensor tilt_sensor.c -liio
 *
 * Build on Debian/Ubuntu:
 *   apt install libiio-dev gcc
 *   gcc -O2 -o tilt_sensor tilt_sensor.c -liio
 *
 * Run with ball_maze.py:
 *   ./tilt_sensor | python3 ball_maze.py YourName
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <time.h>
#include <iio.h>

/* ── tunables ───────────────────────────────────────────────────────────────── */
#define SAMPLE_HZ       50          /* output rate */
#define SLEEP_NS        (1000000000L / SAMPLE_HZ)
#define SCALE_DEFAULT   1.0         /* applied if no iio scale attr found */

/* Names to search for — iio device names vary by driver / vendor */
static const char *ACCEL_NAMES[] = {
    "accel_3d",
    "bmc150_accel",
    "bmi160_accel",
    "lsm6dso_accel",
    "mpu6050",
    "kxcjk-1013",
    "st_lsm6dsx_accel",
    NULL
};

/* ── globals ────────────────────────────────────────────────────────────────── */
static volatile int running = 1;
static void on_signal(int s) { (void)s; running = 0; }

/* ── helpers ────────────────────────────────────────────────────────────────── */
static struct iio_device *find_accel(struct iio_context *ctx)
{
    unsigned int n = iio_context_get_devices_count(ctx);
    for (unsigned int i = 0; i < n; i++) {
        struct iio_device *dev = iio_context_get_device(ctx, i);
        const char *name = iio_device_get_name(dev);
        if (!name) continue;
        for (int j = 0; ACCEL_NAMES[j]; j++) {
            if (strstr(name, ACCEL_NAMES[j])) return dev;
        }
    }
    /* Fallback: return first device that has accel_x channel */
    for (unsigned int i = 0; i < n; i++) {
        struct iio_device *dev = iio_context_get_device(ctx, i);
        if (iio_device_find_channel(dev, "accel_x", false)) return dev;
    }
    return NULL;
}

static double read_scale(struct iio_channel *ch)
{
    char buf[64];
    if (iio_channel_attr_read(ch, "scale", buf, sizeof(buf)) > 0)
        return atof(buf);
    return SCALE_DEFAULT;
}

static double read_raw(struct iio_channel *ch)
{
    char buf[64];
    long long raw = 0;
    if (iio_channel_attr_read(ch, "raw", buf, sizeof(buf)) > 0)
        raw = atoll(buf);
    return (double)raw;
}

/* ── main ───────────────────────────────────────────────────────────────────── */
int main(void)
{
    signal(SIGINT,  on_signal);
    signal(SIGTERM, on_signal);

    struct iio_context *ctx = iio_create_local_context();
    if (!ctx) {
        fprintf(stderr, "tilt_sensor: failed to create IIO context\n");
        return 1;
    }

    struct iio_device *dev = find_accel(ctx);
    if (!dev) {
        fprintf(stderr, "tilt_sensor: no accelerometer found\n");
        fprintf(stderr, "  Available devices:\n");
        unsigned int n = iio_context_get_devices_count(ctx);
        for (unsigned int i = 0; i < n; i++) {
            const char *name = iio_device_get_name(iio_context_get_device(ctx, i));
            fprintf(stderr, "    %s\n", name ? name : "(unnamed)");
        }
        iio_context_destroy(ctx);
        return 1;
    }

    fprintf(stderr, "tilt_sensor: using device '%s'\n", iio_device_get_name(dev));

    /* Try to find channels — names vary (accel_x vs in_accel_x) */
    struct iio_channel *ch_x = iio_device_find_channel(dev, "accel_x", false);
    struct iio_channel *ch_y = iio_device_find_channel(dev, "accel_y", false);
    struct iio_channel *ch_z = iio_device_find_channel(dev, "accel_z", false);

    if (!ch_x || !ch_y || !ch_z) {
        /* Try alternate naming */
        ch_x = iio_device_find_channel(dev, "in_accel_x", false);
        ch_y = iio_device_find_channel(dev, "in_accel_y", false);
        ch_z = iio_device_find_channel(dev, "in_accel_z", false);
    }

    if (!ch_x || !ch_y || !ch_z) {
        fprintf(stderr, "tilt_sensor: missing accel channels\n");
        iio_context_destroy(ctx);
        return 1;
    }

    double sx = read_scale(ch_x);
    double sy = read_scale(ch_y);
    double sz = read_scale(ch_z);
    fprintf(stderr, "tilt_sensor: scale x=%.6f y=%.6f z=%.6f\n", sx, sy, sz);

    /* Announce ourselves so ball_maze.py knows which parser to use */
    printf("TILT_SENSOR_V1\n");
    fflush(stdout);

    struct timespec ts = { .tv_sec = 0, .tv_nsec = SLEEP_NS };

    while (running) {
        double x = read_raw(ch_x) * sx;
        double y = read_raw(ch_y) * sy;
        double z = read_raw(ch_z) * sz;

        /* Output: "ACCEL <x> <y> <z>"  (m/s² or g, depending on driver scale) */
        printf("ACCEL %.4f %.4f %.4f\n", x, y, z);
        fflush(stdout);

        nanosleep(&ts, NULL);
    }

    iio_context_destroy(ctx);
    return 0;
}
