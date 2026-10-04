#!/bin/sh
# Sample the things that show a governor/thermal problem once a second to a CSV. Run on the phone.
# Usage: sample.sh [seconds] [out.csv]     (default 120 s, stress.csv)
# Columns: t, cpu_mhz (min/max over cores), hottest_c, cpu_psi_some10, mem_psi_some10, batt_ua, load1
SECS=${1:-120}; OUT=${2:-stress.csv}
echo "t,cpu_min_mhz,cpu_max_mhz,hottest_c,cpu_psi10,mem_psi10,batt_ua,load1" > "$OUT"
i=0
while [ "$i" -lt "$SECS" ]; do
  f=$(cat /sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_cur_freq 2>/dev/null | sort -n | sed -n '1p;$p' | tr '\n' ' ')
  set -- $f; lo=$(( ${1:-0} / 1000 )); hi=$(( ${2:-0} / 1000 ))
  tmax=0; for z in /sys/class/thermal/thermal_zone*/temp; do v=$(cat "$z" 2>/dev/null) || continue; [ "${v:-0}" -gt "$tmax" ] && tmax=$v; done
  cp=$(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu); mp=$(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/memory)
  b=$(cat /sys/class/power_supply/*/current_now 2>/dev/null | head -n 1)
  echo "$i,$lo,$hi,$((tmax / 1000)),$cp,$mp,${b:-},$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
  i=$((i + 1)); sleep 1
done
echo "wrote $OUT"
