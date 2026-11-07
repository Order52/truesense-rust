#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Tests of tools/release.sh (the version table, the notes, `prepare`). They run on fake trees (and fake git repositories) in a temporary directory, with a fake cargo and a fake make
# on PATH: nothing is built, pushed or tagged anywhere that matters. The one test that reads the real repository only reads it.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
RELEASE=$ROOT/tools/release.sh
GITENV=(GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com)

# make_tree VERSION: a fake source tree with every file of the version table at VERSION (prints its path)
make_tree() {
  local v=$1 d; d=$(tmpdir)
  write_file "$d/Cargo.toml" "[workspace]\nresolver = \"3\"\nmembers = [\"crates/truesense\"]\n\n[workspace.package]\nversion = \"$v\"\nrust-version = \"1.85\"\n\n[workspace.dependencies]\nserde = { version = \"1\" }\n"
  write_file "$d/Cargo.lock" "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"abc\"\n\n[[package]]\nname = \"truesense\"\nversion = \"$v\"\ndependencies = [\n \"serde\",\n \"ts-api\",\n]\n\n[[package]]\nname = \"ts-api\"\nversion = \"$v\"\n"
  write_file "$d/docs/INSTALL.md" "Pin a version with \`TRUESENSE_VERSION=$v\`, and see the 0.4.0 notes.\n"
  write_file "$d/kernel/truesense-wmi/dkms.conf" "PACKAGE_NAME=\"truesense-wmi\"\nPACKAGE_VERSION=\"$v\"\n"
  write_file "$d/kernel/truesense-wmi/truesense_wmi.c" "#define TS_VERSION\t\"$v\"\n"
  write_file "$d/tools/aur-publish.sh" "#   tools/aur-publish.sh $v\n"
  write_file "$d/tools/get.sh" "# Environment: TRUESENSE_VERSION=$v (pin a version), TRUESENSE_SHA256=<hash>\n"
  write_file "$d/packaging/arch/PKGBUILD" "pkgname=truesense\npkgver=$v\npkgrel=1\n"
  write_file "$d/packaging/aur/PKGBUILD.template" "pkgname=truesense\npkgver=$v\npkgrel=1\n"
  write_file "$d/RELEASE_NOTES.md" "# Release notes\n\n## Unreleased\n\n- something new\n\n## $v (2026-01-01): first title\n\nOld notes.\n\n- a\n- b\n\n## 0.0.1 (2025-01-01)\n\nAncient.\n"
  printf '%s\n' "$d"
}
rel() { local t=$1; shift; run "TRUESENSE_ROOT=$t" -- "$BASH_BIN" "$RELEASE" "$@"; }

t_the_real_repository_is_consistent() {
  run -- "$BASH_BIN" "$RELEASE" check
  assert_code 0; assert_contains "$R_OUT" "every file agrees"
}

t_the_real_repository_satisfies_the_updater_layout() {
  # ts-system::update Layout::rust_edition(): these three files exist, and the first standalone `version = "..."` of Cargo.toml is the workspace version
  assert_file "$ROOT/tools/install.sh" "installer"; assert_file "$ROOT/kernel/truesense-wmi/truesense_wmi.c" "kernel source"; assert_file "$ROOT/Cargo.toml" "workspace manifest"
  local first want; first=$(awk '{ if (match($0, /(^|[^A-Za-z0-9_-])version[ \t]*=[ \t]*"[^"]+"/)) { s = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", s); sub(/"$/, "", s); print s; exit } }' "$ROOT/Cargo.toml")
  want=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n1)
  assert_eq "$first" "$want" "the version the updater reads"
}

t_a_consistent_tree_passes_and_names_its_version() {
  local t; t=$(make_tree 0.5.0)
  rel "$t" check; assert_code 0; assert_eq "$R_OUT" "version 0.5.0: every file agrees"
  rel "$t" version; assert_eq "$R_OUT" 0.5.0
  rel "$t" check --tag v0.5.0; assert_code 0; assert_contains "$R_OUT" "(tag v0.5.0)"
}

t_a_file_that_is_out_of_step_is_named() {
  local t f; t=$(make_tree 0.5.0)
  for f in docs/INSTALL.md kernel/truesense-wmi/dkms.conf kernel/truesense-wmi/truesense_wmi.c tools/aur-publish.sh tools/get.sh packaging/arch/PKGBUILD packaging/aur/PKGBUILD.template; do
    t=$(make_tree 0.5.0); sed -i -E 's/0\.5\.0/0.4.9/' "$t/$f"
    rel "$t" check
    assert_code 1; assert_contains "$R_ERR" "$f: says 0.4.9, Cargo.toml says 0.5.0" "$f"
  done
}

t_cargo_lock_must_agree_for_the_workspace_crates_only() {
  local t; t=$(make_tree 0.5.0)
  sed -i '/^name = "ts-api"$/{n;s/0\.5\.0/0.4.0/}' "$t/Cargo.lock"
  rel "$t" check
  assert_code 1; assert_contains "$R_ERR" "Cargo.lock: ts-api is 0.4.0, Cargo.toml says 0.5.0"; assert_not_contains "$R_ERR" "serde" "a registry crate has its own version"
  rm "$t/Cargo.lock"; rel "$t" check; assert_code 1; assert_contains "$R_ERR" "Cargo.lock: cannot read"
}

t_a_tag_must_match_the_version() {
  local t; t=$(make_tree 0.5.0)
  rel "$t" check --tag v0.5.1; assert_code 1; assert_contains "$R_ERR" "tag v0.5.1 does not match version 0.5.0"
  rel "$t" check --tag 0.5.0; assert_code 0
  rel "$t" check --tag nonsense; assert_code 1; assert_contains "$R_ERR" "not a version"
}

t_a_missing_mention_is_a_problem_too() {
  local t; t=$(make_tree 0.5.0)
  write_file "$t/tools/get.sh" "# no version here\n"
  rel "$t" check; assert_code 1; assert_contains "$R_ERR" "tools/get.sh: expected 1 version mention(s)"
  t=$(make_tree 0.5.0); printf 'TRUESENSE_VERSION=0.5.0 twice TRUESENSE_VERSION=0.5.0\n' >> "$t/docs/INSTALL.md"
  rel "$t" check; assert_code 1; assert_contains "$R_ERR" "docs/INSTALL.md: expected 1 version mention(s)"
}

t_an_unreadable_file_is_reported_not_fatal() {
  local t; t=$(make_tree 0.5.0); rm "$t/kernel/truesense-wmi/dkms.conf" "$t/tools/get.sh"
  rel "$t" check
  assert_code 1; assert_contains "$R_ERR" "kernel/truesense-wmi/dkms.conf: cannot read"; assert_contains "$R_ERR" "tools/get.sh: cannot read"
}

t_missing_notes_section() {
  local t; t=$(make_tree 0.5.0)
  write_file "$t/RELEASE_NOTES.md" "# Release notes\n\n## 0.0.1 (2025-01-01)\n\nold\n"
  rel "$t" check; assert_code 1; assert_contains "$R_ERR" "RELEASE_NOTES.md: no section for 0.5.0"
  write_file "$t/RELEASE_NOTES.md" "# Release notes\n\n## Unreleased\n\n- x\n"
  rel "$t" check; assert_code 0
  rm "$t/RELEASE_NOTES.md"; rel "$t" check; assert_code 1; assert_contains "$R_ERR" "RELEASE_NOTES.md: cannot read"
}

t_no_version_in_the_source_of_truth_stops_the_check() {
  local t; t=$(make_tree 0.5.0); write_file "$t/Cargo.toml" "[package]\nname = \"x\"\n"
  rel "$t" check; assert_code 1; assert_contains "$R_ERR" "cannot find the workspace version in Cargo.toml"
}

# the updater (ts-system::update Layout::rust_edition) reads the FIRST standalone version = "..." of the top Cargo.toml of a release
t_an_earlier_version_line_in_cargo_toml_would_fool_the_updater_and_is_refused() {
  local t; t=$(make_tree 0.5.0)
  write_file "$t/Cargo.toml" "[workspace]\nmembers = []\n# was: version = \"0.0.1\"\n\n[workspace.package]\nversion = \"0.5.0\"\n"
  rel "$t" check; assert_code 1; assert_contains "$R_ERR" "the first standalone version"; assert_contains "$R_ERR" "'0.0.1'"
  # rust-version before it, a dependency version after it: fine
  write_file "$t/Cargo.toml" "[workspace.package]\nrust-version = \"1.85\"\nversion = \"0.5.0\"\n\n[workspace.dependencies]\nserde = { version = \"1\" }\n"
  rel "$t" check; assert_code 0
}

# ---- notes -----------------------------------------------------------------------------------------------------------------------------

t_the_notes_of_a_version_are_cut_out_with_the_install_footer() {
  local t; t=$(make_tree 0.5.0)
  rel "$t" notes 0.5.0
  assert_code 0
  assert_line "$R_OUT" "TrueSense 0.5.0 for the Acer Predator Helios 16 (PH16-71): first title."
  assert_contains "$R_OUT" $'Old notes.\n\n- a\n- b\n\n**Install / update:**'
  assert_contains "$R_OUT" "tools/get.sh | sudo bash"; assert_contains "$R_OUT" "SHA256SUMS"; assert_contains "$R_OUT" "Rust crates"
  assert_not_contains "$R_OUT" "Ancient" "the next section"; assert_not_contains "$R_OUT" "something new" "Unreleased"
  rel "$t" notes v0.5.0; assert_code 0; assert_contains "$R_OUT" "TrueSense 0.5.0"      # a tag name works too
  rel "$t" notes 9.9.9; assert_code 1; assert_contains "$R_ERR" "has no notes for 9.9.9"
}

t_section_edge_cases() {
  local t; t=$(make_tree 0.5.0)
  # a heading without a date in parentheses is not a version section; a title is optional; the last line of the file as a heading has no body
  write_file "$t/RELEASE_NOTES.md" "## 0.5.0 broken heading\n\nbody\n\n## 0.4.0 (2026-01-01)\n\nno title here\n"
  rel "$t" notes 0.5.0; assert_code 1
  rel "$t" notes 0.4.0; assert_code 0; assert_line "$R_OUT" "TrueSense 0.4.0 for the Acer Predator Helios 16 (PH16-71)."; assert_contains "$R_OUT" "no title here"
  write_file "$t/RELEASE_NOTES.md" "## 0.5.0 (2026-01-01): last\n"
  rel "$t" notes 0.5.0; assert_code 1
  # 0.5.0 must not match 0.5.0.1 or 10.5.0
  write_file "$t/RELEASE_NOTES.md" "## 10.5.0 (2026-01-01)\n\nwrong\n\n## 0.5.0 (2026-01-01)\n\nright\n"
  rel "$t" notes 0.5.0; assert_contains "$R_OUT" right; assert_not_contains "$R_OUT" wrong
}

# ---- prepare -------------------------------------------------------------------------------------------------------------------------

# fakes: a cargo that does what `cargo update --workspace` does to Cargo.lock (the workspace crates take the Cargo.toml version), a make and two scripts the gate runs,
# all logging into $LOG; GATE_FAIL=1 makes the fake make fail
make_fakes() {
  FAKES=$(tmpdir); LOG=$(tmpdir)/log; : > "$LOG"
  cat > "$FAKES/cargo" <<'EOF'
#!/bin/sh
echo "cargo $*" >> '@LOG@'
manifest=$(echo "$*" | sed 's/.*--manifest-path //'); dir=$(dirname "$manifest")
v=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$manifest" | head -n1)
awk -v v="$v" '
  function flush(  i, l) { for (i = 1; i <= n; i++) { l = buf[i]; if (!src && l ~ /^version = "/) l = "version = \"" v "\""; print l } n = 0; src = 0 }
  /^\[\[package\]\]/ { flush(); inpkg = 1 }
  inpkg { buf[++n] = $0; if ($0 ~ /^source = /) src = 1; next }
  { print }
  END { flush() }' "$dir/Cargo.lock" > "$dir/Cargo.lock.new" && mv "$dir/Cargo.lock.new" "$dir/Cargo.lock"
EOF
  sed -i "s|@LOG@|$LOG|" "$FAKES/cargo"; chmod 755 "$FAKES/cargo"
  fake_program "$FAKES" make "echo \"make \$*\" >> '$LOG'; [ \"\${GATE_FAIL:-}\" != 1 ]"
}
# prepare_tree VERSION: a git repository holding a fake tree, the fakes' gate scripts, and (as origin) an empty bare repository
prepare_tree() {
  local t; t=$(make_tree "$1")
  write_file "$t/tools/ci/check-shell.sh" "echo check-shell >> '$LOG'\n"; write_file "$t/tools/shelltest/run.sh" "echo shelltest >> '$LOG'\n"
  git init -q -b main "$t" 2>/dev/null || git init -q "$t"
  git init -q --bare "$t.origin" 2>/dev/null; git -C "$t" remote add origin "$t.origin"
  env "${GITENV[@]}" git -C "$t" add -A; env "${GITENV[@]}" git -C "$t" commit -q -m initial
  printf '%s\n' "$t"
}
prep() { local t=$1; shift; run "TRUESENSE_ROOT=$t" "PATH=$FAKES:$PATH" "TODAY=2026-03-04" "${GITENV[@]}" -- "$BASH_BIN" "$RELEASE" prepare "$@"; }

t_prepare_bumps_retitles_commits_and_tags() {
  make_fakes; local t; t=$(prepare_tree 0.5.0)
  # a stray mention of the old digits that is not in the table must survive the bump
  printf 'Upgrading from 0.5.0 is easy.\n' >> "$t/docs/INSTALL.md"; env "${GITENV[@]}" git -C "$t" commit -q -am stray
  prep "$t" 0.5.1 --title "a short title"
  assert_code 0; local said=$R_OUT
  rel "$t" check; assert_code 0; assert_eq "$R_OUT" "version 0.5.1: every file agrees"
  assert_contains "$(cat "$t/docs/INSTALL.md")" "Upgrading from 0.5.0 is easy." "the stray mention"
  assert_contains "$(cat "$t/docs/INSTALL.md")" 'TRUESENSE_VERSION=0.5.1'
  assert_contains "$(cat "$t/RELEASE_NOTES.md")" "## 0.5.1 (2026-03-04): a short title"; assert_not_contains "$(cat "$t/RELEASE_NOTES.md")" "## Unreleased"
  assert_contains "$(cat "$t/RELEASE_NOTES.md")" "## 0.5.0 (2026-01-01): first title" "older sections stay"
  assert_contains "$(cat "$t/Cargo.lock")" $'name = "serde"\nversion = "1.0.200"' "a registry crate keeps its version"
  assert_eq "$(env "${GITENV[@]}" git -C "$t" log -1 --format=%s)" "Release 0.5.1: a short title" "commit message"
  assert_eq "$(git -C "$t" tag)" "v0.5.1" "tag"; assert_eq "$(git -C "$t" status --porcelain)" "" "clean tree after the commit"
  assert_contains "$(cat "$LOG")" "cargo update --workspace --offline --manifest-path $t/Cargo.toml" "Cargo.lock is refreshed offline"
  assert_contains "$said" "git push origin HEAD v0.5.1" "how to publish"
  assert_eq "$(git -C "$t.origin" tag)" "" "nothing is pushed unless asked"
}

t_the_gate_runs_before_the_commit_and_a_failure_stops_everything() {
  make_fakes; local t; t=$(prepare_tree 0.5.0)
  prep "$t" 0.5.1 --title t; assert_code 0
  assert_eq "$(sed -n '2,$p' "$LOG" | tr '\n' ',')" "make ci,check-shell,shelltest," "the gate: make ci, shell checks, installer tests (after the lock refresh)"
  make_fakes; t=$(prepare_tree 0.5.0)
  run "TRUESENSE_ROOT=$t" "PATH=$FAKES:$PATH" GATE_FAIL=1 "${GITENV[@]}" -- "$BASH_BIN" "$RELEASE" prepare 0.5.1 --title t
  assert_code 1
  assert_eq "$(git -C "$t" log --format=%s | tr '\n' ',')" "initial," "no commit after a failed gate"; assert_eq "$(git -C "$t" tag)" "" "no tag after a failed gate"
  assert_not_contains "$(cat "$LOG")" "shelltest" "the gate stops at the first failure"
  make_fakes; t=$(prepare_tree 0.5.0); prep "$t" 0.5.1 --title t --no-gate
  assert_code 0; assert_not_contains "$(cat "$LOG")" "make ci" "--no-gate"
}

t_a_failed_lock_refresh_is_caught_before_anything_is_committed() {
  make_fakes; local t; t=$(prepare_tree 0.5.0)
  fake_program "$FAKES" cargo "echo \"cargo \$*\" >> '$LOG'"        # a cargo that leaves Cargo.lock as it was
  prep "$t" 0.5.1 --title t
  assert_code 1; assert_contains "$R_ERR" "after bumping"; assert_contains "$R_ERR" "Cargo.lock: truesense is 0.5.0"
  assert_eq "$(git -C "$t" tag)" "" "no tag"
}

t_prepare_refuses_what_would_make_a_bad_release() {
  make_fakes; local t
  t=$(prepare_tree 0.5.0)
  prep "$t" 0.5.0 --title t; assert_code 1; assert_contains "$R_ERR" "0.5.0 is not newer than 0.5.0"
  prep "$t" 0.4.0 --title t; assert_code 1; assert_contains "$R_ERR" "is not newer"
  prep "$t" 0.10.0 --title t --dry-run; assert_code 0                     # 0.10.0 IS newer than 0.5.0 (version order, not text order)
  prep "$t" 0.5.1; assert_code 1; assert_contains "$R_ERR" "give the release a short title"
  prep "$t" 1.0; assert_code 1; assert_contains "$R_ERR" "not a version"
  prep "$t"; assert_code 1; assert_contains "$R_ERR" "usage"
  write_file "$t/RELEASE_NOTES.md" "## Unreleased\n\n\n## 0.5.0 (2026-01-01)\n\nx\n"; env "${GITENV[@]}" git -C "$t" commit -q -am "empty notes"
  prep "$t" 0.5.1 --title t; assert_code 1; assert_contains "$R_ERR" 'no notes under "## Unreleased"'
  t=$(prepare_tree 0.5.0); echo dirty >> "$t/README.md"
  prep "$t" 0.5.1 --title t; assert_code 1; assert_contains "$R_ERR" "uncommitted changes"
  assert_eq "$(git -C "$t" tag)" "" "nothing tagged"; assert_eq "$(sed -n 's/^version = "\(.*\)"$/\1/p' "$t/Cargo.toml" | head -n1)" 0.5.0 "nothing bumped"
}

t_an_existing_tag_is_refused_and_a_dry_run_changes_nothing() {
  make_fakes; local t; t=$(prepare_tree 0.5.0)
  git -C "$t" tag v0.5.1
  prep "$t" 0.5.1 --title t; assert_code 1; assert_contains "$R_ERR" "tag v0.5.1 already exists"
  git -C "$t" tag -d v0.5.1 >/dev/null
  prep "$t" 0.5.1 --title t --dry-run
  assert_code 0; assert_contains "$R_OUT" "release 0.5.0 -> 0.5.1: t"; assert_contains "$R_OUT" "dry run: nothing changed"
  assert_eq "$(git -C "$t" status --porcelain)" "" "the tree"; assert_eq "$(git -C "$t" tag)" "" "tags"; assert_eq "$(cat "$LOG")" "" "nothing was run"
}

t_a_push_only_happens_when_asked() {
  make_fakes; local t; t=$(prepare_tree 0.5.0)
  prep "$t" 0.5.1 --title t --push
  assert_code 0; assert_contains "$R_OUT" "pushed: CI publishes"
  assert_eq "$(git -C "$t.origin" tag)" "v0.5.1" "the tag reached origin"
}

t_the_command_line() {
  local t; t=$(make_tree 0.5.0)
  rel "$t"; assert_code 2; assert_contains "$R_ERR" "release.sh check"
  rel "$t" frobnicate; assert_code 2; assert_contains "$R_ERR" "unknown command frobnicate"
  rel "$t" --help; assert_code 0; assert_contains "$R_OUT" "prepare X.Y.Z"
  rel "$t" check --bogus; assert_code 1; assert_contains "$R_ERR" "unknown argument"
  rel "$t" notes; assert_code 1
  rel "$t" prepare 0.5.1 --bogus; assert_code 1; assert_contains "$R_ERR" "unknown option"
}

run_tests "$@"
