# Microphone Detection on postmarketOS / phone platforms

## What's broken

User reports microphone is not being detected on postmarketOS phone.
The voice subsystem on desktop Linux uses cpal with the default audio
backend (typically ALSA or PulseAudio). On a phone with PipeWire (which
postmarketOS uses by default for newer images), the default device
enumeration may return different results — or may require explicit
permission/policy for input devices.

## What to investigate

The voice subsystem code is in `src/voice.rs`. The persistent stream is
opened via cpal:

```
cpal::default_host()
   .default_input_device()
   .ok_or("no input device")
```

If that returns None on the phone, we get the silent failure the user
is seeing. Things to try:

1. **Log the host and device enumeration on startup.** Currently we
   eprintln when the stream starts playing but not when device acquisition
   fails. Add explicit logging:
   ```
   for host_id in cpal::available_hosts() {
       eprintln!("[audio] host: {:?}", host_id);
   }
   for dev in host.input_devices().iter() {
       eprintln!("[audio] input device: {:?}", dev.name());
   }
   ```
   This tells us whether cpal sees ANY input devices at all.

2. **Try cpal's ALSA host explicitly.** PipeWire's PulseAudio compatibility
   layer sometimes hides input devices from default enumeration. Forcing
   ALSA backend may surface the mic directly:
   ```
   #[cfg(target_os = "linux")]
   let host = cpal::host_from_id(cpal::HostId::Alsa).unwrap_or_else(|_| cpal::default_host());
   ```

3. **Check XDG portal permissions.** Modern Linux phones often gate
   microphone access through xdg-desktop-portal. Apps that don't request
   permission via the portal get silent failure — the device exists but
   reads return silence. Workaround: the user can grant permission via
   plasmashell settings, OR we can integrate with the portal (significant
   work — own session).

4. **PipeWire vs ALSA device naming.** Even when the device IS found,
   sample rate / channel count negotiation may differ. Our voice.rs
   requests 44100Hz stereo; phone audio chips often default to 48000Hz
   mono. cpal should adapt, but logging is needed to confirm.

## Honest assessment

Phone audio on Linux is a notorious mess. PipeWire is replacing
PulseAudio, ALSA is the underlying layer, and access policy varies by
distro. Without device access I can write speculative fixes that may or
may not work. The most useful thing I can ship blind is BETTER LOGGING
so the user can run on the phone and send back the device enumeration —
then I can write a targeted fix.

## Proposed first step

Add comprehensive audio device logging in voice.rs at startup. Just one
file changed, adds maybe 20 lines, gives us the diagnostic data needed
to know which of the four hypotheses above to pursue.

I'll do this in the next iteration since it's a small change with no
behavior risk on desktop (just adds log lines).
