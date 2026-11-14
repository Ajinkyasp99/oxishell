#!/usr/bin/env bash
# Bump oxishell's version everywhere it is recorded, dated today:
#   Cargo.toml, Cargo.lock, the AppStream metainfo <releases>, docs/RELEASE_NOTES.md.
#
# Usage: scripts/bump-version.sh <new-version> "<one-line release summary>"
#   e.g. scripts/bump-version.sh 0.1.3 "Faster startup and a new theme."
#
# Release notes: an "## Unreleased" section is renamed to the new version;
# without one, a new section is added with the summary as its only bullet.
# Run from anywhere; nothing is committed.
set -euo pipefail
cd "$(dirname "$0")/.."

NEW=${1:-}
SUMMARY=${2:-}
if [[ ! $NEW =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || [[ -z $SUMMARY ]]; then
    echo "usage: $0 <major.minor.patch> \"<one-line release summary>\"" >&2
    exit 2
fi
OLD=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
if [[ $NEW == "$OLD" ]]; then
    echo "already at $OLD" >&2
    exit 1
fi
if [[ $(printf '%s\n%s\n' "$OLD" "$NEW" | sort -V | tail -1) != "$NEW" ]]; then
    echo "new version $NEW is lower than current $OLD" >&2
    exit 1
fi
DATE=$(date +%F)

python3 - "$OLD" "$NEW" "$DATE" "$SUMMARY" <<'PY'
import sys
from xml.sax.saxutils import escape

old, new, date, summary = sys.argv[1:]

def replace_once(path, a, b):
    s = open(path, encoding="utf-8").read()
    if a not in s:
        sys.exit(f"{path}: expected text not found: {a!r}")
    open(path, "w", encoding="utf-8").write(s.replace(a, b, 1))

replace_once("Cargo.toml", f'version = "{old}"', f'version = "{new}"')
replace_once("Cargo.lock", f'name = "oxishell"\nversion = "{old}"', f'name = "oxishell"\nversion = "{new}"')
replace_once(
    "data/dev.oxishell.Oxishell.metainfo.xml",
    "  <releases>\n",
    f'  <releases>\n    <release version="{new}" date="{date}">\n'
    f"      <description>\n        <p>{escape(summary)}</p>\n      </description>\n    </release>\n",
)
notes = "docs/RELEASE_NOTES.md"
s = open(notes, encoding="utf-8").read()
if "## Unreleased" in s:
    s = s.replace("## Unreleased", f"## {new} — {date}", 1)
else:
    s = s.replace("# Release notes\n\n", f"# Release notes\n\n## {new} — {date}\n\n- {summary}\n\n", 1)
open(notes, "w", encoding="utf-8").write(s)
PY

echo "oxishell $OLD -> $NEW ($DATE)"
echo "Build the package: scripts/build-deb.sh && sudo apt install --reinstall ./dist/oxishell_${NEW}_*.deb"
