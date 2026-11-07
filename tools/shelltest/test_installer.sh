#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Tests of tools/install.sh: distribution detection, package names, prerequisites, the three model tiers, the choice of the program to install (and the stale-version refusal),
# the rollback, and the order of the safety-critical steps. The script is only ever sourced with TRUESENSE_LIB_ONLY=1 or run with --check: nothing is installed.
# shellcheck source=tools/shelltest/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
INSTALL=$ROOT/tools/install.sh

# the fake /etc/os-release files and the family the installer must derive from each
declare -A OSREL=(
  [arch]=$'ID=arch\n' [cachy]=$'ID=cachyos\nID_LIKE=arch\n' [manjaro]=$'ID=manjaro\nID_LIKE=arch\n'
  [ubuntu]=$'ID=ubuntu\nID_LIKE=debian\n' [mint]=$'ID=linuxmint\nID_LIKE="ubuntu debian"\n' [debian]=$'ID=debian\n'
  [fedora]=$'ID=fedora\n' [nobara]=$'ID=nobara\nID_LIKE="rhel centos fedora"\n'
  [suse]=$'ID=opensuse-tumbleweed\nID_LIKE="opensuse suse"\n' [weird]=$'ID=gentoo\n'
)
declare -A FAMILY=([arch]=arch [cachy]=arch [manjaro]=arch [ubuntu]=debian [mint]=debian [debian]=debian [fedora]=fedora [nobara]=fedora [suse]=suse [weird]=unknown)

# os_release DISTRO: writes a fake os-release and prints its path
os_release() { local d; d=$(tmpdir); write_file "$d/os-release" "${OSREL[$1]}"; printf '%s\n' "$d/os-release"; }

# install_lib [VAR=value ...] -- SCRIPT [ARGS...]: sources install.sh (functions only) and runs SCRIPT with ARGS as $1...
install_lib() {
  local -a vars=(); while [ "$1" != -- ]; do vars+=("$1"); shift; done; shift
  local script=$1; shift
  run ${vars[@]+"${vars[@]}"} TRUESENSE_LIB_ONLY=1 -- "$BASH_BIN" -c "source \"\$0\"; $script" "$INSTALL" "$@"
}

t_family_detection() {
  local d f
  for d in "${!OSREL[@]}"; do
    f=$(os_release "$d")
    install_lib "TRUESENSE_OS_RELEASE=$f" -- 'detect_os; echo "$FAMILY"'
    assert_eq "$R_OUT" "${FAMILY[$d]}" "family of $d"
  done
}

t_a_missing_os_release_is_an_unknown_family_not_an_error() {
  install_lib TRUESENSE_OS_RELEASE=/nonexistent/os-release -- 'detect_os; echo "$OS_ID $FAMILY"'
  assert_code 0; assert_eq "$R_OUT" "unknown unknown"
}

t_header_packages() {
  local row name distro kernel want
  for row in "ubuntu|ubuntu|6.1.0-test|linux-headers-6.1.0-test" "fedora|fedora|6.1.0-test|kernel-devel-6.1.0-test" \
             "suse default|suse|6.1.0-1-default|kernel-default-devel" "suse other flavour|suse|6.1.0-1-azure|kernel-azure-devel" \
             "arch without pkgbase|arch|6.1.0-test|linux-headers" "unknown|weird|6.1.0-test|the kernel headers for 6.1.0-test"; do
    IFS='|' read -r name distro kernel want <<<"$row"
    install_lib "TRUESENSE_OS_RELEASE=$(os_release "$distro")" "TRUESENSE_KERNEL=$kernel" -- 'detect_os; headers_pkg'
    assert_eq "$R_OUT" "$want" "headers_pkg ($name)"
  done
}

# Nothing in the installer needs Python, PyQt, socat or a language runtime: the program is one static binary.
t_package_names_have_no_python_qt_or_socat() {
  declare -A want_arch=([dkms]=dkms [make]=make [cc]=gcc [clang]="clang llvm lld" [audio]=noise-suppression-for-voice)
  declare -A want_other=([dkms]=dkms [make]=make [cc]=gcc [clang]="clang lld" [audio]="")
  local d fam w
  for d in "${!OSREL[@]}"; do
    fam=${FAMILY[$d]}; [ "$fam" != unknown ] || continue
    install_lib "TRUESENSE_OS_RELEASE=$(os_release "$d")" -- 'detect_os; for w in dkms make cc clang audio python gui socat pyqt rust cargo; do echo "$w=$(pkg "$w")"; done'
    for w in dkms make cc clang audio; do
      if [ "$fam" = arch ]; then assert_line "$R_OUT" "$w=${want_arch[$w]}" "$d: pkg $w"; else assert_line "$R_OUT" "$w=${want_other[$w]}" "$d: pkg $w"; fi
    done
    for w in python gui socat pyqt rust cargo; do assert_line "$R_OUT" "$w=" "$d: pkg $w (that prerequisite does not exist)"; done
  done
}

t_prerequisites_are_the_module_toolchain_only() {
  local f; f=$(os_release arch)
  # nothing installed: the toolchain for the kernel module and the headers, and systemd is a problem a package cannot fix
  install_lib "PATH=$(tmpdir)" "TRUESENSE_OS_RELEASE=$f" TRUESENSE_KERNEL=6.1.0-test -- 'detect_os; check_prereqs; echo "MISSING=${MISSING[*]}"; printf "PROBLEM=%s\n" "${PROBLEMS[@]}"'
  assert_line "$R_OUT" "MISSING=dkms make gcc linux-headers" "nothing installed"
  assert_contains "$R_OUT" "PROBLEM=systemd is required" "a system without systemd must be a problem"
  # the toolchain present: only the headers of the (fake) kernel are left, and no systemd problem
  local bin n; bin=$(tmpdir)
  for n in systemctl dkms make gcc; do fake_program "$bin" "$n" "exit 0"; done
  install_lib "PATH=$bin" "TRUESENSE_OS_RELEASE=$f" TRUESENSE_KERNEL=6.1.0-test -- 'detect_os; check_prereqs; echo "MISSING=${MISSING[*]}"; echo "PROBLEMS=${#PROBLEMS[@]}"'
  assert_line "$R_OUT" "MISSING=linux-headers" "with the toolchain present"
  assert_line "$R_OUT" "PROBLEMS=0"
  for n in python PyQt socat cargo rustc; do assert_not_contains_ci "$R_OUT$R_ERR" "$n" "the prerequisite check"; done
}

t_check_runs_without_root_and_reports_only_things_a_package_can_fix() {
  run "TRUESENSE_OS_RELEASE=$(os_release ubuntu)" -- "$BASH_BIN" "$INSTALL" --check
  [ "$R_CODE" = 0 ] || [ "$R_CODE" = 1 ] || fail "exit code $R_CODE"
  assert_contains "$R_OUT" "prerequisites"
  assert_contains "$R_OUT" "distribution: ubuntu (family: debian)"
  assert_contains "$R_OUT" "package manager: apt"
  local w; for w in python pyqt socat; do assert_not_contains_ci "$R_OUT$R_ERR" "$w" "--check"; done
}

# the Qt window is optional: --check lists its run-time packages per family, says what building it needs, and never changes the verdict or asks for what CI forbids
t_check_names_the_qt_runtime_packages_per_family_as_optional() {
  local row d want
  for row in "arch|qt6-base qt6-declarative qt6-svg" "ubuntu|libqt6svg6 qml6-module-qtquick qml6-module-qtquick-controls" "debian|qml6-module-qt-labs-platform" \
             "fedora|qt6-qtbase-gui qt6-qtdeclarative qt6-qtsvg" "suse|libQt6Svg6 qt6-declarative-imports"; do
    d=${row%%|*}; want=${row#*|}
    run "TRUESENSE_OS_RELEASE=$(os_release "$d")" -- "$BASH_BIN" "$INSTALL" --check
    assert_contains "$R_OUT" "optional: the Qt window (truesense-qt)" "$d"; assert_contains "$R_OUT" "$want" "$d: the Qt run-time packages"
    assert_contains "$R_OUT" "'truesense gui' opens the built-in web window" "$d: what happens without the window"
    # what CI's installer jobs forbid in the report (the check runs in containers without Rust or Python)
    local w; for w in python pyqt socat cargo rustc; do assert_not_contains_ci "$R_OUT$R_ERR" "$w" "$d: --check"; done
  done
  # the verdict is the prerequisites' alone: with the whole module toolchain present (fake programs, fake headers) the exit code is 0 although no Qt window is there
  local bin; bin=$(tmpdir); for w in systemctl dkms make gcc; do fake_program "$bin" "$w" "exit 0"; done
  run "PATH=$bin:$(path_with sh env cat head cut tr uname grep awk sed ls getent realpath dirname date id)" "TRUESENSE_OS_RELEASE=$(os_release arch)" TRUESENSE_KERNEL=6.1.0-test -- "$BASH_BIN" "$INSTALL" --check
  assert_contains "$R_OUT" "optional: the Qt window"; assert_contains "$R_OUT" "installed now: no"
}

t_the_qt_runtime_packages_have_a_name_in_every_family_and_the_development_packages_are_listed() {
  local d
  for d in arch ubuntu fedora suse weird; do
    install_lib "TRUESENSE_OS_RELEASE=$(os_release "$d")" -- 'detect_os; echo "run=$(qt_runtime_pkgs)"; echo "dev=$(qt_dev_pkgs)"'
    assert_code 0; assert_not_contains "$R_OUT" "run=$" "$d"; assert_not_contains "$R_OUT" "dev=$" "$d"
  done
  install_lib "TRUESENSE_OS_RELEASE=$(os_release arch)" -- 'detect_os; qt_runtime_pkgs'
  assert_eq "$R_OUT" "qt6-base qt6-declarative qt6-svg" "the three Qt 6 packages of the brief"
  # the Qt packages are NOT module prerequisites: they never enter MISSING (a missing Qt must not turn into a package-manager call for everybody)
  install_lib "PATH=$(tmpdir)" "TRUESENSE_OS_RELEASE=$(os_release arch)" TRUESENSE_KERNEL=6.1.0-test -- 'detect_os; check_prereqs; echo "MISSING=${MISSING[*]}"'
  assert_not_contains "$R_OUT" "qt6" "the module prerequisites"
}

t_help_documents_the_qt_window() {
  run -- "$BASH_BIN" "$INSTALL" --help
  assert_code 0
  local w; for w in --no-qt TRUESENSE_QT_BIN truesense-qt bin/truesense-qt target/release/truesense-qt "web window"; do assert_contains "$R_OUT" "$w" "--help"; done
}

t_unknown_option_and_help() {
  run -- "$BASH_BIN" "$INSTALL" --nope
  assert_code 2; assert_contains "$R_ERR" "unknown option: --nope"
  run -- "$BASH_BIN" "$INSTALL" --help
  assert_code 0
  local opt; for opt in --check --no-deps --force --prefix=DIR --uninstall "Safety:" TRUESENSE_BIN Cargo.toml; do assert_contains "$R_OUT" "$opt" "--help"; done
}

t_installing_requires_root_and_says_how_to_check_first() {
  [ "$(id -u)" -ne 0 ] || skip "running as root: the refusal cannot be observed (and the script must never run for real)"
  run -- "$BASH_BIN" "$INSTALL"
  assert_code 1; assert_contains "$R_ERR" "run with sudo"; assert_contains "$R_ERR" "--check"
}

# ---- the three model tiers ----------------------------------------------------------------------------------------------------------------

# dmi PRODUCT VENDOR: a fake /sys/class/dmi/id; prints its path
dmi() { local d; d=$(tmpdir); write_file "$d/product_name" "$1\n"; write_file "$d/sys_vendor" "$2\n"; write_file "$d/bios_version" "V1.0\n"; printf '%s\n' "$d"; }

t_the_three_tiers_in_the_check_report() {
  local f out p; f=$(os_release arch)
  check() { run "TRUESENSE_DMI=$(dmi "$1" "$2")" "TRUESENSE_OS_RELEASE=$f" -- "$BASH_BIN" "$INSTALL" --check; }
  check "Predator PH16-71" Acer; assert_contains "$R_OUT" "(supported)" "the verified model"
  for p in "Predator PH16-72" "Predator PH18-71" "Predator PHN16-71" "Nitro AN16-41"; do
    check "$p" Acer
    for out in "NOT supported yet" "--force" "probe"; do assert_contains "$R_OUT" "$out" "report for $p"; done
  done
  check "Inspiron 15" "Dell Inc."
  assert_contains "$R_OUT" "NOT supported"; assert_contains "$R_OUT" "not an Acer Predator"
}

t_installing_on_another_model_is_refused_with_the_safe_next_step() {
  gate() { install_lib "TRUESENSE_DMI=$(dmi "$1" Acer)" -- 'FORCE=$1; model_gate' "$2"; }
  gate "Predator PH18-71" 0
  assert_code 1
  local want; for want in "not an Acer Predator PH16-71" "truesense probe" "similar Acer laptop"; do assert_contains "$R_ERR" "$want" "refusal"; done
  gate OptiPlex 0; assert_code 1; assert_contains "$R_ERR" "This is not an Acer Predator"
  gate "Predator PH16-71" 0; assert_code 0; assert_contains "$R_OUT" "model: Predator PH16-71"
  gate "Predator PH18-71" 1; assert_code 0; assert_contains "$R_OUT" "WARNING"
}

t_tiers_as_a_function() {
  local product word
  for product in "Predator PH16-71|(supported)" "Predator PHN16-71|NOT supported yet" "Nitro V 15|NOT supported yet" "OptiPlex|not an Acer Predator"; do
    word=${product#*|}; product=${product%%|*}
    install_lib -- 'model_tier "$1"' "$product"
    assert_contains "$R_OUT" "$word" "model_tier $product"
  done
}

# ---- find_binary: the program the installer puts on the system -----------------------------------------------------------------------------

SOURCE_VERSION=0.5.0

# source_tree VERSION: a fake source tree whose workspace Cargo.toml declares VERSION ("" = no Cargo.toml); prints its path
source_tree() {
  local d; d=$(tmpdir)
  [ -z "$1" ] || write_file "$d/Cargo.toml" "[workspace]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"$1\"\nrust-version = \"1.85\"\n\n[workspace.dependencies]\nserde = { version = \"1\" }\n"
  printf '%s\n' "$d"
}
# program PATH VERSION: a fake "truesense" that answers --version like the real one
program() { write_file "$1" "#!/bin/sh\necho \"truesense $2\"\n" 755; }
# find_binary_in ROOT [VAR=value ...]
find_binary_in() { local r=$1; shift; install_lib "$@" -- 'ROOT=$1; find_binary; echo "BIN=$BIN"' "$r"; }

t_find_binary_prefers_the_bundle_then_the_build_and_the_override_wins() {
  local root override; root=$(source_tree $SOURCE_VERSION)
  program "$root/target/x86_64-unknown-linux-musl/release/truesense" $SOURCE_VERSION
  find_binary_in "$root"
  assert_code 0; assert_line "$R_OUT" "BIN=$root/target/x86_64-unknown-linux-musl/release/truesense" "only the musl build"
  program "$root/bin/truesense" $SOURCE_VERSION
  find_binary_in "$root"
  assert_code 0; assert_line "$R_OUT" "BIN=$root/bin/truesense" "bin/truesense must win over the build"
  override=$(tmpdir)/mine; program "$override" $SOURCE_VERSION
  find_binary_in "$root" "TRUESENSE_BIN=$override"
  assert_code 0; assert_line "$R_OUT" "BIN=$override" "TRUESENSE_BIN must win"
}

t_find_binary_refuses_a_stale_binary() {
  local root w; root=$(source_tree $SOURCE_VERSION)
  program "$root/bin/truesense" 0.4.9
  find_binary_in "$root"
  assert_code 1
  for w in 0.4.9 $SOURCE_VERSION stale "make build"; do assert_contains "$R_ERR" "$w" "the refusal"; done
  assert_not_contains "$R_OUT" "BIN=" "a stale binary was accepted"
  # a stale build in target/ is refused the same way (the override is checked too)
  root=$(source_tree $SOURCE_VERSION); program "$root/target/x86_64-unknown-linux-musl/release/truesense" 0.4.9
  find_binary_in "$root"; assert_code 1; assert_contains "$R_ERR" stale
  root=$(source_tree $SOURCE_VERSION); program "$root/x" 0.4.9
  find_binary_in "$root" "TRUESENSE_BIN=$root/x"; assert_code 1; assert_contains "$R_ERR" stale
}

t_find_binary_explains_a_missing_binary() {
  local name root w
  # no program, no Cargo.toml: nothing to build; no program and a Cargo.toml but no cargo (PATH holds nothing): the same explanation
  for name in "no Cargo.toml|" "no cargo installed|$SOURCE_VERSION"; do
    root=$(source_tree "${name#*|}")
    find_binary_in "$root" "PATH=$(tmpdir)"
    [ "$R_CODE" = 1 ] || fail "${name%%|*}: exit $R_CODE, want 1"
    for w in "no program to install" bin/truesense "make build" releases rustup; do assert_contains "$R_ERR" "$w" "${name%%|*}: the explanation"; done
  done
  # a program that exists but is not executable does not count
  root=$(source_tree ""); write_file "$root/bin/truesense" "#!/bin/sh\necho truesense $SOURCE_VERSION\n" 644
  find_binary_in "$root"; assert_code 1; assert_contains "$R_ERR" "no program to install"
}

t_find_binary_refuses_a_binary_that_does_not_run() {
  local root; root=$(source_tree "")
  write_file "$root/bin/truesense" "#!/bin/sh\nexit 1\n" 755
  find_binary_in "$root"
  assert_code 1; assert_contains "$R_ERR" "does not run on this machine"
}

t_find_binary_accepts_a_tree_without_a_version_file() {
  local root; root=$(source_tree ""); program "$root/bin/truesense" 9.9.9
  find_binary_in "$root"
  assert_code 0; assert_line "$R_OUT" "BIN=$root/bin/truesense" "nothing to compare with"
}

# The tree's version is the FIRST standalone `version = "..."` of Cargo.toml: the rule the program's own updater applies. rust-version is not it; a later dependency version is not it.
t_the_tree_version_is_the_first_standalone_version_of_cargo_toml() {
  local d; d=$(tmpdir)
  write_file "$d/a" '[workspace]\nresolver = "3"\nmembers = ["crates/x"]\n\n[workspace.package]\nrust-version = "1.85"\nedition = "2024"\nversion = "0.5.0"\n\n[workspace.dependencies]\nserde = { version = "1", features = ["derive"] }\n'
  install_lib -- 'tree_version "$1"' "$d/a"; assert_eq "$R_OUT" "0.5.0" "rust-version and later dependencies are skipped"
  write_file "$d/b" '[package]\nname = "x"\n'
  install_lib -- 'tree_version "$1"' "$d/b"; assert_code 0; assert_eq "$R_OUT" "" "no version at all"
  install_lib -- 'tree_version "$1"' "$d/missing"; assert_code 0; assert_eq "$R_OUT" "" "no such file"
  write_file "$d/c" 'version="1.2.3"\n'
  install_lib -- 'tree_version "$1"' "$d/c"; assert_eq "$R_OUT" "1.2.3" "no spaces around ="
}

t_find_binary_builds_from_source_with_cargo_when_there_is_no_binary() {
  local root tools log musl; root=$(source_tree $SOURCE_VERSION); tools=$(tmpdir); log=$(tmpdir)/cargo.log; musl=$(tmpdir)
  # the fake cargo records how it was called and writes the program where --target says; the fake rustc says the musl standard library is installed
  fake_program "$tools" cargo "echo \"RUSTFLAGS=[\$RUSTFLAGS] TARGET_DIR=\$CARGO_TARGET_DIR \$*\" >> '$log'
while [ \$# -gt 0 ]; do [ \"\$1\" = --target ] && t=\$2; shift; done
mkdir -p \"\$CARGO_TARGET_DIR/\$t/release\"
printf '#!/bin/sh\\necho \"truesense $SOURCE_VERSION\"\\n' > \"\$CARGO_TARGET_DIR/\$t/release/truesense\"; chmod +x \"\$CARGO_TARGET_DIR/\$t/release/truesense\""
  fake_program "$tools" rustc "echo '$musl'"
  find_binary_in "$root" "PATH=$tools:$(path_with sh env mkdir chmod awk)"
  assert_code 0; assert_line "$R_OUT" "BIN=$root/target/x86_64-unknown-linux-musl/release/truesense" "the musl build"
  local calls w; calls=$(cat "$log")
  for w in "build" "--release" "--locked" "-p truesense" "--target x86_64-unknown-linux-musl" "TARGET_DIR=$root/target" "RUSTFLAGS=[]"; do assert_contains "$calls" "$w" "cargo was called without '$w'"; done
}

t_find_binary_falls_back_to_static_glibc_when_the_musl_library_is_missing() {
  local root tools log; root=$(source_tree $SOURCE_VERSION); tools=$(tmpdir); log=$(tmpdir)/cargo.log
  fake_program "$tools" cargo "echo \"RUSTFLAGS=[\$RUSTFLAGS] \$*\" >> '$log'
while [ \$# -gt 0 ]; do [ \"\$1\" = --target ] && t=\$2; shift; done
mkdir -p \"\$CARGO_TARGET_DIR/\$t/release\"
printf '#!/bin/sh\\necho \"truesense $SOURCE_VERSION\"\\n' > \"\$CARGO_TARGET_DIR/\$t/release/truesense\"; chmod +x \"\$CARGO_TARGET_DIR/\$t/release/truesense\""
  fake_program "$tools" rustc "echo /nonexistent/rustlib/x86_64-unknown-linux-musl/lib"
  find_binary_in "$root" "PATH=$tools:$(path_with sh env mkdir chmod awk)"
  assert_code 0; assert_line "$R_OUT" "BIN=$root/target/x86_64-unknown-linux-gnu/release/truesense" "the glibc fallback"
  assert_contains "$(cat "$log")" "RUSTFLAGS=[-C target-feature=+crt-static]" "the fallback must link statically"
  assert_contains "$(cat "$log")" "--target x86_64-unknown-linux-gnu"
  assert_contains "$R_OUT" "rustup target add x86_64-unknown-linux-musl" "the user is told how to get the musl build"
}

t_a_failing_build_stops_the_install_and_a_build_of_the_wrong_version_is_stale() {
  local tools root; tools=$(tmpdir)
  fake_program "$tools" cargo "exit 1"; fake_program "$tools" rustc "echo '$(tmpdir)'"
  find_binary_in "$(source_tree $SOURCE_VERSION)" "PATH=$tools:$(path_with sh env mkdir chmod awk)"
  assert_code 1; assert_contains "$R_ERR" "cargo build failed"
  tools=$(tmpdir); root=$(source_tree $SOURCE_VERSION)
  fake_program "$tools" cargo "while [ \$# -gt 0 ]; do [ \"\$1\" = --target ] && t=\$2; shift; done
mkdir -p \"\$CARGO_TARGET_DIR/\$t/release\"
printf '#!/bin/sh\\necho \"truesense 0.0.1\"\\n' > \"\$CARGO_TARGET_DIR/\$t/release/truesense\"; chmod +x \"\$CARGO_TARGET_DIR/\$t/release/truesense\""
  fake_program "$tools" rustc "echo '$(tmpdir)'"
  find_binary_in "$root" "PATH=$tools:$(path_with sh env mkdir chmod awk)"
  assert_code 1; assert_contains "$R_ERR" stale
}

# ---- the rollback and the order of the steps that protect the machine -----------------------------------------------------------------------

t_rollback_puts_the_previous_program_back_and_restarts_the_service() {
  local p bin log; p=$(tmpdir); bin=$(tmpdir); log=$(tmpdir)/calls
  write_file "$p/bin/truesense" "new" 755; write_file "$p/bin/truesense.prev" "previous" 755
  fake_program "$bin" systemctl "echo \"systemctl \$*\" >> '$log'"; fake_program "$bin" journalctl "exit 0"
  install_lib "PATH=$bin:$(path_with mv sed)" -- 'PREFIX=$1; rollback' "$p"
  assert_eq "$(cat "$p/bin/truesense")" previous "the program"; assert_no_file "$p/bin/truesense.prev"
  assert_contains "$(cat "$log")" "systemctl restart truesense.service" "the old version must be started again"
  assert_contains "$R_OUT" "restoring the previous one"
}

t_rollback_without_a_previous_program_stops_the_service_and_says_the_fans_are_back() {
  local p bin log; p=$(tmpdir); bin=$(tmpdir); log=$(tmpdir)/calls
  write_file "$p/bin/truesense" "new" 755
  fake_program "$bin" systemctl "echo \"systemctl \$*\" >> '$log'"; fake_program "$bin" journalctl "exit 0"
  install_lib "PATH=$bin:$(path_with mv sed)" -- 'PREFIX=$1; rollback' "$p"
  assert_contains "$(cat "$log")" "systemctl stop truesense.service"
  assert_not_contains "$(cat "$log")" "restart" "there is nothing to restart"
  assert_contains "$R_OUT" "back on the firmware's control"
}

# structural: these orders are what keeps a failed install from leaving the laptop without a daemon (a test cannot install for real, so the script's text is checked)
line_of() { grep -n -F -- "$1" "$INSTALL" | head -n1 | cut -d: -f1; }

t_the_program_is_found_before_anything_on_the_system_changes() {
  local find legacy module
  find=$(grep -n '^  \[ "\$MODULE_ONLY" = 1 \] || find_binary$' "$INSTALL" | cut -d: -f1)
  legacy=$(line_of '"$ROOT/tools/migrate-legacy.sh"'); module=$(line_of '"$ROOT/tools/install_kernel_module.sh"')
  [ -n "$find" ] || { fail "find_binary is not called in the pre-flight checks"; return; }
  [ "$find" -lt "$legacy" ] || fail "find_binary (line $find) must come before the legacy cleanup (line $legacy)"
  [ "$find" -lt "$module" ] || fail "find_binary (line $find) must come before the kernel module install (line $module)"
  assert_eq "$(grep -c '^find_binary$' "$INSTALL")" 0 "no second, late find_binary call"
}

t_the_old_python_tree_is_removed_only_after_the_new_service_started() {
  local wait rm_old swap
  wait=$(line_of 'if ! wait_for_service; then rollback'); rm_old=$(line_of 'rm -r -f "$L/truesense" "$R$PREFIX/lib/truesense.prev"')
  swap=$(line_of 'mv -f "$B/truesense.new" "$B/truesense"')
  [ -n "$wait" ] && [ -n "$rm_old" ] && [ -n "$swap" ] || { fail "a step is missing from the script (wait=$wait rm=$rm_old swap=$swap)"; return; }
  [ "$rm_old" -gt "$wait" ] || fail "the old tree is removed (line $rm_old) before the service is known to run (line $wait)"
  [ "$swap" -lt "$wait" ] || fail "the new program must be in place (line $swap) before the service is waited for (line $wait)"
  assert_contains "$(sed -n "$((wait + 1))p" "$INSTALL")" 'rm -f "$B/truesense.prev"' "the rollback copy is dropped only once the new service runs"
}

t_the_program_is_swapped_atomically_with_a_rollback_copy_and_a_fast_link() {
  local text; text=$(cat "$INSTALL")
  assert_contains "$text" 'install -m755 "$BIN" "$B/truesense.new"'
  assert_contains "$text" 'cp -p "$B/truesense" "$B/truesense.prev.new"; mv -f "$B/truesense.prev.new" "$B/truesense.prev"'
  assert_contains "$text" 'mv -f "$B/truesense.new" "$B/truesense"'
  assert_contains "$text" 'mv -f -T "$B/.truesense-fast.new" "$B/truesense-fast"'
  assert_contains "$text" 'truesense-nvml.c' "the GPU helper is built from C"
}

run_tests "$@"
