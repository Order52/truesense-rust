#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Checks the relative links of README.md and docs/*.md: the file a link points at must exist, and a
# `#fragment` must be a heading of that file (GitHub's slug rule). Links in fenced code blocks and
# absolute URLs are not looked at. A broken link is an error in CI's static job.
#   tools/ci/check-doc-links.sh              the repository this script is in
#   tools/ci/check-doc-links.sh DIR          another tree (the tests use it)
set -Eeuo pipefail
root=$(realpath "${1:-$(dirname "$0")/../..}")
cd "$root"
shopt -s nullglob

# GitHub's anchor for a heading: lower case, everything but letters, digits, space, - and _ removed, spaces to -.
slug() { printf '%s\n' "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9 _-]//g; s/ /-/g'; }

# the headings of a markdown file (outside fenced code), one slug per line
headings() {
  awk '/^```/ { fence = !fence; next } !fence && /^#+ / { sub(/^#+ +/, ""); sub(/ +#+ *$/, ""); print }' "$1" |
    while IFS= read -r h; do slug "$h"; done
}

# the link targets of a markdown file (outside fenced code), one per line: "[text](target)" gives target
targets() {
  awk '/^```/ { fence = !fence; next } !fence' "$1" | grep -oE '\]\([^)[:space:]]+\)' | sed -E 's/^\]\(//; s/\)$//' || true
}

bad=0; checked=0
for file in README.md docs/*.md; do
  dir=$(dirname "$file")
  while IFS= read -r target; do
    [ -n "$target" ] || continue
    case $target in http://* | https://* | mailto:*) continue ;; esac
    checked=$((checked + 1))
    path=${target%%#*}
    fragment=
    case $target in *'#'*) fragment=${target#*#} ;; esac
    if [ -z "$path" ]; then dest=$file; else dest="$dir/$path"; fi
    if [ ! -e "$dest" ]; then
      echo "BROKEN  $file: '$target' (no such file: $dest)" >&2; bad=$((bad + 1)); continue
    fi
    if [ -n "$fragment" ]; then
      case $dest in
        *.md)
          # (into a variable first: `grep -q` closing the pipe early would fail the pipeline under pipefail)
          found=$(headings "$dest")
          printf '%s\n' "$found" | grep -qxF -- "$fragment" || { echo "BROKEN  $file: '$target' (no heading '$fragment' in $dest)" >&2; bad=$((bad + 1)); } ;;
      esac
    fi
  done < <(targets "$file")
done
if [ "$bad" -ne 0 ]; then echo "doc links: $bad broken of $checked checked" >&2; exit 1; fi
echo "ok  doc links ($checked checked)"
