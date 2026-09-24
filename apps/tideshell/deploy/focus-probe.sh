#!/bin/sh
# focus-probe.sh — characterise a VCM lens actuator through V4L2.
#
# Every step gets a deterministic two-word name (adjective_noun, derived from
# the step's own parameters), so a run can be narrated out loud and annotated
# afterwards: "quiet_lark buzzed, drowsy_otter did nothing". The same step
# always gets the same name, across runs and across devices, because the name
# is a hash of what the step DOES, not of when it ran.
#
#   ./focus-probe.sh find                    which subdev has focus_absolute
#   ./focus-probe.sh range                   travel limits and dead zone
#   ./focus-probe.sh steps                   step sizes, smallest audible
#   ./focus-probe.sh sweep                   square waves, find resonance
#   ./focus-probe.sh curves                  square/sine/triangle/saw/decay/dither
#   ./focus-probe.sh chirp                   one glide 5->40 and 40->120 Hz
#   ./focus-probe.sh hold                    hold current vs temperature
#   ./focus-probe.sh rate                    how fast writes actually go
#   ./focus-probe.sh all                     everything, ~6 minutes
#   ./focus-probe.sh note quiet_lark "loud"  annotate a step by name
#   ./focus-probe.sh notes                   print the annotations
#   ./focus-probe.sh hits                    re-print the listening report
#
# WHILE IT RUNS: hit Enter every time you hear the lens. Each press is
# timestamped and attributed to whichever step was running, so the report at
# the end says which steps were audible and how many ticks each produced —
# no need to remember or write anything down.
#
# Env: SUBDEV=/dev/v4l-subdevN (auto-detected), DRY=1 to print without writing,
#      SUDO=doas or SUDO= (none, if already root), LOG=path, FORCE=1 to run
#      even while something holds a camera node.
#
# SAFETY: a spring-return VCM holds position with CONTINUOUS current, so
# resting far from 0 dissipates heat against the lens stack forever. Every
# subcommand parks at 0 on exit, including on Ctrl-C. `hold` is the one test
# that deliberately sits at an extreme; it is time-boxed and watches a
# thermal zone.

set -e

# ${SUDO-sudo}, not ${SUDO:-sudo}: SUDO= (empty) must mean "no sudo", which is
# what you want when running the whole script under sudo already.
SUDO=${SUDO-sudo}
LOG=${LOG:-./focus-probe.log}
DRY=${DRY:-0}
CTRL=focus_absolute

# ── naming ───────────────────────────────────────────────────────────────────
# A deterministic name from a string. Same input, same name, forever.
ADJ="quiet drowsy brisk sullen eager patient restless somber nimble stubborn
     wistful frantic placid brittle sturdy fickle solemn giddy weary keen
     hollow amber crooked velvet rusty gentle rigid molten shallow vivid"
NOUN="lark otter heron badger marten weasel plover grebe shrike vole
      kestrel ibis tapir lemur civet dingo gannet stoat quoll serval
      anvil lantern piston gasket flange ratchet spindle bellows caliper trestle"

name_for() {
    # djb2-ish hash in awk; two independent mixes for the two word lists.
    echo "$1" | awk -v adj="$ADJ" -v noun="$NOUN" '
    {
        h1 = 5381; h2 = 7919
        for (i = 1; i <= length($0); i++) {
            c = index(" !\"#$%&'\''()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz", substr($0, i, 1))
            h1 = (h1 * 33 + c) % 1000003
            h2 = (h2 * 31 + c * 7) % 999983
        }
        na = split(adj, A, /[ \n\t]+/); nn = split(noun, N, /[ \n\t]+/)
        printf "%s_%s", A[(h1 % na) + 1], N[(h2 % nn) + 1]
    }'
}

say()  { printf '%s\n' "$*" | tee -a "$LOG"; }

# ── listening ────────────────────────────────────────────────────────────────
# Every Enter press is timestamped by a background reader on the terminal;
# every step records when it began. Correlating the two afterwards is what
# turns "I heard something" into "quiet_lark produced 6 ticks".
HITS="${LOG%.log}.hits"
MARKS="${LOG%.log}.marks"
READER_PID=""

# Pick a nanosecond-ish clock once. Only differences are ever used, so a
# monotonic source is fine and preferable.
if [ "$(date +%s%N 2>/dev/null | tr -dc 'N')" = "" ] && [ -n "$(date +%s%N 2>/dev/null)" ]; then
    TIMER=date
elif [ -r /proc/uptime ]; then
    TIMER=uptime          # 10 ms resolution, monotonic
else
    TIMER=seconds
fi

now_ns() {
    case "$TIMER" in
        date)   date +%s%N ;;
        uptime) awk '{ printf "%.0f", $1 * 1000000000 }' /proc/uptime ;;
        *)      echo "$(date +%s)000000000" ;;
    esac
}

start_listening() {
    # -r is not enough: /dev/tty exists but fails to open when there is no
    # controlling terminal (cron, a pipe, nohup). Try the open itself.
    ( : < /dev/tty ) 2>/dev/null || { say "(no controlling terminal: Enter-to-mark disabled)"; return; }
    : > "$HITS"; : > "$MARKS"
    ( while IFS= read -r _; do now_ns >> "$HITS"; done < /dev/tty ) &
    READER_PID=$!
}

stop_listening() {
    [ -n "$READER_PID" ] && kill "$READER_PID" 2>/dev/null
    READER_PID=""
}

step() { # step <key> <description>
    NAME=$(name_for "$1")
    [ -n "$READER_PID" ] && printf '%s\t%s\t%s\n' "$(now_ns)" "$NAME" "$2" >> "$MARKS"
    printf '\n\033[1;36m%-16s\033[0m %s\n' "$NAME" "$2" | tee -a "$LOG"
}

# Attribute each keypress to the step that was running when it happened.
report_hits() {
    [ -s "$MARKS" ] || return 0
    printf '%s\n' "" | tee -a "$LOG"
    printf 'what you heard\n' | tee -a "$LOG"
    awk -F'\t' '
        FNR == NR { t[NR] = $1; n[NR] = $2; d[NR] = $3; steps = NR; next }
        {
            for (i = steps; i >= 1; i--) if ($1 >= t[i]) { c[i]++; if (!f[i]) f[i] = ($1 - t[i]) / 1e9; break }
        }
        END {
            for (i = 1; i <= steps; i++)
                if (c[i]) printf "  %-16s %3d tick%s  (first at +%.1fs)  %s\n", n[i], c[i], c[i] == 1 ? "" : "s", f[i], d[i]
            print ""
            for (i = 1; i <= steps; i++) if (!c[i]) printf "  silent: %-16s %s\n", n[i], d[i]
        }' "$MARKS" "$HITS" 2>/dev/null | tee -a "$LOG"
}

# ── device ───────────────────────────────────────────────────────────────────
find_subdev() {
    for s in /dev/v4l-subdev*; do
        [ -e "$s" ] || continue
        if $SUDO v4l2-ctl -d "$s" --list-ctrls 2>/dev/null | grep -q "$CTRL"; then
            echo "$s"
            return 0
        fi
    done
    return 1
}

limits() { # min and max from the driver, not assumed
    # Plain assignments in their own statements: busybox awk rejects the
    # `sub(...) && var=x` form outright and then runs nothing at all.
    $SUDO v4l2-ctl -d "$SUBDEV" --list-ctrls 2>/dev/null \
      | awk -v c="$CTRL" '
        $1 == c {
            m = ""; M = ""
            for (i = 1; i <= NF; i++) {
                if ($i ~ /^min=/) { t = $i; sub("min=", "", t); m = t }
                if ($i ~ /^max=/) { t = $i; sub("max=", "", t); M = t }
            }
            if (m != "" && M != "") { print m, M; exit }
        }'
}

# ── the fast path ────────────────────────────────────────────────────────────
# Each v4l2-ctl invocation is a fork+exec, so a waveform played that way tops
# out around 50 Hz — right in the region a VCM resonance is likely to sit.
# When python3 is present, play waveforms through VIDIOC_S_CTRL directly on an
# open fd instead: sub-millisecond per sample, and real timing.
have_fast() { [ "$DRY" != "1" ] && command -v python3 >/dev/null 2>&1; }

# play_samples <dt_seconds> <<< one integer per line
play_samples() {
    dt=$1
    if have_fast; then
        $SUDO python3 -c '
import sys, fcntl, struct, time, ctypes
dev, dt = sys.argv[1], float(sys.argv[2])
VIDIOC_S_CTRL = 0xc008561b            # _IOWR("V", 27, struct v4l2_control)
CID = 0x009a090a                      # V4L2_CID_FOCUS_ABSOLUTE
vals = [int(x) for x in sys.stdin.read().split()]
fd = open(dev, "rb+", buffering=0)
t = time.perf_counter()
for v in vals:
    fcntl.ioctl(fd, VIDIOC_S_CTRL, struct.pack("Ii", CID, v))
    t += dt
    d = t - time.perf_counter()
    if d > 0: time.sleep(d)
' "$SUBDEV" "$dt"
    else
        while read -r v; do
            [ -n "$v" ] || continue
            set_focus "$v"
            sleep "$dt" 2>/dev/null || true
        done
    fi
}

set_focus() {
    if [ "$DRY" = "1" ]; then
        printf '   [dry] %s=%s\n' "$CTRL" "$1"
    else
        $SUDO v4l2-ctl -d "$SUBDEV" --set-ctrl="$CTRL=$1" >/dev/null 2>&1 || true
    fi
}

get_focus() {
    [ "$DRY" = "1" ] && { echo "$1"; return; }
    $SUDO v4l2-ctl -d "$SUBDEV" --get-ctrl="$CTRL" 2>/dev/null | sed 's/.*: *//'
}

park() { set_focus "$MIN"; }

temp_now() { # the warmest camera-ish thermal zone, in C, or empty
    for z in /sys/class/thermal/thermal_zone*; do
        t=$(cat "$z/type" 2>/dev/null) || continue
        case "$t" in *cam*|*skin*|*quiet*) v=$(cat "$z/temp" 2>/dev/null) || continue
            echo "$t $((v / 1000))" ;;
        esac
    done | sort -k2 -n | tail -1
}

# Who is holding a camera, one "pid comm" per line. Note that a media node
# open is not the same as a stream running: pipewire and libcamera keep nodes
# open just to enumerate. That is why this warns and names the holder rather
# than refusing outright.
camera_holders() {
    for n in ${NODES:-/dev/video* /dev/media*}; do
        [ -e "$n" ] || continue
        for pid in $($SUDO fuser "$n" 2>/dev/null); do
            case "$pid" in *[!0-9]*) continue ;; esac
            c=$(cat /proc/$pid/comm 2>/dev/null) || continue
            echo "$pid $c $n"
        done
    done | sort -u
}

# ── tests ────────────────────────────────────────────────────────────────────
t_range() {
    say "== range: where the lens actually moves, and where it does nothing"
    say "   Listen. Note the first value where you hear it leave rest, and the"
    say "   value past which nothing further happens."
    for v in $(seq_vals 0 "$MAX" 8); do
        step "range:$v" "go to $v"
        set_focus "$v"
        sleep 1
        say "   reads back $(get_focus "$v")"
    done
    park
}

t_steps() {
    say "== steps: the smallest move you can hear, from rest and from midpoint"
    for base in "$MIN" $((MAX / 2)); do
        for d in 8 16 32 64 128 256; do
            step "step:$base+$d" "from $base, jump $d"
            set_focus "$base"; sleep 0.4
            set_focus $((base + d)); sleep 0.6
        done
    done
    park
}

t_sweep() {
    say "== sweep: square waves. A VCM has a mechanical resonance; one of these"
    say "   will be clearly the loudest. That period is the one worth using."
    AMP=$((MAX / 4))
    MID=$((MAX / 2))
    for period_ms in 200 120 80 50 33 25 16 12 8; do
        step "sweep:${period_ms}ms:$AMP" "square wave, period ${period_ms} ms (~$((1000 / period_ms)) Hz), amplitude $AMP"
        # Through play_samples rather than a fork per edge: at 8 ms a
        # fork-per-edge loop cannot keep the period it claims.
        RATE=0.002; have_fast || RATE=0.02
        n=$(awk -v p="$period_ms" -v r="$RATE" 'BEGIN { n = int(p / 1000 / r); print n < 2 ? 2 : n }')
        reps=$((1500 / period_ms))
        i=0
        while [ $i -lt $reps ]; do samples square "$MID" "$AMP" "$n"; i=$((i + 1)); done | play_samples "$RATE"
        set_focus "$MID"; sleep 0.3
    done
    park
}

# ── waveforms ────────────────────────────────────────────────────────────────
# A square wave is a bad instrument for this: it asks for an instantaneous
# jump the actuator cannot make, and dw9719 parts apply slew shaping that
# smooths exactly that transient. The shapes below differ in how they ARRIVE,
# which is what decides whether the lens ticks, hums, or moves silently.
#
# samples <shape> <mid> <amp> <n> — n integers, one per line, one period.
samples() {
    awk -v shape="$1" -v mid="$2" -v amp="$3" -v n="$4" -v lo="$MIN" -v hi="$MAX" '
    function clamp(v) { return v < lo ? lo : (v > hi ? hi : int(v + 0.5)) }
    BEGIN {
        pi = 3.14159265358979
        for (i = 0; i < n; i++) {
            p = i / n
            if      (shape == "square")   v = mid + (p < 0.5 ? -amp/2 : amp/2)
            else if (shape == "sine")     v = mid + (amp/2) * sin(2*pi*p)
            else if (shape == "triangle") v = mid + amp * (p < 0.5 ? (p*2 - 0.5) : (1.5 - p*2))
            else if (shape == "saw")      v = mid - amp/2 + amp*p
            else if (shape == "decay")    v = mid + (amp/2) * sin(2*pi*p*4) * (1 - p)
            else if (shape == "dither")   v = mid + (i % 2 ? amp/2 : -amp/2)
            else                           v = mid
            print clamp(v)
        }
    }'
}

t_curves() {
    say "== curves: the same excursion, arrived at different ways. A square"
    say "   demands a step the actuator cannot make and the driver smooths it;"
    say "   a triangle or sine may be quieter, or may hum instead of tick."
    MID=$((MAX / 2)); AMP=$((MAX / 2))
    RATE=0.002   # 500 Hz sample rate on the fast path
    have_fast || { say "   (no python3: falling back to v4l2-ctl per sample, ~50 Hz ceiling —"; say "    the higher-frequency steps below will not be played at their stated rate)"; RATE=0.02; }
    for shape in square sine triangle saw decay dither; do
        for period_ms in 100 50 25; do
            n=$(awk -v p="$period_ms" -v r="$RATE" 'BEGIN { n = int(p / 1000 / r); print n < 4 ? 4 : n }')
            step "curve:$shape:${period_ms}ms" "$shape, period ${period_ms} ms (~$((1000 / period_ms)) Hz), amplitude $AMP, $n samples/period"
            reps=$((1200 / period_ms))
            i=0
            while [ $i -lt $reps ]; do samples "$shape" "$MID" "$AMP" "$n"; i=$((i + 1)); done | play_samples "$RATE"
            set_focus "$MID"; sleep 0.4
        done
    done
    park
}

t_chirp() {
    say "== chirp: one continuous glide from slow to fast. Whatever frequency"
    say "   the lens is loudest at, you will hear it pass through. Say when."
    MID=$((MAX / 2)); AMP=$((MAX / 2)); RATE=0.002
    have_fast || { say "   (no python3 — chirp needs the fast path to be meaningful; skipping)"; return; }
    for span in "5 40" "40 120"; do
        set -- $span
        step "chirp:$1-$2" "glide $1 Hz to $2 Hz over 8 s, amplitude $AMP"
        awk -v f0="$1" -v f1="$2" -v mid="$MID" -v amp="$AMP" -v r=0.002 -v dur=8 -v lo="$MIN" -v hi="$MAX" '
        BEGIN {
            pi = 3.14159265358979; n = dur / r; ph = 0
            for (i = 0; i < n; i++) {
                t = i / n
                f = f0 + (f1 - f0) * t
                ph += 2 * pi * f * r
                v = mid + (amp/2) * sin(ph)
                print v < lo ? lo : (v > hi ? hi : int(v + 0.5))
            }
        }' | play_samples "$RATE"
        set_focus "$MID"; sleep 0.5
    done
    park
}

t_hold() {
    say "== hold: a spring-return VCM burns current to STAY put. This is the"
    say "   one test that sits at an extreme on purpose. Time-boxed."
    for pos in "$MIN" $((MAX / 2)) "$MAX"; do
        step "hold:$pos" "hold at $pos for 30 s, watching temperature"
        before=$(temp_now)
        set_focus "$pos"
        sleep 30
        after=$(temp_now)
        say "   before: ${before:-(no thermal zone)}"
        say "   after:  ${after:-(no thermal zone)}"
    done
    park
    say "   Parked at $MIN. Compare the three deltas: if holding at $MAX costs"
    say "   real degrees, the shell must park the lens when idle."
}

t_rate() {
    say "== rate: how fast a write actually lands. Everything else is capped"
    say "   by this number."
    [ "$TIMER" = uptime ] && say "   (10 ms clock: using the larger batch as the real figure)"
    for n in 20 100 400; do
        step "rate:$n" "$n alternating writes"
        start=$(date +%s%N 2>/dev/null || echo 0)
        i=0
        while [ $i -lt $n ]; do
            if [ $((i % 2)) -eq 0 ]; then set_focus "$MIN"; else set_focus $((MAX / 2)); fi
            i=$((i + 1))
        done
        end=$(date +%s%N 2>/dev/null || echo 0)
        if [ "$start" != 0 ]; then
            # A zero delta means the batch finished inside one clock tick, not
            # that writes are free. Say which it is rather than printing inf.
            say "   $(awk -v s="$start" -v e="$end" -v n="$n" -v tm="$TIMER" 'BEGIN {
                d = e - s
                if (d <= 0) { printf "under this clocks resolution (%s): try a larger batch", tm }
                else        { printf "%.3f ms per write, %.0f Hz ceiling", d/1e6/n, n/(d/1e9) }
            }')"
        fi
    done
    park
}

seq_vals() { # start end count — integer steps, no seq dependency on floats
    awk -v a="$1" -v b="$2" -v n="$3" 'BEGIN { for (i = 0; i <= n; i++) printf "%d\n", a + (b - a) * i / n }'
}

# ── notes ────────────────────────────────────────────────────────────────────
t_note() {
    [ -n "$2" ] || { echo "usage: $0 note <name> \"what you observed\"" >&2; exit 1; }
    printf '%s\t%s\t%s\n' "$(date -Is)" "$1" "$2" >> "${LOG%.log}.notes"
    echo "noted against $1"
}

t_hits() { report_hits; }

t_notes() {
    [ -f "${LOG%.log}.notes" ] || { echo "(no notes yet)"; return; }
    awk -F'\t' '{ printf "%-16s %s\n", $2, $3 }' "${LOG%.log}.notes"
}

# ── main ─────────────────────────────────────────────────────────────────────
CMD=${1:-all}

case "$CMD" in
    note)  shift; t_note "$@"; exit 0 ;;
    notes) t_notes; exit 0 ;;
    hits)  LOG=${LOG}; HITS="${LOG%.log}.hits"; MARKS="${LOG%.log}.marks"; report_hits; exit 0 ;;
    name)  shift; name_for "$1"; echo; exit 0 ;;
esac

command -v v4l2-ctl >/dev/null 2>&1 || { echo "v4l2-ctl not found: apk add v4l-utils" >&2; exit 1; }

SUBDEV=${SUBDEV:-$(find_subdev || true)}
[ -n "$SUBDEV" ] || { echo "no subdev exposes $CTRL (checked /dev/v4l-subdev*)" >&2; exit 1; }

set -- $(limits)
MIN=${1:-0}; MAX=${2:-1023}

HOLDERS=$(camera_holders)
if [ -n "$HOLDERS" ]; then
    echo "these processes hold a camera node:" >&2
    echo "$HOLDERS" | sed 's/^/  /' >&2
    if [ "$FORCE" = "1" ]; then
        echo "FORCE=1: continuing. If one of them is actually streaming, its frames will be smeared." >&2
    else
        echo "" >&2
        echo "Moving the lens during an exposure smears the focal plane, so this stops here." >&2
        echo "  - if one is a camera app, close it" >&2
        echo "  - pipewire/libcamera often hold nodes open WITHOUT streaming; in that case" >&2
        echo "    re-run with FORCE=1 $0 $CMD" >&2
        exit 1
    fi
fi

trap 'echo; echo "parking lens at $MIN"; stop_listening; DRY=$DRY park 2>/dev/null; report_hits; exit 130' INT TERM

say "focus-probe: $SUBDEV, $CTRL range $MIN..$MAX, log $LOG"
say "clock: $TIMER$([ "$TIMER" = seconds ] && echo ' (1 s resolution — rate figures will be useless)')"
have_fast && say "writer: ioctl fast path (python3)" || say "writer: v4l2-ctl per sample (~50 Hz ceiling)"
say "started $(date -Is)${DRY:+ }$([ "$DRY" = 1 ] && echo '(DRY RUN — no writes)')"
say ""
say "  >>> Hit ENTER every time you hear the lens. Each press is timestamped"
say "      and attributed to the step that was running. Ctrl-C is safe: it"
say "      parks the lens and still prints the report."
say ""
start_listening
park

case "$CMD" in
    find)  say "found $SUBDEV, range $MIN..$MAX" ;;
    range) t_range ;;
    steps) t_steps ;;
    sweep) t_sweep ;;
    hold)  t_hold ;;
    rate)  t_rate ;;
    curves) t_curves ;;
    chirp) t_chirp ;;
    all)   t_rate; t_range; t_steps; t_sweep; t_curves; t_chirp; t_hold ;;
    *)     echo "unknown: $CMD (find range steps sweep curves chirp hold rate all note notes hits name)" >&2; exit 1 ;;
esac

park
stop_listening
report_hits
say ""
say "done. Lens parked at $MIN."
say "Add detail to any step:   $0 note <name> \"what happened\""
say "Re-print the report:      $0 hits"
