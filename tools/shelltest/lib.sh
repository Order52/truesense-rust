#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# shellcheck disable=SC2034  # ROOT, BASH_BIN, GH_URL ... are for the test files that source this one
# A small test framework for the shipped shell scripts (the Go edition's tools/shelltest, without Go). A test file sources this, defines functions named t_<what-it-checks>
# and ends with `run_tests`. Every test runs in its own subshell with its own temporary directory ($TMP), which is removed afterwards together with any fake server it started.
#
# The scripts under test are exercised the way a user runs them: through bash, with environment variables that point them at fake os-release files, a fake DMI directory, fake
# programs on PATH and a local fake GitHub. Nothing here installs anything: install.sh is only ever sourced with TRUESENSE_LIB_ONLY=1 (its functions are called one by one) or run
# with --check; get.sh always runs with TRUESENSE_GET_ONLY=1, which verifies and unpacks but never installs; release.sh works on a copy of a tree in $TMP.

# the real repository (the scripts under test live there); it is only read
ROOT=$(realpath "$(dirname "${BASH_SOURCE[0]}")/../..")
BASH_BIN=$(command -v bash)
TESTS_RUN=0; TESTS_FAILED=0

# ---- assertions: a failed one prints why, marks the test failed and lets it go on --------------------------------------------------------------

fail() { printf '      FAIL: %s\n' "$*"; : > "$TMP/.failed"; }
assert_eq() { [ "$1" = "$2" ] || fail "${3:-value}: got '$1', want '$2'"; }
assert_code() { [ "$R_CODE" = "$1" ] || fail "exit code $R_CODE, want $1 (stdout: $R_OUT; stderr: $R_ERR)"; }
assert_contains() { case $1 in *"$2"*) ;; *) fail "${3:-text} lacks '$2': $1";; esac; }
assert_not_contains() { case $1 in *"$2"*) fail "${3:-text} must not contain '$2': $1";; esac; }
assert_not_contains_ci() { local h=${1,,} n=${2,,}; case $h in *"$n"*) fail "${3:-text} must not mention '$2': $1";; esac; }
assert_line() { printf '%s\n' "$1" | grep -qxF -- "$2" || fail "${3:-text} has no line '$2': $1"; }
assert_file() { [ -e "$1" ] || fail "${2:-file} $1 does not exist"; }
assert_no_file() { [ ! -e "$1" ] || fail "${2:-file} $1 must not exist"; }

# ---- running a script ---------------------------------------------------------------------------------------------------------------------

# run [VAR=value ...] -- command args...   runs the command with the inherited environment minus everything the scripts read (TRUESENSE_*, SUDO_USER, PKEXEC_UID), so a
# developer's own settings never leak into a test, plus the given assignments. Sets R_OUT, R_ERR (without the final newline) and R_CODE. A command name is looked up in the
# PATH that is given, so pass absolute names (bash is $BASH_BIN) when the PATH is a restricted one.
run() {
  local -a vars=() clear=(); local k
  while [ "$1" != -- ]; do vars+=("$1"); shift; done; shift
  while IFS= read -r k; do clear+=(-u "$k"); done < <(compgen -e | grep -E '^(TRUESENSE_|SUDO_USER$|PKEXEC_UID$)' || true)
  R_CODE=0
  env ${clear[@]+"${clear[@]}"} ${vars[@]+"${vars[@]}"} "$@" > "$TMP/.out" 2> "$TMP/.err" || R_CODE=$?
  R_OUT=$(cat "$TMP/.out"); R_ERR=$(cat "$TMP/.err")
}

# write_file PATH TEXT [MODE]: creates the file and its directories (the text is written as is, with printf %b escapes expanded)
write_file() { mkdir -p "$(dirname "$1")"; printf '%b' "$2" > "$1"; chmod "${3:-644}" "$1"; }

# fake_program DIR NAME BODY: an executable shell script called NAME in DIR (the body is written as is: no escape processing)
fake_program() { mkdir -p "$1"; printf '#!/bin/sh\n%s\n' "$3" > "$1/$2"; chmod 755 "$1/$2"; }

# path_with NAME...: a directory holding symlinks to the real programs of those names, usable as a whole PATH ("no cargo", "no gh", "no systemctl" without depending on the machine)
path_with() {
  local d n p; d=$(mktemp -d "$TMP/path.XXXXXX")
  for n in "$@"; do p=$(command -v "$n") || { echo "the test needs $n on PATH" >&2; return 1; }; ln -s "$p" "$d/$n"; done
  printf '%s\n' "$d"
}

# tmpdir: a fresh empty directory inside the test's $TMP
tmpdir() { mktemp -d "$TMP/d.XXXXXX"; }

# ---- the fake GitHub ----------------------------------------------------------------------------------------------------------------------

# start_github DOCROOT: serves the files under DOCROOT (a file DOCROOT/a/b answers /a/b; DOCROOT/a/b.status holding a number answers that status with no body; anything else is a
# 404) on 127.0.0.1 and sets GH_URL. It is stopped when the test ends. Needs the compiled helper that run.sh builds (TS_FAKEGITHUB).
start_github() {
  [ -x "${TS_FAKEGITHUB:-}" ] || { fail "the fake GitHub is not built (run the tests through tools/shelltest/run.sh)"; return 1; }
  local port=""; : > "$TMP/.port"
  "$TS_FAKEGITHUB" "$1" > "$TMP/.port" 2>/dev/null & PIDS+=("$!")
  for _ in $(seq 1 100); do port=$(head -n1 "$TMP/.port"); [ -n "$port" ] && break; sleep 0.05; done
  [ -n "$port" ] || { fail "the fake GitHub did not start"; return 1; }
  GH_URL=http://127.0.0.1:$port
}

# skip REASON: ends the test as skipped (it cannot be observed here)
skip() { echo "SKIP: $*"; exit 0; }

# ---- the runner ---------------------------------------------------------------------------------------------------------------------------

# run_tests [PATTERN]: runs every t_ function of the file (those matching PATTERN, if given), in definition order, each in a subshell
run_tests() {
  local fn pat=${1:-}; local -a names=()
  while IFS= read -r fn; do names+=("$fn"); done < <(declare -F | awk '{print $3}' | grep '^t_' | sort -V || true)
  for fn in "${names[@]}"; do
    [ -z "$pat" ] || [[ "$fn" == *$pat* ]] || continue
    TESTS_RUN=$((TESTS_RUN + 1))
    local out; out=$(
      TMP=$(mktemp -d); PIDS=(); export TMP
      cleanup() { local p; for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done; rm -r -f "$TMP"; }
      trap cleanup EXIT
      set +e
      "$fn"; rc=$?
      [ "$rc" = 0 ] || fail "the test function returned $rc"
      [ ! -e "$TMP/.failed" ]
    ) && out_rc=0 || out_rc=1
    if [ "$out_rc" = 0 ] && [[ "$out" == SKIP:* ]]; then printf 'skip  %s (%s)\n' "${fn#t_}" "${out#SKIP: }"
    elif [ "$out_rc" = 0 ]; then printf 'ok    %s\n' "${fn#t_}"; [ -z "$out" ] || printf '%s\n' "$out"
    else printf 'FAIL  %s\n' "${fn#t_}"; [ -z "$out" ] || printf '%s\n' "$out"; TESTS_FAILED=$((TESTS_FAILED + 1)); fi
  done
  printf '%s: %d test(s), %d failed\n' "$(basename "$0")" "$TESTS_RUN" "$TESTS_FAILED"
  [ "$TESTS_FAILED" -eq 0 ]
}
