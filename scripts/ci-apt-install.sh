#!/usr/bin/env bash
# Install apt packages on a GitHub-hosted Ubuntu runner without stalling on a dead mirror.
#
# Usage: scripts/ci-apt-install.sh <package>...
#
# Hosted runners read Ubuntu's archive through a mirror list (/etc/apt/apt-mirrors.txt) that
# lists azure.archive.ubuntu.com first. When that mirror resolves but does not answer
# (2026-11-11, actions/runner-images#15297), apt waits out every retry on every index file
# before it moves on, and the step appears stuck. So fetch each mirror's InRelease
# first, and give apt only the mirrors that answered. apt still checks every package against
# Ubuntu's archive signing key, whichever mirror served it.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <package>..." >&2
  exit 2
fi

MIRROR_LIST=/etc/apt/apt-mirrors.txt
CANDIDATES=(
  http://azure.archive.ubuntu.com/ubuntu/
  https://archive.ubuntu.com/ubuntu/
  https://mirrors.edge.kernel.org/ubuntu/
)

codename=$(. /etc/os-release && echo "$VERSION_CODENAME")

if grep -rqs "mirror+file:$MIRROR_LIST" /etc/apt/sources.list /etc/apt/sources.list.d/; then
  list=""
  priority=0
  for mirror in "${CANDIDATES[@]}"; do
    if curl -fsS --connect-timeout 5 --max-time 15 -o /dev/null "${mirror}dists/${codename}/InRelease"; then
      priority=$((priority + 1))
      list+="${mirror}"$'\t'"priority:${priority}"$'\n'
      echo "mirror ok: $mirror"
    else
      echo "::warning::Ubuntu mirror $mirror did not answer; skipping it"
    fi
  done
  if [ "$priority" -eq 0 ]; then
    echo "::error::No Ubuntu mirror answered: ${CANDIDATES[*]}"
    exit 1
  fi
  printf '%s' "$list" | sudo tee "$MIRROR_LIST" >/dev/null
else
  echo "apt sources do not use $MIRROR_LIST; leaving mirrors unchanged"
fi

# CI needs no man pages; the man-db trigger is slow on hosted runners.
sudo rm -f /var/lib/man-db/auto-update

APT=(sudo DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get
  -o Acquire::Retries=2
  -o Acquire::http::Timeout=20
  -o Acquire::https::Timeout=20
  -o DPkg::Lock::Timeout=120)

"${APT[@]}" update
"${APT[@]}" install -y --no-install-recommends "$@"
