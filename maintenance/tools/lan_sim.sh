#!/bin/bash
# lan_sim.sh -- put N lanbots on a real (namespaced) LAN and watch them meet.
#
# Loopback is not a LAN: one network stack, no broadcast domain, and every
# socket shares one port space. This builds a bridge and gives each player
# its own network namespace with its own address on it -- separate stacks,
# real UDP broadcast -- which is as close to four machines on a switch as one
# box gets. Needs root (CAP_NET_ADMIN) and iproute2.
#
#   maintenance/tools/lan_sim.sh /path/to/lanbot [joiners=3] [secs=7]
set -u
BOT=${1:?path to the lanbot binary}; N=${2:-3}; SECS=${3:-7}
BR=cbbr0; OUT=$(mktemp -d)
cleanup() { for i in $(seq 0 "$N"); do ip netns del "cb$i" 2>/dev/null; done; ip link del "$BR" 2>/dev/null; }
trap cleanup EXIT; cleanup
ip link add "$BR" type bridge && ip link set "$BR" up || { echo "cannot create a bridge (need root)"; exit 2; }
for i in $(seq 0 "$N"); do
  ip netns add "cb$i"
  ip link add "v$i" type veth peer name "p$i"
  ip link set "v$i" master "$BR" up
  ip link set "p$i" netns "cb$i"
  ip -n "cb$i" addr add "10.77.0.$((i+1))/24" brd 10.77.0.255 dev "p$i"
  ip -n "cb$i" link set "p$i" up; ip -n "cb$i" link set lo up
  # 255.255.255.255 needs a route to leave by; a real LAN host has a default one.
  ip -n "cb$i" route add default dev "p$i" 2>/dev/null
done
ip netns exec cb0 "$BOT" host Bramble "$SECS" > "$OUT/0" 2>&1 &
sleep 0.5
NAMES=(x Ash Fox Wren Moss Pike)
for i in $(seq 1 "$N"); do ip netns exec "cb$i" "$BOT" join "${NAMES[$i]}" "$SECS" > "$OUT/$i" 2>&1 & done
wait
for i in $(seq 0 "$N"); do echo "--- 10.77.0.$((i+1))"; cat "$OUT/$i"; done
grep -q "FINAL players=$((N+1)) " "$OUT/0" && echo "PASS: the host counted all $((N+1)) players" || { echo "FAIL: host did not see everyone"; exit 1; }
