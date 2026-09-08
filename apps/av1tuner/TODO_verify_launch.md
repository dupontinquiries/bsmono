# TODO: confirm av1tuner launches on Asahi

The reported crash:

```
WGPU error: Limit 'max_color_attachments' value 8 is better than allowed 5
```

Fixed by requesting the ADAPTER'S OWN limits in `av1tuner_app/src/main.rs` rather than
wgpu's defaults — see `av1tuner_core::preflight::wgpu_limits_note`, which is a function
rather than a comment so it is greppable from the error text.

## Not verified

There is no GPU in the container this was built in. The claim is "the requested limits are
satisfiable by construction", NOT "I watched it start".

- [ ] It opens on Asahi with the default renderer (wgpu).
- [ ] `AV1TUNER_RENDERER=glow` still works, so the fallback is intact.
- [ ] If it fails again, note WHICH limit — the error stops at the first, so a second
      over-large default would look identical. Requesting the adapter's limits should make
      that impossible, and if it happens anyway the descriptor override is not taking effect.

## Why the one field was not just clamped

Setting `max_color_attachments` to 5 would have made this launch and left the next
over-large default to fail the same way on the next device. egui needs one attachment; the
eight was a default in a descriptor nobody filled in, not a requirement.
