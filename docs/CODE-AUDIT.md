# Code audit

A read-through of the whole workspace (16 crates, about 146,000 lines of Rust and 18,000 of QML) against one standard: clean, modular, scalable, and written as if the vendor had made it. Each section says what was looked at, what was found, what was changed and what was left alone on purpose. The numbers are from the commit this page was last updated in; section 12 lists what the pass changed.

Method: `cargo clippy --workspace --all-targets` with `-W unused -W dead_code -W unreachable_pub -W missing_docs -W clippy::pedantic` (the audit only; the gate is stricter on some lints and looser on others), `cargo tree`, `cargo doc --no-deps` with `-D warnings`, and small scripts over the sources (public items never used outside their crate, `unsafe` inventory, tests that depend on time).

## 1. Crates

Source lines exclude tests and test helpers. "Public items" counts `pub` free items; "methods" counts `pub fn` inside impls.

| Crate | Job | Source lines | Tests | Public items / methods |
|---|---|---:|---:|---:|
| `ts-foundation` | config, state files, atomic file writes, sd_notify, wake-ups, build info, shared small rules (names, locks) | 2,100 | 100 | 70 / 59 |
| `ts-platform` | kernel-module client, firmware simulator, typed hardware features with read-back, command runner | 2,300 | 94 | 44 / 68 |
| `ts-ipc` | control-socket wire format, uid-checked server, client | 1,200 | 51 | 24 / 45 |
| `ts-decision` | pure decision logic: fan curves, smart fan, auto profile, smart profiles, game rules, thermal governor, alerts, bindings, crash guard | 4,300 | 208 | 123 / 144 |
| `ts-fx` | lighting engine and the 32 themes (GPL-3.0 derived, see THIRD_PARTY.md) | 8,700 | 104 | 121 / 22 |
| `ts-domain` | what the hardware can do: keyboard, looks, per-key effects, audio, power, undervolt, profiles, key bindings, favourites, actions | 12,400 | 462 | 318 / 292 |
| `ts-system` | the machine around it: models, doctor, selftest, services, update, osd, touchpad, sensors, monitor sources | 8,600 | 389 | 221 / 121 |
| `ts-daemon` | the control loop and the socket command table | 3,500 | 177 | 32 / 24 |
| `ts-api` | the application layer every client shares, capabilities, monitor, the real service builder | 8,200 | 395 | 118 / 306 |
| `ts-cli` | the command tree and the fast status-bar client | 5,200 | 126 | 6 / 2 |
| `ts-tui` | the terminal UI | 3,900 | 153 | 2 / 0 |
| `ts-gui` | the command bus the apps use, and the local web window (fallback) | 3,700 | 171 | 43 / 13 |
| `ts-keys` | PREDATOR/Fn key service, theme renderer, login startup | 2,000 | 106 | 40 / 25 |
| `ts-ui-model` | the Qt-free logic of the app: bus, live store, view-models per page | 8,500 | 355 | 213 / 84 |
| `ts-qt` | the Qt 6 / QML skin (the only crate that needs Qt) | 900 | 18 | 20 / 13 |
| `truesense` | the static binary: dispatch by subcommand and program name | 500 | 9 | 8 / 1 |

A binary crate (`truesense`) and the clients that are used only through one entry point (`ts-cli`, `ts-tui`) have a deliberately tiny public surface. The large public surfaces (`ts-domain`, `ts-system`, `ts-api`, `ts-ui-model`) are the layers other crates are built on; section 4 narrows what nothing outside uses.

## 2. Dependency direction

`cargo tree` shows one acyclic graph that follows the layer table of RUST-MIGRATION.md:

```
foundation <- platform, ipc, decision, fx
platform, fx <- domain <- system <- daemon (+ipc, decision) <- api <- cli, tui, gui, keys <- ui-model <- qt
```

* No cycle can exist (Cargo refuses). No lower crate names a higher one.
* The Qt-free crates (everything but `ts-qt`) do not depend on Qt or on `cxx`; the default workspace members exclude `ts-qt` so `make ci` never needs Qt. `ts-ui-model` is Qt-free and stays so.
* Sibling-client edges: `ts-ui-model` uses the command bus of `ts-gui` (`Commands`, the same contract the web window serves) and the start-up helpers of `ts-keys`. That is the reason the bus lives in a crate of its own layer and not in the window; it is not a violation, and moving it would only add a crate.
* Duplicate crate versions in `Cargo.lock`: `codespan-reporting` (0.11 and 0.13) and `foldhash` (0.1 and 0.2). Both come from the build-time tooling of cxx-qt and from ratatui's `lru`; neither can be unified from here without forking a dependency. Left alone.
* Unused dependencies: every `Cargo.toml` entry was checked by removing it and building all targets. 14 entries in 7 crates were unused and removed (`truesense`: ts-domain, ts-decision; `ts-ipc`: ts-foundation; `ts-keys`: ts-ipc, ts-system, serde; `ts-platform`: libc; `ts-system`: ts-domain; `ts-tui`: ts-foundation, ts-ipc, serde; `ts-qt`: ts-api, ts-gui, ts-keys). Two of them (ts-foundation in `ts-ipc` and `ts-tui`) came back later because the shared lock helper uses them: 12 net.
* `ts-ui-model` now also depends on `ts-decision` and `ts-domain` (both Qt-free, both below it): see section 5.
* Third-party dependencies of the static program stay at the small set the plan names: `serde`, `serde_json`, `toml`, `libc`, `sha2`, `flate2`, `ratatui`/`crossterm`, `tiny_http`.

## 3. Module size

Only six files had more than 800 lines of non-test code, and none mixed unrelated jobs:

| File | Lines | Verdict |
|---|---:|---|
| `ts-ui-model/src/vm/lighting.rs` | 1,296 | one page's view-model with one dispatcher; its sections were separated by banner comments. **Split** into six submodules (`colour`, `picture`, `effects`, `looks`, `bindings`, `themes`) of 90 to 330 lines; the dispatcher, shared constants and tests stay (887 lines with the tests). A move only. |
| `ts-decision/src/curves.rs` | 874 | one concept (a fan curve and its follower), most of it validation text. Left. |
| `ts-system/src/update/mod.rs` | 851 | the update flow as one story; the hard parts are already in `extract`, `host`, `fetch`. Left. |
| `ts-foundation/src/config.rs` | 832 | one TOML schema with its validators. Left. |
| `ts-ui-model/src/vm/monitor.rs`, `cooling.rs` | 815, 809 | one page each. Left. |

Splitting a file just to get under a number would scatter one idea over several files; those stay.

## 4. Dead code and public API

* `dead_code`, `unused` and `missing_docs` report nothing: the gate already denies them through clippy's `-D warnings` plus `missing_docs = "warn"`.
* `unreachable_pub` reported 78 items: `pub` on items of private modules (every `ts-fx` effect, `ts-fx` key and hardware tables), of binary crates and of test helpers. They are `pub(crate)` now (rustc's own fix).
* Public items of a library crate that no other crate uses (587 by script: the name searched in every other crate) were narrowed to `pub(crate)` and the compiler decided which narrowings stand (a re-export or a public signature that names the type keeps it public): 308 stood. With the 24 more from `unreachable_pub`, about 330 items are narrowed; 12 of those became public again when `ts-ui-model` started to read them (the curve limits, the power range, the GPU cap).
* What had no user at all was deleted: `ts_ui_model::bus::CODE_NEEDS_DAEMON` (QML spells the code itself) and `truesense::launcher::stop_requested` (an `#[allow(dead_code)]` "kept for symmetry"). Five more items are used only by their own tests (`USB_LEVELS`, `fanctl::plan_off`, `netstat::list_interfaces`, the two `builtin_names`); they stay public rather than be moved behind `#[cfg(test)]`, because each is a small, documented, tested piece of the layer's vocabulary.
* `ts_foundation::buildinfo` carried a commit and build date that the build scripts inject but nothing printed; `truesense --version --long` now prints them (`truesense -V` is unchanged: scripts compare it).

## 5. Duplicated logic

| Duplicate | Where | Decision |
|---|---|---|
| The rule for a library name that becomes a file name (letters, digits, `.`, `_`, `-`, 1 to 40) | `ts-decision` x3, `ts-domain` x3, `ts-ui-model` x3 | one `ts_foundation::names`; it is a safety rule (no path can be built from a name), so it must not drift |
| Mutex lock that survives a poisoned lock | 6 free helpers and about 50 inline copies | one `ts_foundation::sync::locked`; helpers replaced |
| `parse_look` (a look copy must decode and change something) | `ts-daemon` and `ts-api` | one `Look::from_copy` in `ts-domain` |
| Ranges and risk bands of the Performance, Cooling, Keys, Lighting, Profiles and Settings pages, mirrored in `ts-ui-model::vm` (the Smart auto ranges and keys, four copies of the power-mode list, the curve limits and follower ranges, the PL1/PL2, CPU and GPU undervolt bands and their wording, the alert ranges and defaults, the key timing ranges, the favourites cap, the built-in key profile names, the keyboard brightness and speed ranges) | `ts-ui-model` | `ts-ui-model` depends on `ts-decision` and `ts-domain` (both Qt-free) and reads the real constants; the three `risk` functions of the page are the daemon's own (`power::risk`, `undervolt::risk`, `gpuundervolt::risk`), so the wording cannot drift. Literal pins of the safety limits stay in `ts-ui-model/src/limits.rs` and the view-model tests, so changing a limit is a deliberate act that fails a test first |
| `autoprofile::Activity` and `fanctl::Activity` | `ts-decision`, `ts-daemon` | two names for one idea, converted by a `From` in the daemon. Decided earlier to leave `ts-decision` alone: the types are in different state machines that were ported separately and are pinned by golden transcripts. Left. |
| `FAN_MIN`/`FAN_MAX` (20 and 100) and `MIC_VOLUME_MAX`/`BOOST_MAX` in `ts-ui-model` | `ts-ui-model` | the fan duty rule lives in `ts-platform` (typed `FanDuty`), which `ts-ui-model` does not depend on, and the microphone limits are the page's own slider ends. Left; the daemon refuses anything else. |
| The grammar of key actions mirrored in `ts-ui-model::vm::keys` | `ts-ui-model` | an editor-state machine (kind/choice/text), not a table; it cannot be replaced by a constant. Kept, with the existing test that probes the real parser through `set_key_action`. |

## 6. Error handling

* Every crate that can fail has a typed error enum with `Display` and `std::error::Error`; 52 error types exist, they carry the operation, and `source()` is implemented where there is one. No `anyhow`; the only `Box<dyn Error>` is the boxed source inside `ApiError`.
* `Result<_, String>` crosses a crate boundary only at buses: the daemon's command table (`ts_ipc::Handlers`, the JSON wire), the `ts-ui-model::vm` verbs (`Bus.vm(...)`, whose error text goes straight to QML), and the Qt argument parser. Those are the documented edges.
* One place leaked a string where a type exists: `ts_domain::keybinds::validate`; it returns the text the user sees, by design (the Python edition's messages), and every caller shows it unchanged. Left, and documented.

## 7. `unsafe`

The workspace lint `unsafe_code` is `deny` (it was not set before: a new `unsafe` anywhere would have compiled unnoticed), and each of the nine modules that need it says so with `#![allow(unsafe_code)]` (or `#[allow]` on the one function); 195 other source files are `#![forbid(unsafe_code)]`. Inventory:

| Crate / file | Blocks | What | SAFETY comment | Test |
|---|---:|---|---|---|
| `truesense/src/signals.rs` | 9 | `sigaction`, self-pipe, `isatty` | yes | yes (signal delivery, pipe) |
| `ts-daemon/src/assembly.rs` | 3 | `sigwait`, `pthread_sigmask` | yes | yes |
| `ts-domain/src/keyboard/hidraw.rs` | 1 | `ioctl` HID feature report | yes | yes (pty/file fake) |
| `ts-foundation/src/fsx.rs` | 1 | `flock` | yes | yes |
| `ts-ipc/src/server.rs` | 1 | `getsockopt(SO_PEERCRED)` | yes | yes |
| `ts-keys/src/watch.rs` | 10 | inotify, self-pipe, `poll` | yes | yes |
| `ts-keys/src/fnkeys/hid.rs` | 1 | `poll` | yes | yes |
| `ts-system/src/doctor/sys.rs` | 2 | `geteuid`, `access` | yes | yes |
| `ts-qt/src/bus_object.rs` | the bridge macro's | `unsafe extern "C++"` blocks and `unsafe fn` invokables that `#[cxx_qt::bridge]` requires | module-level note added | the offscreen end-to-end test (`tests/app.rs`) |

## 8. `unwrap`, `expect`, `panic` outside tests

Clippy denies `unwrap_used` and `panic` and warns `expect_used` (an error under `-D warnings`) for every crate. The `#[allow(...)]` sites are all `#[cfg(test)]` modules or test helpers, except: `ts-api/adapt/node.rs:62` and `ts-daemon/daemon/adapt.rs:276` (test modules), `ts-daemon/daemon/daemon.rs:454` (`too_many_arguments` on the one constructor that takes every port), `ts-fx/engine.rs:203` (`dead_code` outside tests on a field only the parity test reads, documented). The `dead_code` allow of `truesense/launcher.rs` was real dead code and is gone. No production `unwrap` or `panic` exists. `clippy::pedantic` also flags `ts_ui_model`'s `int_range` helper as possibly infinite: it is bounded by its `take_while`, but would overflow for a `hi` near `i64::MAX`; the callers pass slider ends, so it is left, and noted here.

## 9. Logging

There is no logging framework by design (no dependency, no global state). Daemon and services write one line per event to stdout/stderr, which systemd sends to the journal; library code that needs to say something takes a `log: Arc<dyn Fn(&str)>` (or a trait) that tests replace. `println!`/`eprintln!` appear only in the entry points (`truesense`, `ts-qt`, `ts-keys` service, the daemon assembly) and in the build scripts.

## 10. Naming, TODOs, rustdoc

* No `TODO`, `FIXME`, `XXX` or `HACK` in Rust, QML or TOML.
* Naming: modules are nouns of the domain (`looks`, `curves`), traits name the port (`Device`, `Clock`, `CommandRunner`), fakes are `Fake*` or in a `testing` module, constructors `new`/`with_*`/`simulated`. British spelling in docs and messages ("colour", "favourite"), kept.
* `cargo doc --no-deps` with `-D warnings` failed on about a dozen broken, private or ambiguous links before this pass, and on 58 more after the narrowing (a public doc linking to an item that is now `pub(crate)`); all fixed, and `make doc` (part of `make ci`) now keeps it that way.
* Pedantic: `clippy::pedantic` reports about 3,400 findings (most of them `must_use_candidate`, `missing_errors_doc`, the `cast_*` family and `doc_markdown`), so enabling it with an allow-list of forty lints would be churn without a safety gain. Nothing was added to the gate. Its few real findings were fixed (a vacuous `|| true` assertion in a bus test that had hidden a wrong expectation, two double negations).

## 11. Tests that depend on time or on the machine

* Deterministic with injected clocks: the key service (`ts-keys`), the daemon loop, alerts, update, selftest, fan and profile state machines.
* Fixed in this pass: the live-feed hub tests (`ts-gui`, 5 ms ticks and 30 to 80 ms sleeps) now wait for counted readings and for the reader thread to end (its clone of the commands is dropped), with a 30 s bound that only a failure uses; the key-service "real descriptor" test (`ts-keys`, a 400 ms long-press threshold against a real clock) runs on a manual clock and waits for each report to be taken and handled before time moves; the test that stops the key service "after 60 ms" stops it when the service has logged what the test is about; an inotify test drains until the event stream is quiet; `backoff` no longer asserts that a 100-microsecond wait took under 10 ms; the trickling-peer deadline test has a bound a loaded machine cannot reach.
* Remaining real-time tests (all wait for a condition with a bound of 5 to 60 seconds, or sleep only to prove that something does NOT happen): `ts-ipc` server (idle and slow clients, real sockets in a temp directory), `ts-foundation::wake` and `backoff` (a few milliseconds, assert only lower bounds), `ts-daemon` resilience tests (child process, kill and restart), `ts-ui-model` poller and coalescer (millisecond ticks with polling asserts), `ts-tui` worker, `ts-qt` end-to-end (spawns the app offscreen).
* Tests that read the machine: only `/proc/self` for the effective uid and loopback or Unix sockets in a temporary directory. The installer tests (`tools/shelltest`) no longer touch the real system (fixed in 0.5.0).

## 12. What this pass changed

| Step | Change | Size |
|---|---|---|
| Dependencies | 14 unused entries removed (12 net) | 7 crates |
| `unsafe` | workspace lint `unsafe_code = "deny"`, allowed in nine modules, SAFETY note on the cxx-qt bridge, one test no longer uses libc | 10 files |
| Public API | 78 `unreachable_pub` items and about 250 more items nobody outside the crate uses are `pub(crate)`; 2 dead items deleted | about 330 items, about 110 files |
| Shared rules | `ts_foundation::names` (the file-safe name rule: 6 copies), `ts_foundation::sync::locked` (6 helpers and about 40 inline copies), `Look::parse_copy` (2 copies) | about 50 sites |
| `ts-ui-model` | reads limits and risk bands from the layers that enforce them; mirrors removed | 16 files, +225 -292 lines |
| Tests | 6 time-dependent tests made deterministic, 1 flaky inotify test, 1 vacuous assertion fixed, about 10 tests added (`names`, `sync`, `parse_copy`, the limits and risk bands) | 12 files |
| Docs | README, ARCHITECTURE, STATUS, QT-APP, INSTALL, RELEASING consistent with the code; 2 broken links fixed; link checker `tools/ci/check-doc-links.sh` (CI static job, `make check-docs`, shell test) | 12 files |
| rustdoc | about 70 broken or private links fixed; `make doc` is part of `make ci` | 37 files |
| Module size | `vm/lighting.rs` split | 7 files |

Whole pass: 174 files changed, about 2,500 lines added and 2,000 removed (a good part of both is moved code). The gate at the end: `make ci` (3,008 tests, rustdoc), `make qt-ci` (18 Rust and 111 QML tests), `tools/shelltest/run.sh` (all pass).

### Left alone on purpose

* The two `Activity` types, the grammar mirror of the Keys editor, `FAN_MIN`/`FAN_MAX` (section 5).
* Duplicate crate versions caused by the build tooling of cxx-qt (section 2).
* `Result<_, String>` at the buses and `keybinds::validate` (section 6).
* Files between 800 and 900 lines that are one idea (section 3).
* Real-time tests that wait on a condition with a long bound, or only prove that nothing happens (section 11).
* Wiring a "move the unreadable `ui.json` aside" action to a button: `UiStore::move_unreadable_aside` exists and is tested but nothing in the window calls it (it was one of the few public methods with no production user).

### Needs a decision

* Whether `ts-ui-model` should keep depending on the sibling client crates `ts-gui` (the command bus) and `ts-keys` (start-up helpers), or whether the bus trait should move down to `ts-api`. It works and is documented; moving it is a larger change with no user-visible gain.
* A `SECURITY.md` (docs/SAFETY.md pointed at one that never existed; it now says to open an issue or use the repository's private security advisories).
