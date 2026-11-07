#!/usr/bin/env bash
# Runs the QML component tests (Qt Quick Test) against the real QML module built by cargo.
#
# cxx-qt exports the module (qmldir plus a copy of every .qml) to target/cxxqt/qml_modules during
# the build; qmltestrunner imports it from there, so the tests exercise exactly the files the app
# ships. The Rust-only parts of the module (the Bus) are not loaded: components never mention it.
# Offscreen, no session bus (so nothing can appear on a real desktop), file reads for the icons and
# the shared format vectors.
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${QMAKE:=/usr/bin/qmake6}"
: "${CARGO:=cargo}"
QTBIN=${QTBIN:-$("$QMAKE" -query QT_INSTALL_LIBEXECS)/bin}
[ -x "$QTBIN/qmltestrunner" ] || QTBIN=/usr/lib/qt6/bin
QMAKE=$QMAKE $CARGO build -p ts-qt --lib >/dev/null
TARGET=${CARGO_TARGET_DIR:-target}
EXPORT=$TARGET/cxxqt/qml_modules/org/truesense/app
MODULE=$TARGET/qml-test-modules
DIR=$MODULE/org/truesense/app
rm -rf "$MODULE"
mkdir -p "$DIR"
# The exported qmldir names the static Rust plugin (the Bus), which qmltestrunner does not have:
# keep the component list, drop the plugin lines.
grep -vE '^(optional plugin|plugin|classname|typeinfo|prefer) ' "$EXPORT/qmldir" > "$DIR/qmldir"
# (an absolute CARGO_TARGET_DIR must work too: resolve the directory instead of prefixing $PWD)
ln -s "$(cd "$EXPORT" && pwd)/qml" "$DIR/qml"
# The icons are Qt resources in the app; the exported copy has only the .qml files.
ln -sfn "$PWD/crates/ts-qt/qml/icons" "$EXPORT/qml/icons"
export QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_FORCE_STDERR_LOGGING=1
export DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/bus
exec "$QTBIN/qmltestrunner" -import "$MODULE" -input crates/ts-qt/tests/qml "$@"
