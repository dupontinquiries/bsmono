/* snapclone_face_ffi.h — C FFI header for the snapclone-face Rust library.
 *
 * This file is committed to the repo so the project compiles without running
 * cargo first. The Rust side is in `rust/snapclone-face/src/ffi.rs`; keep
 * the two in sync by hand (cbindgen integration could automate this later).
 *
 * Phase 1 of the face-aware bandwidth optimization (see docs/FACE_BANDWIDTH.md).
 * The functions here expose ONLY the face-crop library — detection, reference
 * selection, and consent state are local-Rust concerns or live in other FFI
 * surfaces.
 *
 * Usage: #include "face/snapclone_face_ffi.h" inside extern "C" {} in C++.
 *
 *   extern "C" {
 *   #include "face/snapclone_face_ffi.h"
 *   }
 */

#ifndef SNAPCLONE_FACE_FFI_H
#define SNAPCLONE_FACE_FFI_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

/* ─── Error codes ────────────────────────────────────────────────────────── */

typedef enum FaceFfiError {
    FaceFfiError_Ok               = 0,
    FaceFfiError_InvalidInput     = 1,
    FaceFfiError_ConsentDisabled  = 2,
    FaceFfiError_BufferTooSmall   = 3,
    FaceFfiError_InvalidLabel     = 4,
    FaceFfiError_Internal         = 99,
} FaceFfiError;

/* ─── Label kind discriminator ───────────────────────────────────────────── */

typedef enum FaceLabelKind {
    /* User's own face — front camera selfies. contact_id ignored. */
    FaceLabelKind_SelfFace = 0,
    /* A face from a snap received from a specific contact. contact_id is
     * the \0-terminated UTF-8 contact identifier. */
    FaceLabelKind_Contact  = 1,
} FaceLabelKind;

/* ─── Opaque handle ──────────────────────────────────────────────────────── */

/* The Rust side defines this as `pub struct FaceLibraryHandle { ... }`. C
 * code MUST treat it as opaque — only pass pointers around, never
 * dereference. */
typedef struct FaceLibraryHandle FaceLibraryHandle;

/* ─── Lifecycle ──────────────────────────────────────────────────────────── */

/* Create a new in-memory face library. Returns non-null handle on success,
 * null on (vanishingly rare) allocation failure. Library starts ENABLED.
 *
 * budget_bytes = 0  →  use crate default (50 MB; see library.rs).
 * Caller takes ownership; must call sc_face_library_close eventually. */
FaceLibraryHandle *sc_face_library_open(size_t budget_bytes);

/* Destroy a handle. Passing null is safe (no-op). After this call, the
 * pointer is invalid. */
void sc_face_library_close(FaceLibraryHandle *handle);

/* ─── Storage operations ─────────────────────────────────────────────────── */

/* Add a face crop to the library.
 *
 *   handle         — live handle from sc_face_library_open
 *   label_kind     — SelfFace or Contact (see FaceLabelKind)
 *   contact_id     — UTF-8 \0-terminated for Contact, null for SelfFace
 *   content_hash   — 32 bytes; SHA-256 of canonical-form crop bytes
 *   bytes          — encoded image data (JPEG/PNG/etc.); may be null
 *                    iff bytes_len == 0
 *   bytes_len      — length of `bytes` in bytes
 *   width, height  — crop dimensions in pixels
 *   captured_at_ms — unix millis when this crop was captured
 *   quality        — [0.0, 1.0] quality estimate; 0.0 = unknown
 *
 * Returns Ok on success. ConsentDisabled if the library has been
 * disabled via sc_face_library_set_enabled(false). InvalidInput for
 * null required pointers, invalid UTF-8 in contact_id, or unknown
 * label_kind values.
 *
 * Duplicates (same content_hash) refresh the existing entry's
 * timestamp/quality rather than storing twice. */
FaceFfiError sc_face_library_add(
    FaceLibraryHandle *handle,
    FaceLabelKind      label_kind,
    const char        *contact_id,
    const uint8_t     *content_hash,
    const uint8_t     *bytes,
    size_t             bytes_len,
    uint32_t           width,
    uint32_t           height,
    int64_t            captured_at_ms,
    float              quality
);

/* Wipe all crops from the library. THE consent-withdrawal action;
 * always succeeds regardless of enabled state.
 *
 * If `removed_out` is non-null, writes the number of crops that were
 * removed. */
FaceFfiError sc_face_library_wipe(
    FaceLibraryHandle *handle,
    size_t            *removed_out
);

/* Enable or disable the library. Disabling refuses future writes and
 * makes reads return empty/zero, but does NOT erase stored data. Call
 * sc_face_library_wipe separately for that. */
FaceFfiError sc_face_library_set_enabled(
    FaceLibraryHandle *handle,
    bool               enabled
);

/* Returns the current enabled state. Returns false for null handle. */
bool sc_face_library_is_enabled(FaceLibraryHandle *handle);

/* Total bytes currently stored. Returns 0 for null handle. */
size_t sc_face_library_total_bytes(FaceLibraryHandle *handle);

/* Number of crops stored for a specific label. Writes to *count_out. */
FaceFfiError sc_face_library_label_count(
    FaceLibraryHandle *handle,
    FaceLabelKind      label_kind,
    const char        *contact_id,
    size_t            *count_out
);

/* ─── Detector ──────────────────────────────────────────────────────────── */

/* Opaque handle to a face detector. Independent from the library handle.
 * A typical app has one detector (shared by all callers via Arc internally)
 * and one library. */
typedef struct FaceDetectorHandle FaceDetectorHandle;

/* Open a face detector by loading a YOLO ONNX model.
 *
 *   model_path     — \0-terminated UTF-8 path to a YOLO ONNX file
 *   conf_threshold — minimum confidence to emit. Pass -1.0 for default (0.45)
 *   iou_threshold  — NMS IoU threshold. Pass -1.0 for default (0.45)
 *   error_out      — optional out-param for the specific error on failure.
 *                     Pass NULL if you don't care which error.
 *
 * Returns a non-null handle on success, NULL on failure. The model is
 * loaded immediately (50-200ms) so call at app startup, not per-frame.
 *
 * Caller takes ownership; must call sc_face_detector_close. */
FaceDetectorHandle *sc_face_detector_open(
    const char    *model_path,
    float          conf_threshold,
    float          iou_threshold,
    FaceFfiError  *error_out
);

/* Destroy a detector handle. Passing NULL is safe (no-op). */
void sc_face_detector_close(FaceDetectorHandle *handle);

/* Run detection on an RGB image.
 *
 *   rgb           — width*height*3 RGB888 bytes, tightly packed
 *   width, height — image dimensions in pixels
 *   max_results   — capacity of each output array. May be 0 to query
 *                    "how many would there be?" without copying.
 *   out_*         — five parallel output arrays. Each must point to at
 *                    least max_results writable elements (or NULL if
 *                    max_results == 0).
 *   count_out     — written with the number of detections actually
 *                    returned (always <= max_results)
 *
 * The five output arrays are parallel: detection i has bbox
 * (out_x_min[i], out_y_min[i], out_x_max[i], out_y_max[i]) and
 * confidence out_confidence[i]. Coordinates are in source-image pixel
 * space; confidence is in [0.0, 1.0].
 *
 * Detections are sorted by confidence descending (NMS-filtered).
 * When more faces exist than max_results, only the top max_results
 * by confidence are returned.
 *
 * Returns Ok on success, InvalidInput for null pointers or bad args,
 * Internal if inference failed at runtime. */
FaceFfiError sc_face_detector_detect(
    FaceDetectorHandle *handle,
    const uint8_t      *rgb,
    uint32_t            width,
    uint32_t            height,
    size_t              max_results,
    float              *out_x_min,
    float              *out_y_min,
    float              *out_x_max,
    float              *out_y_max,
    float              *out_confidence,
    size_t             *count_out
);

#endif /* SNAPCLONE_FACE_FFI_H */
