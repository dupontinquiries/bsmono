# TODO: end-to-end verification — snaps, calls, friends, QR

Asked 2026-09-08: *"make sure snaps and voice calls and add friends wired
end-to-end"* and *"make sure qr scanner works"*.

NOT VERIFIED. Each of these crosses a boundary this container cannot reach — a
camera, a microphone, a second device, or a network — so the honest status is
"unit-tested in pieces, never walked whole".

## What IS known, from this session's reading

- **Video snaps cannot encode yet, and the reason is not the encoder.**
  `video_backend` picks a backend and `av1_encode` builds the argv (both wired
  in v0.1.7xx), but `video_record::Recorder` COUNTS frames — it does not write
  them. There is no input for ffmpeg to read. The camera page now prints the
  command that would run; making it runnable means writing frames to disk first.
- **`vcard` import is wired** (`Action::ImportVCard` → `vcard::parse`), so "add
  friends" has at least one working path.
- Both previously-unreferenced modules (`video_backend`, `av1_encode`) now have
  callers; nothing else in the crate is unreferenced.

## The walk, in dependency order so a failure isolates

- [ ] Camera preview renders at all (blank preview fails everything below it).
- [ ] A photo snap: capture → send → arrives → opens → expires.
- [x] `video_record` produces FRAMES ON DISK. `video_spill` is the sink and the
      camera loop now calls `Recorder::offer` — which had NO CALLER at all, so the
      recorder counted frames it never saw. Raw RGBA, exactly the shape
      `Source::RawStdin` describes to ffmpeg.
      - [ ] Confirm on the device: a recording leaves a file in
            `~/.cache/snapclone/spill/` whose size is frames x w x h x 4.
      - [x] It is piped in. `video_encode_run` spawns the encoder on its own
            thread with the spill streamed to stdin, and the result comes back
            over a channel to the camera page.
      - [ ] Confirm on the device that the clip PLAYS. Everything up to the
            ffmpeg invocation is tested here (against `sh -c 'cat > file'`, which
            has the same stdin contract); whether real ffmpeg accepts the argv
            for a real sensor's frames is not something this container can say.
- [ ] A video snap, once the above lands.
- [ ] Voice call: ring → answer → audio both ways → hang up. Note WHICH leg
      fails, since the signalling and the media path are separate.
- [ ] Add a friend by QR: scan → decode → request → accept on the other device.
- [ ] Add a friend by vcard import.
- [ ] QR SCANNER specifically: does the camera feed reach the decoder, and does
      a decode that fails say so? A scanner that silently does nothing is
      indistinguishable from a camera that is not running.

## Note on the QR reveal fix

`qr_shown_at` is cleared on OPEN rather than on close (v0.1.6xx) because three
close paths existed and only one cleared it. If the scanner shows a stale code,
that is the place to look first.
