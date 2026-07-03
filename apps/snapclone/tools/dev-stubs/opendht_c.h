/* opendht_c.h — MINIMAL STUB for syntax-checking SnapClone's DHT code.
 *
 * This is NOT the real OpenDHT C API header. It declares exactly the subset of
 * the opendht C API that SnapClone's JamiAdapter_dht_impl.cpp uses, with
 * matching signatures, so that file (and the DHT-gated branches elsewhere) can
 * be compiled with -fsyntax-only WITHOUT the real OpenDHT library — which is
 * built from a git submodule that isn't fetchable in this sandbox.
 *
 * Signatures were reverse-engineered from the call sites; they mirror the real
 * opendht/c/opendht_c.h. Good enough to catch signature/type errors in our
 * code. It will NOT link (no implementations) — this is a syntax/type gate only.
 */
#ifndef OPENDHT_C_STUB_H
#define OPENDHT_C_STUB_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque runtime + value types (always used behind pointers). */
typedef struct dht_runner dht_runner;
typedef struct dht_value  dht_value;

/* InfoHash is a fixed 20-byte key, used by value (callers take its address). */
typedef struct dht_infohash {
    uint8_t bytes[20];
} dht_infohash;

/* A borrowed view into a value's bytes (returned by value). */
typedef struct dht_data_view {
    const uint8_t *data;
    size_t         size;
} dht_data_view;

/* Callback typedefs matching the call sites. */
typedef bool (*dht_get_cb)(const dht_value *value, void *user_data);
typedef void (*dht_done_cb)(bool ok, void *user_data);
typedef void (*dht_shutdown_cb)(void *user_data);

/* ── infohash ─────────────────────────────────────────────────────────────── */
void dht_infohash_get_from_string(dht_infohash *h, const char *s);

/* ── runner lifecycle ─────────────────────────────────────────────────────── */
dht_runner *dht_runner_new(void);
void        dht_runner_delete(dht_runner *runner);
int         dht_runner_run(dht_runner *runner, uint16_t port);
void        dht_runner_bootstrap(dht_runner *runner, const char *host, const char *service);
void        dht_runner_shutdown(dht_runner *runner, dht_shutdown_cb cb, void *user_data);

/* ── put / get ────────────────────────────────────────────────────────────── */
void dht_runner_put(dht_runner *runner, const dht_infohash *h, dht_value *value,
                    dht_done_cb done_cb, void *user_data, bool permanent);
void dht_runner_get(dht_runner *runner, const dht_infohash *h,
                    dht_get_cb value_cb, dht_done_cb done_cb, void *user_data);

/* ── value ────────────────────────────────────────────────────────────────── */
dht_value    *dht_value_new(const uint8_t *data, size_t size);
void          dht_value_unref(dht_value *value);
dht_data_view dht_value_get_data(const dht_value *value);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* OPENDHT_C_STUB_H */
