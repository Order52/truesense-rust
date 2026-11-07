#!/usr/bin/env bash
# Refreshes i18n/*.ts from the QML sources: every qsTrId("id") //% "English text" in qml/, plus the
# page and group titles of qml/pages/registry.json (which QML reads at run time, so lupdate cannot
# see them: they are written to a temporary QML file as QT_TRID_NOOP lines first).
#
# en.ts gets its translations filled with the source text (qsTrId returns the id when a catalogue
# has no entry, so English needs a catalogue too). Any other i18n/<code>.ts keeps what it has and
# gets new ids as unfinished entries; English is the only language today. Run it after adding or
# changing strings; commit the .ts files.
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
[ -f i18n/en.ts ] || printf '<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE TS>\n<TS version="2.1" language="en">\n</TS>\n' > i18n/en.ts
files=(i18n/*.ts)
"$LUPDATE" -silent -no-obsolete -locations none -extensions qml "$tmp/qml" -ts "${files[@]}"
# English: translation = source.
awk '
  /<source>/ { src = $0; sub(/^[ \t]*<source>/, "", src); sub(/<\/source>[ \t]*$/, "", src) }
  /<translation type="unfinished"><\/translation>|<translation type="unfinished"\/>/ { print "        <translation>" src "</translation>"; next }
  # lupdate pre-fills an id whose source text another id already has, still marked unfinished.
  /<translation type="unfinished">.*<\/translation>/ { print "        <translation>" src "</translation>"; next }
  { print }
' i18n/en.ts > "$tmp/en.ts" && mv "$tmp/en.ts" i18n/en.ts
echo "updated: $(grep -c '<message' i18n/en.ts) messages"
for f in "${files[@]}"; do
  [ "$f" = i18n/en.ts ] || echo "untranslated in $(basename "$f" .ts): $(grep -c 'type="unfinished"' "$f" || true)"
done
