# Ambience: how other programs tell tideshell where it is

tideshell dresses to match a place: an accent from the place's own colours,
its warmth, quieter motion where it is quiet. It recognizes places itself by
the WiFi it is on (`nmcli`), and any program can name the place instead.

## The place hint (one line, rewritten whenever it changes)
`$XDG_RUNTIME_DIR/tideshell.place`
```
landmark=<snapclone landmark id> name=Cafe_Nord
wifi=<bssid> name=Home_net
```
Spaces in the name are written as `_`. tideshell reads the file every 20 s;
a landmark hint wins over WiFi. snapclone writes it when it recognizes a
landmark from a photo fingerprint (snapclone-landmark); anything else may
too (a GPS geofence, a calendar).

## The log tideshell keeps (`~/.local/share/tideshell/places.log`)
One place per line, human-readable, append-safe:
```
landmark:lm-42|Cafe Nord|7|lux=9:2.301/4,13:2.877/9|sound=-52.1/12|colors=dc9650,7a5030|last=1758700000000
```
`lux` is sum of log10(lux+1) / samples per hour; `sound` the mean noise floor
(dBFS) / samples; `colors` the dominant colours seen there, most frequent
first. Nothing leaves the device.

## What comes back
- the shell's accent (season and time of day shaded 60/40 toward the
  place's first colour), the surfaces' warmth, calmer springs at quiet places;
- `notification gain` already adapts to the room's loudness
  (`$XDG_RUNTIME_DIR/tideshell.notify-gain`).

## The palette tideshell publishes (`$XDG_RUNTIME_DIR/tideshell.palette`)
The living palette itself -- the accent, the tinted surfaces, the contrast
the light calls for, light or dark -- so an app can dress to match:
```
v1 light=0 seq=41 bg_base=#101418ff bg_card=#181d23ff bg_raised=#20262dff bg_hover=#2a3139ff text=#e6e6e6ff text_weak=#9aa3adff accent=#d9905aff accent_soft=#5a4538ff border=#303841ff
```
One line, replaced atomically. Written on a visible change (3/255 in any
channel), at most twice a second; a light/dark flip is written at once.
Unknown keys are ignored and a missing role keeps the reader's own colour,
so roles can be added. danger/success/warning are deliberately not sent.
Codec, publisher and watcher: `shared/ambient-palette`; the mapping to
`PaletteColors`: `uikit_egui::theme_bridge::ambient`. vidkit-player follows
it (`VIDKIT_AMBIENT=0` to opt out).
