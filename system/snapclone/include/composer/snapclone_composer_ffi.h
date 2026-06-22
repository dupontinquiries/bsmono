/* snapclone_composer_ffi.h — C ABI for the group live-composer decision model.
 *
 * Given the feed's tiles (id + timestamp + has-audio), a scrub position, and
 * whether the active tile is currently playing voice, returns the up-to-3
 * visible tiles (prev/active/next) and each one's playback instruction. The Qt
 * group view renders these and wires QMediaPlayer accordingly.
 *
 * The policy (3-up window, 24h cutoff, voice gating "one at a time",
 * tap-to-replay) lives in the tested Rust crate. Link libsnapclone_composer.a.
 * No allocation crosses the boundary; the caller owns the output array.
 */
#ifndef SNAPCLONE_COMPOSER_FFI_H
#define SNAPCLONE_COMPOSER_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* playback-state codes */
#define SC_PB_PLAYING_WITH_AUDIO 0  /* active tile: play with sound from start */
#define SC_PB_PLAYING_MUTED      1  /* neighbor, no voice: loop silently */
#define SC_PB_DIMMED             2  /* neighbor, voice active: dim + time-warp + mute */
#define SC_PB_STILL              3  /* image, no audio track: just display */

/* one feed tile handed IN to the model */
typedef struct ScTile {
    uint64_t id;              /* opaque media id (app maps back to a handle) */
    int64_t  ts_ms;           /* capture/receive time, unix millis */
    int32_t  has_audio_track; /* 1 = video/audio, 0 = still image */
} ScTile;

/* one visible tile handed OUT */
typedef struct ScVisibleTile {
    uint64_t id;
    int32_t  playback; /* SC_PB_* */
    int32_t  slot;     /* -1 prev (older), 0 active, +1 next (newer) */
    int32_t  rate_pct; /* playback rate % of normal; for SC_PB_DIMMED the
                          time-warp rate (e.g. 35 = 0.35x), else 100 */
} ScVisibleTile;

/* Compute the visible window for a scrub. active tile = clamp(base_index+steps).
 * `active_has_voice` != 0 freezes the neighbors. Writes up to 3 ScVisibleTile
 * into `out` (must hold >= 3) and returns the count. If non-null,
 * `out_active_index` gets the active tile's index in the 24h-filtered list
 * (-1 if empty) and `out_window_len` the number of tiles in the 24h window. */
int32_t sc_composer_view(const ScTile *tiles, int32_t tile_count,
                         int32_t base_index, int64_t steps,
                         int32_t active_has_voice, int64_t now_ms,
                         ScVisibleTile *out,
                         int32_t *out_active_index, int32_t *out_window_len);

/* Tap-to-activate by tile id (replay). Same outputs as sc_composer_view; if the
 * id has aged out of the window it falls back to the live (newest) view. */
int32_t sc_composer_activate_id(const ScTile *tiles, int32_t tile_count,
                                uint64_t tap_id, int32_t active_has_voice,
                                int64_t now_ms, ScVisibleTile *out,
                                int32_t *out_active_index, int32_t *out_window_len);

/* ── 2D retiling layout ("two small, one big") ──────────────────────────────
 * The 3-up window is a 2D tiling layout: one BIG focused pane + the other
 * present slots as SMALL peripherals. The "rotate" gesture re-tiles by
 * promoting a different slot to big. The Qt layer animates between layouts. */

typedef struct ScTilePlacement {
    int32_t slot;   /* -1 prev, 0 active, +1 next */
    float   x, y, w, h;
    int32_t is_big; /* 1 = the single big/focused pane */
} ScTilePlacement;

#define SC_EDGE_RIGHT  0  /* smalls in a right column (wide viewports) */
#define SC_EDGE_BOTTOM 1  /* smalls in a bottom row  (tall/phone viewports) */
#define SC_EDGE_AUTO   2  /* pick by viewport aspect */

/* Compute placements for `slots` (array of slot ids), `big_slot` getting the
 * big pane. `big_fraction` (0..1; pass <=0 for default 0.68) is the big pane's
 * share of the long axis; `gap` is inter-pane spacing (pass <0 for default).
 * Fills `out` (capacity out_cap) and returns the count written. */
int32_t sc_composer_layout(const int32_t *slots, int32_t slot_count, int32_t big_slot,
                           float vp_x, float vp_y, float vp_w, float vp_h,
                           int32_t edge, float big_fraction, float gap,
                           ScTilePlacement *out, int32_t out_cap);

/* Re-tile step (the rotate gesture): returns the next slot to promote to big.
 * `forward` != 0 cycles forward through present slots. */
int32_t sc_composer_next_big(const int32_t *slots, int32_t slot_count,
                             int32_t current_big, int32_t forward);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_COMPOSER_FFI_H */
