#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Tests of tools/get.sh against a fake GitHub (tools/shelltest/fakegithub.rs): checksum verification, the refusals (mismatch, unlisted file, missing list, wrong version, not a
# TrueSense release), version pinning and discovery, the attestation check. get.sh always runs with TRUESENSE_GET_ONLY=1: it verifies and unpacks, it never installs.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
GET=$ROOT/tools/get.sh
REL=Order52/truesense/releases/download

# make_bundle VERSION OUTFILE [LAYOUT...]: truesense-VERSION.tar.gz the way the release pipeline lays it out (one top directory).
# LAYOUT: inside=X.Y.Z (the Cargo.toml says another version), noinstall (no tools/install.sh), noversion (no Cargo.toml), python (a 0.3.x tree: the version is in
# src/truesense/__init__.py), go (a 0.4.x tree: the version is in internal/buildinfo/buildinfo.go)
make_bundle() {
  local version=$1 out=$2 inside=$1 noinstall=0 mode=rust o d top; shift 2
  for o in "$@"; do case $o in inside=*) inside=${o#inside=};; noinstall) noinstall=1;; noversion) mode=none;; python) mode=python;; go) mode=go;; esac; done
  d=$(tmpdir); top=$d/truesense-$version; mkdir -p "$top/tools"
  echo TrueSense > "$top/README.md"
  [ "$noinstall" = 1 ] || printf '#!/bin/sh\n' > "$top/tools/install.sh"
  case $mode in
    rust) printf '[workspace]\nresolver = "3"\n\n[workspace.package]\nversion = "%s"\nrust-version = "1.85"\n' "$inside" > "$top/Cargo.toml"; mkdir -p "$top/bin"; echo binary > "$top/bin/truesense";;
    python) mkdir -p "$top/src/truesense"; printf '__version__ = "%s"\n' "$inside" > "$top/src/truesense/__init__.py";;
    go) mkdir -p "$top/internal/buildinfo"; printf 'package buildinfo\n\nconst Version = "%s"\n' "$inside" > "$top/internal/buildinfo/buildinfo.go";;
  esac
  tar -czf "$out" -C "$d" "truesense-$version"
}

# publish DOC VERSION SUMS [LAYOUT...]: the release VERSION on the fake GitHub under DOC. SUMS: right | none | status=N | text=...
publish() {
  local doc=$1 v=$2 sums=$3 asset; shift 3
  asset=$doc/$REL/v$v/truesense-$v.tar.gz
  mkdir -p "$(dirname "$asset")"; make_bundle "$v" "$asset" "$@"
  case $sums in
    none) ;;
    right) printf '%s  truesense-%s.tar.gz\n' "$(sha256sum "$asset" | cut -d' ' -f1)" "$v" > "$doc/$REL/v$v/SHA256SUMS";;
    status=*) printf '%s\n' "${sums#status=}" > "$doc/$REL/v$v/SHA256SUMS.status";;
    text=*) printf '%s' "${sums#text=}" > "$doc/$REL/v$v/SHA256SUMS";;
  esac
}

# run_get DOC [VERSION] [VAR=value ...]: get.sh against the fake GitHub serving DOC ("" = none pinned)
run_get() {
  local doc=$1 v=${2:-}; shift 2 || shift $#
  start_github "$doc" || return 1
  local -a vars=("TRUESENSE_WEB=$GH_URL" "TRUESENSE_API=$GH_URL" TRUESENSE_GET_ONLY=1)
  [ -z "$v" ] || vars+=("TRUESENSE_VERSION=$v")
  run "${vars[@]}" "$@" -- "$BASH_BIN" "$GET"
}

t_a_release_with_a_matching_list_is_verified_and_unpacked() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 right
  run_get "$doc" 0.5.0
  assert_code 0; local w; for w in "TrueSense 0.5.0: downloading" "checksum verified" unpacked; do assert_contains "$R_OUT" "$w" stdout; done
}

t_a_download_that_does_not_match_its_list_is_refused() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 "text=$(printf '%064d' 0)  truesense-0.5.0.tar.gz
"
  run_get "$doc" 0.5.0
  assert_code 1; assert_contains "$R_ERR" "checksum mismatch"; assert_contains "$R_ERR" "refusing to install"
}

t_a_list_that_does_not_name_the_file_is_refused() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 "text=$(printf '%064d' 1)  other.tar.gz
"
  run_get "$doc" 0.5.0
  assert_code 1; assert_contains "$R_ERR" "does not list"
}

t_a_binary_mode_marker_in_the_list_is_accepted() {
  local doc f; doc=$(tmpdir); publish "$doc" 0.5.0 right
  f=$doc/$REL/v0.5.0/SHA256SUMS
  sed -i 's/  truesense-/ *truesense-/' "$f"        # `sha256sum -b` writes "hash *name"
  run_get "$doc" 0.5.0
  assert_code 0
}

# every release is published with a list: a missing one is refused, never skipped (the Go edition fetched very old releases unchecked; there are no old Rust releases)
t_a_release_without_a_list_is_refused_not_skipped() {
  local v doc
  for v in 0.3.15 0.4.0 0.5.0 0.5.1 1.0.0 0.10.0; do
    doc=$(tmpdir); publish "$doc" "$v" none
    mkdir -p "$doc/Order52/truesense/archive/refs/tags"; cp "$doc/$REL/v$v/truesense-$v.tar.gz" "$doc/Order52/truesense/archive/refs/tags/v$v.tar.gz"      # the old fallback source is there: it must not be used
    run_get "$doc" "$v"
    [ "$R_CODE" = 1 ] || fail "$v: exit $R_CODE, want 1"
    assert_contains "$R_ERR" "no SHA256SUMS" "$v"
    assert_not_contains "$R_OUT" "predates" "$v"
  done
}

t_a_server_error_on_the_list_is_an_error_not_a_fallback() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 status=500
  run_get "$doc" 0.5.0
  assert_code 1; assert_contains "$R_ERR" "could not fetch the release checksums"; assert_contains "$R_ERR" "HTTP 500"
}

t_a_bundle_that_says_another_version_is_refused() {
  local name opts doc
  for name in "rust tree|inside=0.4.0" "python tree|python inside=0.3.15" "go tree|go inside=0.4.0" "no version|noversion"; do
    opts=${name#*|}; doc=$(tmpdir)
    # shellcheck disable=SC2086  # the layout words are separate arguments
    publish "$doc" 0.5.0 right $opts
    run_get "$doc" 0.5.0
    [ "$R_CODE" = 1 ] || fail "${name%%|*}: exit $R_CODE, want 1"
    local w; for w in "the download says version" "not 0.5.0" refusing; do assert_contains "$R_ERR" "$w" "${name%%|*}"; done
  done
}

t_a_download_that_is_not_a_truesense_release_is_refused() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 right noinstall
  run_get "$doc" 0.5.0
  assert_code 1; assert_contains "$R_ERR" "not a TrueSense release"
}

t_a_pinned_hash_must_also_match() {
  local doc h; doc=$(tmpdir); publish "$doc" 0.5.0 right
  run_get "$doc" 0.5.0 "TRUESENSE_SHA256=$(printf '%064d' 0 | tr 0 f)"
  [ "$R_CODE" != 0 ] || fail "a wrong pinned hash was accepted: $R_OUT"
  h=$(sha256sum "$doc/$REL/v0.5.0/truesense-0.5.0.tar.gz" | cut -d' ' -f1)
  run_get "$doc" 0.5.0 "TRUESENSE_SHA256=$h"
  assert_code 0
}

t_the_newest_version_is_found_from_the_tags_when_none_is_pinned() {
  local doc; doc=$(tmpdir); publish "$doc" 0.10.0 right
  # GitHub pretty-prints the tag list: one name per line; junk and older tags must not win, and 0.10.0 beats 0.9.0 (version order, not text order)
  mkdir -p "$doc/repos/Order52/truesense"
  printf '[\n  {\n    "name": "v0.5.0"\n  },\n  {\n    "name": "v0.9.0"\n  },\n  {\n    "name": "v0.3.15"\n  },\n  {\n    "name": "junk"\n  },\n  {\n    "name": "v0.10.0"\n  }\n]\n' > "$doc/repos/Order52/truesense/tags"
  run_get "$doc" ""
  assert_code 0; assert_contains "$R_OUT" "TrueSense 0.10.0"
}

t_an_unreachable_tag_list_says_how_to_pin_a_version() {
  local doc; doc=$(tmpdir); mkdir -p "$doc/repos/Order52/truesense"; echo '[]' > "$doc/repos/Order52/truesense/tags"
  run_get "$doc" ""
  assert_code 1; assert_contains "$R_ERR" "could not find the latest version"; assert_contains "$R_ERR" "TRUESENSE_VERSION"
}

t_a_malformed_version_is_refused_before_anything_is_fetched() {
  local v doc; doc=$(tmpdir)
  for v in '1.0; touch /tmp/hacked' 1.0 v1.0.0 1.0.0-rc1 '$(id)'; do
    run_get "$doc" "$v"
    [ "$R_CODE" = 1 ] || fail "$v: exit $R_CODE"
    assert_contains "$R_ERR" "not a version" "$v"
  done
}

t_it_insists_on_root_unless_told_otherwise() {
  [ "$(id -u)" -ne 0 ] || skip "running as root: the refusal cannot be observed"
  run -- "$BASH_BIN" "$GET"          # no TRUESENSE_GET_ONLY: the refusal comes before anything is fetched or run
  assert_code 1; assert_contains "$R_ERR" "run as root"
}

t_it_needs_curl_tar_and_sha256sum() {
  run "PATH=$(path_with id)" TRUESENSE_GET_ONLY=1 -- "$BASH_BIN" "$GET"      # none of the three
  assert_code 1; assert_contains "$R_ERR" "curl is required"
}

# ---- the build provenance check (TRUESENSE_VERIFY_ATTESTATION=1) ----------------------------------------------------------------------------

t_attestation_verification_uses_the_github_cli() {
  local doc good log bad nogh args; doc=$(tmpdir); publish "$doc" 0.5.0 right
  good=$(tmpdir); log=$(tmpdir)/gh.log
  fake_program "$good" gh "echo \"\$*\" >> '$log'"
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$good:$PATH"
  assert_code 0; assert_contains "$R_OUT" "build provenance verified"
  args=$(cat "$log")
  local w; for w in "attestation verify" ts.tar.gz "--repo Order52/truesense"; do assert_contains "$args" "$w" "gh arguments"; done

  bad=$(tmpdir); fake_program "$bad" gh "exit 1"
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$bad:$PATH"
  assert_code 1; assert_contains "$R_ERR" "does not verify"; assert_contains "$R_ERR" refusing

  # without gh at all: curl, tar and the coreutils the script uses, but no gh
  nogh=$(path_with curl tar sha256sum sed awk sort head tail mktemp rm id tr cut cat)
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$nogh"
  assert_code 1; assert_contains "$R_ERR" "needs the GitHub CLI"
}

run_tests "$@"
