#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Tests of tools/get.sh against a fake GitHub (tools/shelltest/fakegithub.rs): checksum verification, the refusals (mismatch, unlisted file, missing list, wrong version, not a
# TrueSense release), version pinning and discovery, the attestation check. get.sh always runs with TRUESENSE_GET_ONLY=1: it verifies and unpacks, it never installs.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
GET=$ROOT/tools/get.sh
REL=Order52/truesense-rust/releases/download

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
    mkdir -p "$doc/Order52/truesense-rust/archive/refs/tags"; cp "$doc/$REL/v$v/truesense-$v.tar.gz" "$doc/Order52/truesense-rust/archive/refs/tags/v$v.tar.gz"      # the old fallback source is there: it must not be used
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
  mkdir -p "$doc/repos/Order52/truesense-rust"
  printf '[\n  {\n    "name": "v0.5.0"\n  },\n  {\n    "name": "v0.9.0"\n  },\n  {\n    "name": "v0.3.15"\n  },\n  {\n    "name": "junk"\n  },\n  {\n    "name": "v0.10.0"\n  }\n]\n' > "$doc/repos/Order52/truesense-rust/tags"
  run_get "$doc" ""
  assert_code 0; assert_contains "$R_OUT" "TrueSense 0.10.0"
}

t_an_unreachable_tag_list_says_how_to_pin_a_version() {
  local doc; doc=$(tmpdir); mkdir -p "$doc/repos/Order52/truesense-rust"; echo '[]' > "$doc/repos/Order52/truesense-rust/tags"
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
  local w; for w in "attestation verify" ts.tar.gz "--repo Order52/truesense-rust"; do assert_contains "$args" "$w" "gh arguments"; done

  bad=$(tmpdir); fake_program "$bad" gh "exit 1"
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$bad:$PATH"
  assert_code 1; assert_contains "$R_ERR" "does not verify"; assert_contains "$R_ERR" refusing

  # without gh at all: curl, tar and the coreutils the script uses, but no gh
  nogh=$(path_with curl tar sha256sum sed awk sort head tail mktemp rm id tr cut cat)
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$nogh"
  assert_code 1; assert_contains "$R_ERR" "needs the GitHub CLI"
}

# ---- the optional prebuilt Qt window (truesense-qt-X.Y.Z-linux-x86_64.tar.gz) ---------------------------------------------------------------

# publish_qt DOC VERSION [list=no|wrong] [empty] [missing]: the Qt tarball next to the bundle, listed in SHA256SUMS (list=no leaves it out, list=wrong lists another hash), empty (no
# binary inside), missing (listed but not served)
publish_qt() {
  local doc=$1 v=$2 o list=yes empty=0 missing=0 d asset hash; shift 2
  for o in "$@"; do case $o in list=*) list=${o#list=};; empty) empty=1;; missing) missing=1;; esac; done
  asset=$doc/$REL/v$v/truesense-qt-$v-linux-x86_64.tar.gz
  d=$(tmpdir); mkdir -p "$d/truesense-qt-$v/bin"; [ "$empty" = 1 ] || printf '#!/bin/sh\necho "truesense-qt %s"\n' "$v" > "$d/truesense-qt-$v/bin/truesense-qt"
  chmod 755 "$d/truesense-qt-$v/bin"/* 2>/dev/null || true
  tar -czf "$asset" -C "$d" "truesense-qt-$v"
  hash=$(sha256sum "$asset" | cut -d' ' -f1); [ "$list" != wrong ] || hash=$(printf '%064d' 7)
  [ "$list" = no ] || printf '%s  truesense-qt-%s-linux-x86_64.tar.gz\n' "$hash" "$v" >> "$doc/$REL/v$v/SHA256SUMS"
  [ "$missing" = 0 ] || rm -f "$asset"
}

t_a_listed_qt_window_is_verified_and_added_to_the_bundle() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 right; publish_qt "$doc" 0.5.0
  run_get "$doc" 0.5.0
  assert_code 0; assert_contains "$R_OUT" "prebuilt Qt window verified and added (bin/truesense-qt"; assert_contains "$R_OUT" "verified and unpacked"
}

t_a_release_without_a_qt_window_installs_as_before() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 right
  run_get "$doc" 0.5.0
  assert_code 0; assert_contains "$R_OUT" "this release has no prebuilt Qt window"; assert_contains "$R_OUT" "web window"
  publish_qt "$doc" 0.5.0 list=no
  run_get "$doc" 0.5.0
  assert_code 0; assert_contains "$R_OUT" "has no prebuilt Qt window" "an unlisted file is not fetched"
}

t_a_qt_window_that_cannot_be_downloaded_is_skipped_but_one_that_does_not_match_its_list_is_refused() {
  local doc; doc=$(tmpdir); publish "$doc" 0.5.0 right; publish_qt "$doc" 0.5.0 missing
  run_get "$doc" 0.5.0
  assert_code 0; assert_contains "$R_OUT" "could not be downloaded: continuing without it"
  doc=$(tmpdir); publish "$doc" 0.5.0 right; publish_qt "$doc" 0.5.0 list=wrong
  run_get "$doc" 0.5.0
  assert_code 1; assert_contains "$R_ERR" "checksum mismatch for truesense-qt-0.5.0-linux-x86_64.tar.gz"; assert_contains "$R_ERR" "refusing"
  doc=$(tmpdir); publish "$doc" 0.5.0 right; publish_qt "$doc" 0.5.0 empty
  run_get "$doc" 0.5.0
  assert_code 1; assert_contains "$R_ERR" "does not hold truesense-qt-0.5.0/bin/truesense-qt"
}

t_no_qt_is_honoured_and_the_qt_window_gets_the_same_attestation_check() {
  local doc good log bad; doc=$(tmpdir); publish "$doc" 0.5.0 right; publish_qt "$doc" 0.5.0
  run_get "$doc" 0.5.0 TRUESENSE_NO_QT=1
  assert_code 0; assert_contains "$R_OUT" "not fetching the prebuilt Qt window"; assert_not_contains "$R_OUT" "verified and added"
  good=$(tmpdir); log=$(tmpdir)/gh.log; fake_program "$good" gh "echo \"\$*\" >> '$log'"
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$good:$PATH"
  assert_code 0; assert_contains "$(cat "$log")" "qt.tar.gz"; assert_contains "$R_OUT" "prebuilt Qt window verified and added"
  bad=$(tmpdir); fake_program "$bad" gh "case \"\$*\" in *qt.tar.gz*) exit 1;; esac"
  run_get "$doc" 0.5.0 TRUESENSE_VERIFY_ATTESTATION=1 "PATH=$bad:$PATH"
  assert_code 1; assert_contains "$R_ERR" "truesense-qt-0.5.0-linux-x86_64.tar.gz does not verify"
}

run_tests "$@"
