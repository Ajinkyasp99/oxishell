#!/usr/bin/env bash
# Install apt packages on a GitHub-hosted Ubuntu runner without stalling on a dead mirror.
#
# Usage: scripts/ci-apt-install.sh <package>...
#
# Hosted runners read Ubuntu's archive through a mirror list (/etc/apt/apt-mirrors.txt) that
# lists azure.archive.ubuntu.com first. When that mirror resolves but does not answer
# (2026-11-11, actions/runner-images#15297), apt waits out every retry on every index file
# before it moves on, and the step appears stuck. So fetch each mirror's InRelease
# first, and give apt only the mirrors that answered. A mirror can also answer that probe and
# then stall on the .deb downloads (2026-11-19: every package "Ign" from azure until the step
# timed out), so the packages are downloaded under a time limit, and a mirror that stalls is
# dropped and the download retried from the next one. apt still checks every package against
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

# CI needs no man pages; the man-db trigger is slow on hosted runners.
sudo rm -f /var/lib/man-db/auto-update

APT=(sudo DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get
  -o Acquire::Retries=1
  -o Acquire::http::Timeout=15
  -o Acquire::https::Timeout=15
  -o DPkg::Lock::Timeout=120)

# Seconds for one mirror to deliver the indexes, then the packages, before it counts as stalled.
UPDATE_LIMIT=${APT_UPDATE_LIMIT:-120}
DOWNLOAD_LIMIT=${APT_DOWNLOAD_LIMIT:-180}

if ! grep -rqs "mirror+file:$MIRROR_LIST" /etc/apt/sources.list /etc/apt/sources.list.d/; then
  echo "apt sources do not use $MIRROR_LIST; leaving mirrors unchanged"
  "${APT[@]}" update
  "${APT[@]}" install -y --no-install-recommends "$@"
  exit 0
fi

mirrors=()
for mirror in "${CANDIDATES[@]}"; do
  if curl -fsS --connect-timeout 5 --max-time 15 -o /dev/null "${mirror}dists/${codename}/InRelease"; then
    mirrors+=("$mirror")
    echo "mirror ok: $mirror"
  else
    echo "::warning::Ubuntu mirror $mirror did not answer; skipping it"
  fi
done

# Each round gives apt the mirrors still in the running, best first; a round that stalls drops
# the first one. Packages already downloaded stay in apt's cache for the next round.
while [ "${#mirrors[@]}" -gt 0 ]; do
  list=""
  for i in "${!mirrors[@]}"; do
    list+="${mirrors[$i]}"$'\t'"priority:$((i + 1))"$'\n'
  done
  printf '%s' "$list" | sudo tee "$MIRROR_LIST" >/dev/null
  echo "apt mirrors: ${mirrors[*]}"

  if timeout "$UPDATE_LIMIT" "${APT[@]}" update &&
    timeout "$DOWNLOAD_LIMIT" "${APT[@]}" install -y --no-install-recommends --download-only "$@"; then
    # Everything is in apt's cache now; this only unpacks and configures.
    "${APT[@]}" install -y --no-install-recommends "$@"
    exit 0
  fi
  echo "::warning::apt via ${mirrors[0]} failed or stalled; retrying without it"
  mirrors=("${mirrors[@]:1}")
done

echo "::error::No Ubuntu mirror delivered the packages: ${CANDIDATES[*]}"
exit 1
