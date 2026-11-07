# TrueSense (Rust edition): the gates. `make ci` is what CI runs.
CARGO ?= cargo

.PHONY: build test lint fmt ci
build:          ## debug build of every crate
	$(CARGO) build --workspace
test:           ## every test, hermetic (no hardware, no root)
	$(CARGO) test --workspace
lint:           ## rustfmt and clippy with warnings as errors
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
fmt:
	$(CARGO) fmt --all
ci: lint test   ## what CI runs
