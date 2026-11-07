# TrueSense (Rust edition) developer shortcuts. `make ci` is what CI's static and test jobs run, locally.
CARGO  ?= cargo
TARGET ?= x86_64-unknown-linux-musl
BIN    := target/$(TARGET)/release/truesense
# The Qt app needs the Qt 6 development files (qmake6, qmlcachegen, lrelease); it is not in the
# default workspace members, so `make ci` never builds it. `make qt-ci` is its gate.
QMAKE  ?= /usr/bin/qmake6
QTENV  := QMAKE=$(QMAKE)

.PHONY: debug build test lint fmt ci qt-build qt-release qt-test qt-lint qt-ci static-ci shelltest check-shell verify-units release-check install uninstall doctor module-test audit
debug:           ## debug build of every crate except the Qt app (see qt-build)
	$(CARGO) build
build:           ## the static release program, built the way releases are: target/$(TARGET)/release/truesense (truesense-fast is a link the installer makes)
	@TARGET=$(TARGET) tools/ci/build-binary.sh >/dev/null && echo "built $(BIN)"
test:            ## every test of the default members, hermetic (no hardware, no root, no Qt)
	$(CARGO) test
lint:            ## rustfmt and clippy with warnings as errors
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --all-targets -- -D warnings
fmt:
	$(CARGO) fmt --all
shelltest:       ## the installer and get.sh tests (bash only, nothing is installed)
	@tools/shelltest/run.sh
check-shell:     ## bash -n and shellcheck on every script we ship
	@tools/ci/check-shell.sh
verify-units:    ## systemd-analyze verify on every unit
	@tools/ci/verify-units.sh | tail -3
release-check:   ## do the version numbers, notes and tag agree?  (make release-check TAG=v0.5.0)
	@tools/release.sh check $(if $(TAG),--tag $(TAG),)
ci: lint test    ## rustfmt, clippy, tests: the Rust gate (without the Qt app)
qt-build:        ## the Qt app (debug): target/debug/truesense-qt
	$(QTENV) $(CARGO) build -p ts-qt
qt-release:      ## the Qt app (release): target/release/truesense-qt
	$(QTENV) $(CARGO) build --release -p ts-qt
qt-test:         ## Qt app tests, offscreen: Rust unit and end-to-end tests, then the QML component tests
	$(QTENV) QT_QPA_PLATFORM=offscreen $(CARGO) test -p ts-qt
	$(QTENV) crates/ts-qt/tools/qml-tests.sh
qt-lint:         ## rustfmt and clippy (warnings are errors) on the Qt app
	$(CARGO) fmt -p ts-qt -- --check
	$(QTENV) $(CARGO) clippy -p ts-qt --all-targets -- -D warnings
qt-ci: qt-lint qt-test  ## the Qt app's gate
static-ci:       ## what CI's static job runs, locally (tools that are not installed are skipped): version table, shell, units, installer tests
	@tools/release.sh check
	@if command -v actionlint >/dev/null 2>&1; then actionlint && echo "ok  actionlint"; else echo "skip actionlint"; fi
	@tools/ci/verify-units.sh | tail -3
	@tools/ci/check-shell.sh
	@tools/shelltest/run.sh
audit:           ## known vulnerabilities in the dependencies of Cargo.lock (cargo-audit, if installed)
	@if command -v cargo-audit >/dev/null 2>&1; then $(CARGO) audit; else echo "skip cargo-audit (cargo install cargo-audit --locked)"; fi
install: build   ## one command: kernel module (DKMS), daemon, user services, completions (needs sudo)
	sudo tools/install.sh
uninstall:
	sudo tools/uninstall.sh
doctor: build
	./$(BIN) doctor
module-test:     ## build the kernel module for every installed kernel (warnings on)
	@tools/ci/build-modules.sh
