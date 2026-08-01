# av1tuner and av1tuner-m1: which one is live

`apps/av1tuner-m1` is the **live fork**. `apps/av1tuner` is stale and should be
retired once someone confirms nothing external references it.

Five files differ, and **m1 is ahead in every one**:

| file | m1 adds |
|---|---|
| `av1tuner_core/src/concat.rs` | **+297 lines**: VVenC raw H.266 muxing through mkvmerge, because ffmpeg's raw H.266 demuxer does not synthesise per-packet timestamps |
| `av1tuner_core/src/preflight.rs` | an mkvmerge check for VVenC, so a missing dependency surfaces before a long encode instead of at the very end |
| `av1tuner_app/src/app.rs` | cache hits are **verified decodable** before being accepted — a cached file can be byte-intact and still not decode |
| `av1tuner_core/src/cleanup.rs` | `combined.266`, `audio.opus` and friends recognised as intermediates |
| `av1tuner_core/src/job.rs` | whitespace only |

Nothing in the base copy is missing from m1, so consolidating loses nothing.

## Both were broken the same way

`av1tuner_core` **did not compile its tests** in either copy: a test helper in
`scene.rs` constructs `VideoInfo` without the `audio_streams` field the struct
gained when audio-encode mode landed.

That is why it went unnoticed — the **library** still built, so the app ran and
only the tests were dead. Nothing that ran was broken; everything that would
have *checked* it was.

With the helper fixed, m1's suite runs and found two real defects:

* `derive_state_dir` relied on `Path::parent()` returning `None` for a bare
  filename. It returns `Some("")`, so the `unwrap_or(".")` branch was **dead
  code** and the intended `./name` never happened.
* A test asserted `102400 / 8 + 1` and called it `div_ceil`. 102400 divides by 8
  exactly, so the answer is 12800 — the **code was right and the test was not**.

m1 is now **97/97**.
