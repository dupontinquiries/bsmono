/*
 * tilt_sensor_sysfs.c — raw accelerometer daemon, sysfs fallback
 *
 * Use this if libiio is not available. Reads directly from
 * /sys/bus/iio/devices/iio:device*/in_accel_{x,y,z}_raw
 *
 * Build:
 *   gcc -O2 -o tilt_sensor_sysfs tilt_sensor_sysfs.c
 *   (no extra libs needed)
 *
 * Run:
 *   ./tilt_sensor_sysfs | python3 ball_maze.py YourName
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <time.h>
#include <glob.h>
#include <errno.h>

#define SAMPLE_HZ   50
#define SLEEP_NS    (1000000000L / SAMPLE_HZ)
#define PATH_MAX_L  256

static volatile int running = 1;
static void on_signal(int s) { (void)s; running = 0; }

static double read_raw_from(const char *path)
{
    FILE *f = fopen(path, "r");
    if (!f) return 0.0;
    long raw = 0;
    fscanf(f, "%ld", &raw);
    fclose(f);
    return (double)raw;
}

static double read_sysfs_double(const char *path)
{
    FILE *f = fopen(path, "r");
    if (!f) return 0.0;
    double v = 0.0;
    fscanf(f, "%lf", &v);
    fclose(f);
    return v;
}

static int find_accel_base(char *out_base, size_t sz)
{
    /* Try iio:device0 through iio:device9 looking for in_accel_x_raw */
    for (int i = 0; i < 10; i++) {
        char path[PATH_MAX_L];
        snprintf(path, sizeof(path),
                 "/sys/bus/iio/devices/iio:device%d/in_accel_x_raw", i);
        if (access(path, R_OK) == 0) {
            snprintf(out_base, sz,
                     "/sys/bus/iio/devices/iio:device%d", i);
            return 0;
        }
    }
    return -1;
}

int main(void)
{
    signal(SIGINT,  on_signal);
    signal(SIGTERM, on_signal);

    char base[PATH_MAX_L];
    if (find_accel_base(base, sizeof(base)) != 0) {
        fprintf(stderr, "tilt_sensor_sysfs: no IIO accelerometer found in sysfs\n");
        fprintf(stderr, "  Searched /sys/bus/iio/devices/iio:device*/in_accel_x_raw\n");
        return 1;
    }
    fprintf(stderr, "tilt_sensor_sysfs: using %s\n", base);

    /* Read scale (may be per-axis or shared) */
    char path[PATH_MAX_L];
    snprintf(path, sizeof(path), "%s/in_accel_scale", base);
    double scale = read_sysfs_double(path);
    if (scale == 0.0) scale = 1.0;

    double sx = scale, sy = scale, sz = scale;
    /* Check per-axis scales */
    snprintf(path, sizeof(path), "%s/in_accel_x_scale", base);
    double tmp = read_sysfs_double(path);
    if (tmp != 0.0) sx = tmp;
    snprintf(path, sizeof(path), "%s/in_accel_y_scale", base);
    tmp = read_sysfs_double(path);
    if (tmp != 0.0) sy = tmp;
    snprintf(path, sizeof(path), "%s/in_accel_z_scale", base);
    tmp = read_sysfs_double(path);
    if (tmp != 0.0) sz = tmp;

    fprintf(stderr, "tilt_sensor_sysfs: scale x=%.6f y=%.6f z=%.6f\n", sx, sy, sz);

    char px[PATH_MAX_L], py[PATH_MAX_L], pz[PATH_MAX_L];
    snprintf(px, sizeof(px), "%s/in_accel_x_raw", base);
    snprintf(py, sizeof(py), "%s/in_accel_y_raw", base);
    snprintf(pz, sizeof(pz), "%s/in_accel_z_raw", base);

    printf("TILT_SENSOR_V1\n");
    fflush(stdout);

    struct timespec ts = { .tv_sec = 0, .tv_nsec = SLEEP_NS };

    while (running) {
        double x = read_raw_from(px) * sx;
        double y = read_raw_from(py) * sy;
        double z = read_raw_from(pz) * sz;
        printf("ACCEL %.4f %.4f %.4f\n", x, y, z);
        fflush(stdout);
        nanosleep(&ts, NULL);
    }
    return 0;
}
