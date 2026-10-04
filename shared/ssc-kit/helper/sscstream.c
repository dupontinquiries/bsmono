// sscstream: stream the phone's IMU through libssc as text lines, one per measurement:
//   A <ms> x y z   accelerometer, m/s^2      G <ms> x y z   gyroscope, rad/s
//   M <ms> x y z   magnetometer, uT
// usage: sscstream [a][g][m] [seconds]   (default: agm, forever). <ms> is the monotonic clock in milliseconds.
#include <libssc.h>
#include <libssc-sensor-magnetometer.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void on_measure(SSCSensor *s, gfloat x, gfloat y, gfloat z, gpointer tag) {
    (void)s;
    printf("%c %.1f %.5f %.5f %.5f\n", (char)GPOINTER_TO_INT(tag), g_get_monotonic_time() / 1000.0, x, y, z);
    fflush(stdout);
}
static gboolean stop(gpointer loop) { g_main_loop_quit(loop); return G_SOURCE_REMOVE; }

int main(int argc, char **argv) {
    const char *which = argc > 1 && strpbrk(argv[1], "agm") ? argv[1] : "agm";
    int secs = argc > 2 ? atoi(argv[2]) : (argc > 1 && !strpbrk(argv[1], "agm") ? atoi(argv[1]) : 0);
    GError *err = NULL;
    GMainLoop *loop = g_main_loop_new(NULL, FALSE);
    if (strchr(which, 'a')) {
        SSCSensorAccelerometer *a = ssc_sensor_accelerometer_new_sync(NULL, &err);
        if (!a || !ssc_sensor_accelerometer_open_sync(a, NULL, &err)) { fprintf(stderr, "accelerometer: %s\n", err ? err->message : "?"); return 1; }
        g_signal_connect(a, "measurement", G_CALLBACK(on_measure), GINT_TO_POINTER('A'));
    }
    if (strchr(which, 'g')) {
        SSCSensorGyroscope *g = ssc_sensor_gyroscope_new_sync(NULL, &err);
        if (!g || !ssc_sensor_gyroscope_open_sync(g, NULL, &err)) { fprintf(stderr, "gyroscope: %s\n", err ? err->message : "?"); err = NULL; }
        else g_signal_connect(g, "measurement", G_CALLBACK(on_measure), GINT_TO_POINTER('G'));
    }
    if (strchr(which, 'm')) {
        SSCSensorMagnetometer *m = ssc_sensor_magnetometer_new_sync(NULL, &err);
        if (!m || !ssc_sensor_magnetometer_open_sync(m, NULL, &err)) { fprintf(stderr, "magnetometer: %s\n", err ? err->message : "?"); err = NULL; }
        else g_signal_connect(m, "measurement", G_CALLBACK(on_measure), GINT_TO_POINTER('M'));
    }
    if (secs > 0) g_timeout_add_seconds(secs, stop, loop);
    g_main_loop_run(loop);
    return 0;
}
