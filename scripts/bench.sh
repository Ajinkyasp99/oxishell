#!/usr/bin/env bash
# Measure startup time, idle CPU and memory of cmux-linux (Phase 11).
# Usage: scripts/bench.sh [path/to/bin-dir]   (default: target/release)
# Needs a display; under CI use: xvfb-run -a scripts/bench.sh
set -euo pipefail
BIN=${1:-target/release}
APP=$BIN/cmux-linux; CLI=$BIN/cmux
TMP=$(mktemp -d /tmp/cmux-bench.XXXXXX); trap 'kill $PID 2>/dev/null || true; rm -rf "$TMP"' EXIT
mkdir -p "$TMP"/{config/cmux-linux,state,run}; chmod 700 "$TMP/run"
printf '[ipc]\nallow_send = true\n[notifications]\ndesktop = false\n[session]\nrestore = false\n[terminal]\nshell = "/bin/bash"\n' > "$TMP/config/cmux-linux/config.toml"
export XDG_CONFIG_HOME=$TMP/config XDG_STATE_HOME=$TMP/state XDG_RUNTIME_DIR=$TMP/run CMUX_NON_UNIQUE=1 GSETTINGS_BACKEND=memory
unset CMUX_SOCKET CMUX_PANE_ID
PID=

now() { date +%s%N; }
rss_kb() { awk '/VmRSS/{r=$2} /RssAnon/{a=$2} END{print r" kB (anon "a" kB)"}' /proc/$1/status; }
cpu_ticks() { awk '{print $14+$15}' /proc/$1/stat; }
tree_rss_kb() { # app + descendant processes (shells); threads are not double-counted
  local total=0 p
  for p in $(ps -e -o pid=,ppid= | awk -v root="$1" '{par[$1]=$2} END{keep[root]=1; changed=1; while(changed){changed=0; for(p in par) if(!(p in keep) && (par[p] in keep)){keep[p]=1; changed=1}} for(p in keep) print p}'); do
    total=$((total + $(awk '/VmRSS/{print $2}' /proc/$p/status 2>/dev/null || echo 0)))
  done; echo $total; }
start() {
  local t0; t0=$(now)
  "$APP" >/dev/null 2>&1 & PID=$!
  until "$CLI" ping >/dev/null 2>&1; do sleep 0.01; done
  STARTMS=$(( ($(now) - t0) / 1000000 ))
}
stop() { kill -TERM $PID; wait $PID 2>/dev/null || true; PID=; }

echo "# cmux-linux benchmark — $(date -u +%Y-%m-%dT%H:%MZ)"
echo "host: $(uname -r), $(nproc) CPUs, display: ${WAYLAND_DISPLAY:+wayland }${DISPLAY:+x11 $DISPLAY}, GSK_RENDERER=${GSK_RENDERER:-default}"
times=()
for i in 1 2 3 4 5; do start; times+=("$STARTMS"); stop; done
sorted=($(printf '%s\n' "${times[@]}" | sort -n))
echo "startup to control-socket ready (ms), 5 runs: ${times[*]}  median=${sorted[2]}"

start; sleep 3
c0=$(cpu_ticks $PID); sleep 10; c1=$(cpu_ticks $PID)
hz=$(getconf CLK_TCK)
echo "idle CPU, 1 pane, 10 s: $(awk -v d=$((c1-c0)) -v hz=$hz 'BEGIN{printf "%.2f%%", d/hz/10*100}')"
echo "RSS, 1 pane: app $(rss_kb $PID), app+shells $(tree_rss_kb $PID) kB"
P=$("$CLI" --json pane list | python3 -c 'import json,sys;print(json.load(sys.stdin)[0]["id"])')
for n in 10 20; do
  have=$("$CLI" --json pane list | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')
  while [ "$have" -lt "$n" ]; do
    if [ $((have % 4)) -eq 0 ]; then "$CLI" workspace new >/dev/null; else "$CLI" pane split -d right >/dev/null; fi
    have=$((have+1))
  done
  sleep 3
  c0=$(cpu_ticks $PID); sleep 10; c1=$(cpu_ticks $PID)
  echo "panes=$n: RSS app $(rss_kb $PID), app+shells $(tree_rss_kb $PID) kB, idle CPU $(awk -v d=$((c1-c0)) -v hz=$hz 'BEGIN{printf "%.2f%%", d/hz/10*100}')"
done
"$CLI" workspace select 1 >/dev/null 2>&1 || true
P=$("$CLI" --json pane list | python3 -c 'import json,sys;print([p for p in json.load(sys.stdin) if p["focused"]][0]["id"])')
t0=$(now); "$CLI" pane send --pane "$P" --enter 'seq 1 1000000; echo BENCH-DONE' >/dev/null
until "$CLI" pane read --pane "$P" --lines 3 | grep -q '^BENCH-DONE'; do sleep 0.05; done
echo "1,000,000 lines (seq) in a visible pane: $(( ($(now) - t0) / 1000000 )) ms"
t0=$(now); for i in $(seq 1 100); do "$CLI" ping >/dev/null; done
echo "control-socket round trip: $(( ($(now) - t0) / 100000 )) µs avg (incl. CLI process start)"
stop
