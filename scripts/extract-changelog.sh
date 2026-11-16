#!/usr/bin/env bash
# Extract the body of a single CHANGELOG.md section, without its
# `## [X.Y.Z] - DATE` header.
#
# Usage: ./scripts/extract-changelog.sh <version>
#   e.g. ./scripts/extract-changelog.sh 0.1.0
#
# Output goes to stdout. Used by the GitHub Release workflow to feed the
# Added/Fixed/Changed/Removed sections into the release body so we don't
# have to babysit auto-generated commit lists.
#
# Assumes Keep-a-Changelog format: each version has a `## [X.Y.Z] - DATE`
# header, and the section ends at the next `## [` heading.

set -euo pipefail

VERSION="${1:?usage: $0 <version>}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHANGELOG="$SCRIPT_DIR/../CHANGELOG.md"

if [[ ! -f "$CHANGELOG" ]]; then
  echo "error: $CHANGELOG not found" >&2
  exit 1
fi

# Print the lines after the `## [VERSION]` header up to (not including) the
# next `## [` heading or end of file, then trim surrounding blank lines.
# Plain string prefix matching avoids regex-escaping the version, and fixes
# the old `sed '1d;$d'` bug that dropped the last body line when the section
# was the final one in the file.
SECTION="$(awk -v hdr="## [${VERSION}]" '
  index($0, hdr) == 1 { on = 1; next }
  on && index($0, "## [") == 1 { exit }
  on { lines[++n] = $0 }
  END {
    s = 1; while (s <= n && lines[s] ~ /^[ \t\r]*$/) s++
    e = n; while (e >= s && lines[e] ~ /^[ \t\r]*$/) e--
    for (i = s; i <= e; i++) print lines[i]
  }
' "$CHANGELOG")"

if [[ -z "$SECTION" ]]; then
  echo "error: no CHANGELOG section found for version $VERSION" >&2
  exit 1
fi

printf '%s\n' "$SECTION"
