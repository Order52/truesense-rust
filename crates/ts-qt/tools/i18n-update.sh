#!/usr/bin/env bash
# Refreshes i18n/*.ts from the QML sources: every qsTrId("id") //% "English text" in qml/, plus the
# page and group titles of qml/pages/registry.json (which QML reads at run time, so lupdate cannot
# see them: they are written to a temporary QML file as QT_TRID_NOOP lines first).
#
# en.ts gets its translations filled with the source text (qsTrId returns the id when a catalogue
# has no entry, so English needs a catalogue too). Other languages keep what they have and get new
# ids as unfinished entries. Run it after adding or changing strings; commit the .ts files.
set -euo pipefail
cd "$(dirname "$0")/.."
LUPDATE=${LUPDATE:-/usr/lib/qt6/bin/lupdate}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/qml"
cp -r qml/. "$tmp/qml/"
{
  echo 'import QtQuick'
  echo 'Item {'
  jq -r '(.groups[], .pages[]) | "    //% \"\(.title)\"\n    property var _t_\(.id) : QT_TRID_NOOP(\"\(.titleId)\")"' qml/pages/registry.json
  echo '}'
} > "$tmp/qml/RegistryTitles.qml"
files=(i18n/en.ts i18n/ar.ts)
[ -f i18n/en.ts ] || printf '<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE TS>\n<TS version="2.1" language="en">\n</TS>\n' > i18n/en.ts
[ -f i18n/ar.ts ] || printf '<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE TS>\n<TS version="2.1" language="ar">\n</TS>\n' > i18n/ar.ts
"$LUPDATE" -silent -no-obsolete -locations none -extensions qml "$tmp/qml" -ts "${files[@]}"
# English: translation = source.
awk '
  /<source>/ { src = $0; sub(/^[ \t]*<source>/, "", src); sub(/<\/source>[ \t]*$/, "", src) }
  # lupdate also pre-fills an unfinished entry with the text of an earlier message that has the same
  # English source ("Eco" in two places), so any unfinished English entry becomes source = translation.
  /<translation type="unfinished">[^<]*<\/translation>|<translation type="unfinished"\/>/ { print "        <translation>" src "</translation>"; next }
  { print }
' i18n/en.ts > "$tmp/en.ts" && mv "$tmp/en.ts" i18n/en.ts
echo "updated: $(grep -c '<message' i18n/en.ts) messages; untranslated in ar: $(grep -c 'type="unfinished"' i18n/ar.ts)"
