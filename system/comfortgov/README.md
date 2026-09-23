# comfortgov

A comfort governor for phones. Not a kernel governor and not a cpupower
plugin: a userspace daemon that owns the cpufreq *windows* (min/max per
cluster) and a few vm knobs, on top of the kernel's schedutil. Dry-run by
default; nothing is written without `--apply`.

```
comfortgov --probe          what this system exposes (OPPs, thermal zones, battery, MGLRU, PSI)
comfortgov                  decisions logged, nothing written
comfortgov --apply          writes (root)
```

## Model ("sleep vs work")

| state | floor | ceiling | when |
|---|---|---|---|
| Sleep | OPP minimum | the knee | no input for 2 s, light load |
| Interactive | the knee | max | input within 2 s |
| Burst | the knee | max | heavy load (util ≥ 0.75) that started < 8 s ago: race to idle |
| Sustained | the knee | 70 % of max | heavy load older than 8 s that never drained |

**The knee** is the highest frequency at the minimum voltage. Below it a
lower frequency saves almost nothing (the voltage cannot fall further) and
adds jitter; `powersave` pins the OPP minimum, which is why it feels rough.
The heuristic knee is the lowest OPP ≥ 30 % of max; `--probe` prints the OPP
table, set `--floor policyN=KHZ` per cluster to the real knee once known.

**Thermal ceiling** from the skin sensor (`skin`/`case`/`quiet` zones; else
the battery pack, which sits against the hand; else the hottest CPU zone
minus 12 °C): 1.0 up to 38 °C, falling to 0.5 at 43 °C, slewed at 5 %/s
(the case lags the die by minutes; bursts stay free, sustained load is
what warms the hand), released only below 37 °C.

**Battery**: below 20 % on battery, everything runs under the 70 % cap.

**Memory**: swappiness 40 and MGLRU `min_ttl_ms` 1000 while interacting
(working-set protection), 100/0 asleep; proactive `memory.reclaim` of the
background cgroup (`--bg-cgroup`) in 64 MiB steps at most every 5 s, only
asleep, PSI-quiet, and on the charger or short of RAM. That needs apps in
their own cgroups (hatch launching each app in a transient scope): not
wired yet, so reclaim is off unless the cgroup is given.

Input comes from the mtime of a hint file (`$XDG_RUNTIME_DIR/comfortgov.input`);
tideshell should touch it on input. Verified against a fake sysfs tree
(policy: 6 rules break-tested, all caught; sysfs: discovery, sampling,
write order); never yet on the FP5.
