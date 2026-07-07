# RECONCILE — R41 ambient worker returns vs in-house

Late-returning R41 worker threads (dispatched before the single-thread switch)
duplicated the seven ambient modules I'd implemented in-house. All seven had
IDENTICAL public APIs and struct fields to the merged versions, so they were
drop-in candidates. Decision per module:

ADOPTED worker version (same behaviour, more tests — strictly better):
  - shading_uniforms   6  → 18 tests
  - erosion            6  → 12 tests (58 incl. property loops)
  - water_physics      6  → 10 tests
  - plant_growth_env   6  →  8 tests
  - fog_volume         5  →  6 tests
  - light_physics      6  →  6 tests (parity; took worker's as the official submission)

KEPT in-house version:
  - sound_masking — worker used a LINEAR attenuation model
    (loudness·1/(1+d²)·(1−occ)); in-house uses a dB model
    (loudness − 10·log10(1+d²) − occ_dB). The dragon perception stack
    (dragon_hearing → dragon_activity) is tuned to the dB scale and the worker
    model broke 4/6 dragon_hearing tests. Kept the dB model; the dragon stack
    and env_frame stay green.

Verified: cargo check --features gpu, all swapped + downstream tests
(env_frame, dragon_hearing, dragon_activity, dragon_mode), release --features
render, and HARNESS GREEN.
