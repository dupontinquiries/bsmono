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
#      SUDO=doas, LOG=path (default ./focus-probe.log).
#
# SAFETY: a spring-return VCM holds position with CONTINUOUS current, so
# resting far from 0 dissipates heat against the lens stack forever. Every
# subcommand parks at 0 on exit, including on Ctrl-C. `hold` is the one test
# that deliberately sits at an extreme; it is time-boxed and watches a
# thermal zone.

set -e

SUDO=${SUDO:-sudo}
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

now_ns() { date +%s%N 2>/dev/null || echo 0; }

start_listening() {
    [ -r /dev/tty ] || { say "(no terminal: Enter-to-mark disabled)"; return; }
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
    $SUDO v4l2-ctl -d "$SUBDEV" --list-ctrls 2>/dev/null \
      | awk -v c="$CTRL" '$1 == c { for (i=1;i<=NF;i++) { if ($i ~ /^min=/) sub("min=","",$i) && m=$i; if ($i ~ /^max=/) sub("max=","",$i) && M=$i } print m, M }'
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

camera_busy() {
    # Anything holding a camera should win over this script.
    if command -v fuser >/dev/null 2>&1; then
        fuser /dev/video* /dev/media0 2>/dev/null | grep -q . && return 0
    fi
    return 1
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
        half=$(awk -v p="$period_ms" 'BEGIN { printf "%.3f", p / 2000 }')
        cycles=$((1500 / period_ms))
        i=0
        while [ $i -lt $cycles ]; do
            set_focus $((MID - AMP / 2)); sleep "$half"
            set_focus $((MID + AMP / 2)); sleep "$half"
            i=$((i + 1))
        done
        set_focus "$MID"; sleep 0.3
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
    for n in 20 100; do
        step "rate:$n" "$n alternating writes"
        start=$(date +%s%N 2>/dev/null || echo 0)
        i=0
        while [ $i -lt $n ]; do
            if [ $((i % 2)) -eq 0 ]; then set_focus "$MIN"; else set_focus $((MAX / 2)); fi
            i=$((i + 1))
        done
        end=$(date +%s%N 2>/dev/null || echo 0)
        if [ "$start" != 0 ]; then
            say "   $(awk -v s="$start" -v e="$end" -v n="$n" 'BEGIN { printf "%.2f ms per write, %.0f Hz ceiling", (e-s)/1e6/n, n/((e-s)/1e9) }')"
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

if camera_busy; then
    echo "a camera is in use — moving the lens now would blur its frames. Stop it first." >&2
    exit 1
fi

trap 'echo; echo "parking lens at $MIN"; stop_listening; DRY=$DRY park 2>/dev/null; report_hits; exit 130' INT TERM

say "focus-probe: $SUBDEV, $CTRL range $MIN..$MAX, log $LOG"
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
    all)   t_rate; t_range; t_steps; t_sweep; t_hold ;;
    *)     echo "unknown: $CMD (find range steps sweep hold rate all note notes hits name)" >&2; exit 1 ;;
esac

park
stop_listening
report_hits
say ""
say "done. Lens parked at $MIN."
say "Add detail to any step:   $0 note <name> \"what happened\""
say "Re-print the report:      $0 hits"
