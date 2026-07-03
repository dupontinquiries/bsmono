#ifndef SNAPCLONE_CORESCHED_FFI_H
#define SNAPCLONE_CORESCHED_FFI_H

/*
 * snapclone_coresched_ffi.h — C ABI for the Rust snapclone-coresched crate.
 *
 * Efficiency-core classification + thread-placement policy for battery savings.
 * The crate DECIDES (parses sysfs, classifies E/P cores, picks a placement per
 * work class); the C++ caller APPLIES the decision with the relevant Linux
 * syscall on the target thread's TID:
 *   - SC_MECH_AFFINITY_EFFICIENCY → sched_setaffinity(tid, mask of cpu ids)
 *   - SC_MECH_UCLAMP_EFFICIENCY   → sched_setattr(tid, SCHED_FLAG_UTIL_CLAMP,
 *                                                 util_clamp_max = uclamp_max)
 *   - SC_MECH_NICE_ONLY           → setpriority(PRIO_PROCESS, tid, nice_delta)
 *   - SC_MECH_NONE                → leave it to the scheduler
 * The nice_delta should be applied in ADDITION to affinity/uclamp.
 *
 * Keeping the unsafe syscall surface in C++ (where these calls are trivial)
 * keeps this crate pure, portable, and fully unit-tested.
 *
 * Available when CMake is built with -DSNAPCLONE_RUST_CORESCHED=ON.
 */

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Work-class codes (match Rust WorkClass). */
#define SC_WORK_INTERACTIVE          0
#define SC_WORK_LATENCY_SENSITIVE    1
#define SC_WORK_PERIODIC_BACKGROUND  2
#define SC_WORK_BACKGROUND_COMPUTE   3
#define SC_WORK_BACKGROUND_IO        4

/* Mechanism codes (match Rust Mechanism). */
#define SC_MECH_NONE                 0
#define SC_MECH_NICE_ONLY            1
#define SC_MECH_UCLAMP_EFFICIENCY    2
#define SC_MECH_AFFINITY_EFFICIENCY  3

typedef struct CPlacement {
    uint8_t  mechanism;   /* SC_MECH_* */
    int32_t  nice_delta;  /* 0..19; apply via setpriority in addition */
    uint32_t uclamp_max;  /* 0..1024 util ceiling for SC_MECH_UCLAMP_* */
    uint32_t cpu_count;   /* number of cpu ids the placement wants pinned */
} CPlacement;

/*
 * 1 if the system is heterogeneous (has efficiency cores worth demoting onto),
 * 0 if uniform (desktop/server — caller should skip pinning; nice still helps).
 * Typically called once at startup and cached.
 */
uint8_t sc_coresched_has_efficiency_cores(void);

/*
 * Query the placement for a work class. Writes scalar fields into *out and the
 * cpu id list into cpu_out (up to cpu_out_cap entries). out->cpu_count is the
 * TRUE count (may exceed cpu_out_cap — then only the first cpu_out_cap ids were
 * written, which is fine: a smaller affinity set is still all-efficiency-cores).
 * Returns 0 on success, 1 if out is NULL.
 *
 * cpu_out may be NULL with cpu_out_cap 0 if the caller only needs the scalar
 * fields (e.g. for SC_MECH_NICE_ONLY / SC_MECH_NONE classes).
 */
uint8_t sc_coresched_placement(uint8_t work_class,
                               CPlacement *out,
                               uint32_t   *cpu_out,
                               uint32_t    cpu_out_cap);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_CORESCHED_FFI_H */
