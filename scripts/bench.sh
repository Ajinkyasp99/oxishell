#!/usr/bin/env bash
# Measure startup time, idle CPU and memory of oxishell (Phase 11).
# Usage: scripts/bench.sh [path/to/bin-dir]   (default: target/release)
# Needs a display; under CI use: xvfb-run -a scripts/bench.sh
set -euo pipefail
BIN=${1:-target/release}
APP=$BIN/oxishell; CLI=$BIN/oxi
TMP=$(mktemp -d /tmp/oxishell-bench.XXXXXX); trap 'kill ${PID:-} ${spid:-} 2>/dev/null || true; rm -rf "$TMP"' EXIT
mkdir -p "$TMP"/{config/oxishell,state,run}; chmod 700 "$TMP/run"
printf '[ipc]\nallow_send = true\n[notifications]\ndesktop = false\n[session]\nrestore = false\n[terminal]\nshell = "/bin/bash"\n' > "$TMP/config/oxishell/config.toml"
export XDG_CONFIG_HOME=$TMP/config XDG_STATE_HOME=$TMP/state XDG_RUNTIME_DIR=$TMP/run OXISHELL_NON_UNIQUE=1 GSETTINGS_BACKEND=memory
unset OXISHELL_SOCKET OXISHELL_PANE_ID
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

echo "# oxishell benchmark — $(date -u +%Y-%m-%dT%H:%MZ)"
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

# --- helper-process launches while idle, in a git repo --------------------------------------------
# Counts processes started by the app (any descendant) over 60 s. Method: strace -f on execve when
# the app can be launched under it (ptrace of own child works even with yama ptrace_scope=1);
# otherwise /proc sampling every 100 ms (may miss very short-lived processes).
REPO=$TMP/repo
mkdir -p "$REPO"
git -C "$REPO" init -q
git -C "$REPO" -c user.name=bench -c user.email=bench@example.com commit -q --allow-empty -m init
count_children() { # $1 = number of panes
  local n=$1 spid have P i lines0 method
  if command -v strace >/dev/null 2>&1 && strace -qq -e trace=execve true >/dev/null 2>&1; then
    method="strace -f execve"
    strace -f -qq -e trace=execve -o "$TMP/exec.$n" "$APP" >/dev/null 2>&1 & spid=$!
    until "$CLI" ping >/dev/null 2>&1; do sleep 0.05; done
    PID=
    for i in 1 2 3 4 5 6 7 8 9 10; do
      PID=$(pgrep -P "$spid" -x oxishell | head -1) || true
      [ -n "$PID" ] && break
      sleep 0.2
    done
    if [ -z "$PID" ]; then
      echo "bench: could not find the oxishell process under strace (pid $spid)" >&2
      kill "$spid" 2>/dev/null || true
      exit 1
    fi
  else
    method="/proc sampling (100 ms)"
    "$APP" >/dev/null 2>&1 & PID=$!; spid=$PID
    until "$CLI" ping >/dev/null 2>&1; do sleep 0.01; done
  fi
  P=$("$CLI" --json pane list | python3 -c 'import json,sys;print(json.load(sys.stdin)[0]["id"])')
  "$CLI" pane send --pane "$P" --enter "cd $REPO" >/dev/null; sleep 1
  for i in $(seq 2 "$n"); do "$CLI" pane split -d right >/dev/null; done
  sleep 5  # let startup probes settle; only launches after this point are counted
  if [ "$method" = "strace -f execve" ]; then
    lines0=$(wc -l < "$TMP/exec.$n"); sleep 60
    tail -n +$((lines0 + 1)) "$TMP/exec.$n" | python3 -c '
import re,sys,collections
c=collections.Counter()
for l in sys.stdin:
    m=re.search(r"execve\(\"([^\"]+)\".*\) = 0$", l.strip())
    if m: c[m.group(1).rsplit("/",1)[-1]]+=1
print(sum(c.values()), ", ".join(f"{k}: {v}" for k,v in sorted(c.items(), key=lambda kv:-kv[1])) or "none")' > "$TMP/res.$n"
  else
    python3 - "$PID" 60 > "$TMP/res.$n" <<'PY'
import os,sys,time,collections
root=int(sys.argv[1]); end=time.time()+float(sys.argv[2]); seen={}; base=None
def table():
    t={}
    for d in os.listdir("/proc"):
        if d.isdigit():
            try:
                f=open(f"/proc/{d}/stat").read(); r=f.rsplit(")",1); comm=f[f.index("(")+1:f.rindex(")")]
                x=r[1].split(); t[int(d)]=(int(x[1]),x[19],comm)
            except Exception: pass
    return t
def desc(t):
    keep={root}; ch=True
    while ch:
        ch=False
        for p,(pp,_,_) in t.items():
            if p not in keep and pp in keep: keep.add(p); ch=True
    return keep-{root}
base={(p,t[p][1]) for t in [table()] for p in desc(t)}
while time.time()<end:
    t=table()
    for p in desc(t):
        k=(p,t[p][1])
        if k not in base: seen[k]=t[p][2]
    time.sleep(0.1)
c=collections.Counter(seen.values())
print(sum(c.values()), ", ".join(f"{k}: {v}" for k,v in sorted(c.items(), key=lambda kv:-kv[1])) or "none")
PY
  fi
  local label="$n idle panes"; [ "$n" -eq 1 ] && label="1 idle pane"
  echo "child processes started per minute, $label in a git repo: $(cut -d' ' -f1 "$TMP/res.$n")  [$(cut -s -d' ' -f2- "$TMP/res.$n" | sed 's/^ *//')]  (method: $method)"
  kill -TERM "$PID" 2>/dev/null || true; wait "$spid" 2>/dev/null || true; PID=
}
count_children 1
count_children 4
