#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# The documentation link checker (tools/ci/check-doc-links.sh): it finds a missing file and a missing heading, and leaves
# absolute URLs and fenced code alone. The last test runs it on the real repository, so a broken link fails here as well as in CI.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
CHECK=$ROOT/tools/ci/check-doc-links.sh

tree() {  # a tiny repository in $TMP/repo with the given README text
  mkdir -p "$TMP/repo/docs"
  printf '%s\n' "$1" > "$TMP/repo/README.md"
  printf '# Title\n\n## A heading, with (punctuation)!\n\nbody\n' > "$TMP/repo/docs/A.md"
}

t_good_links_pass() {
  tree $'[a](docs/A.md) [b](docs/A.md#a-heading-with-punctuation) [c](#top) [d](https://example.org/x)\n\n# Top'
  run -- bash "$CHECK" "$TMP/repo"
  assert_code 0; assert_contains "$R_OUT" "ok  doc links (3 checked)"
}

t_a_missing_file_is_reported() {
  tree '[gone](docs/Nope.md)'
  run -- bash "$CHECK" "$TMP/repo"
  assert_code 1; assert_contains "$R_ERR" "BROKEN  README.md: 'docs/Nope.md'"
}

t_a_missing_heading_is_reported() {
  tree '[x](docs/A.md#not-there)'
  run -- bash "$CHECK" "$TMP/repo"
  assert_code 1; assert_contains "$R_ERR" "no heading 'not-there'"
}

t_links_inside_fenced_code_are_not_checked() {
  tree $'```\n[gone](docs/Nope.md)\n```\n[ok](docs/A.md)'
  run -- bash "$CHECK" "$TMP/repo"
  assert_code 0
}

t_links_between_docs_are_resolved_from_the_docs_folder() {
  tree '[a](docs/A.md)'
  printf '[up](../README.md) [sibling](A.md)\n' > "$TMP/repo/docs/B.md"
  run -- bash "$CHECK" "$TMP/repo"
  assert_code 0
}

t_the_real_documentation_has_no_broken_link() {
  run -- bash "$CHECK" "$ROOT"
  assert_code 0
}

run_tests
