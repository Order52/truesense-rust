// The ONE place colours, sizes and fonts are defined. Components and pages use only these tokens
// (Theme.bg, Theme.accent, ...); nothing else in the app contains a colour literal, so the light
// theme, the dark theme and a future high-contrast theme are a change to this file alone.
//
// Dark values are the web window's (crates/ts-gui/web/app.css), which match the old Qt app; the
// light values are its :root set. "system" follows the desktop colour scheme and falls back to
// dark when the desktop does not say (the default look of the app is dark).
pragma Singleton
import QtQuick

QtObject {
    id: theme

    // ---- which palette ------------------------------------------------------------------------
    readonly property bool systemDark: Application.styleHints.colorScheme !== Qt.ColorScheme.Light
    readonly property bool dark: Ui.theme === "light" ? false : (Ui.theme === "dark" ? true : systemDark)

    function pick(darkValue, lightValue) { return dark ? darkValue : lightValue }

    // ---- accent (Settings > Appearance: Ui.accent) --------------------------------------------
    // Per accent: [dark accent, light accent, dark soft, light soft, dark line, light line,
    // dark selection, light selection, dark focus ring, light focus ring]. Every light accent keeps
    // white text on it at 4.5:1 or more; every dark accent keeps the near-black ink at 7:1 or more.
    readonly property var accents: ({
        cyan:   ["#2ed3e6", "#007585", "#0f2a31", "#dcf3f6", "#1d6672", "#9fd6de", "#132f37", "#d6f0f4", "#8deaf5", "#005a66"],
        blue:   ["#58a6ff", "#1a66cc", "#1c2b3f", "#e2edfb", "#2f5d8f", "#9fc0ee", "#1f2d42", "#dbe8fb", "#8ec2ff", "#0b4fa8"],
        red:    ["#ff4d5e", "#c0283a", "#36181d", "#fbe3e6", "#7a2a35", "#eeb0b8", "#38191f", "#fbe0e3", "#ff99a3", "#9a1c2c"],
        orange: ["#ff9f43", "#9e4f00", "#33261a", "#fdeedd", "#7a5426", "#f0c79a", "#35281b", "#fde9d4", "#ffc58a", "#7a3d00"],
        purple: ["#b28cff", "#6b3fd1", "#262040", "#ece4fd", "#4f3f86", "#c6b2f2", "#28223f", "#e8defd", "#d2bcff", "#4f2aa8"],
        green:  ["#3ddc97", "#13804f", "#14302a", "#def6ea", "#22694f", "#a5dcc0", "#16332b", "#dbf3e6", "#8af0c3", "#0d5e39"]
    })
    readonly property var _acc: accents[Ui.accent] !== undefined ? accents[Ui.accent] : accents.cyan
    function _a(i) { return dark ? _acc[i] : _acc[i + 1] }

    // ---- surfaces -----------------------------------------------------------------------------
    // Dark: deep graphite with a faint cool tint, cards a step up, tiles another (a gaming look that
    // keeps text contrast above 7:1).
    readonly property color bg: pick("#111317", "#f3f5f8")
    readonly property color sidebar: pick("#0d0f12", "#e9edf2")
    readonly property color card: pick("#181b20", "#ffffff")
    readonly property color cardLine: pick("#252a31", "#dde2e9")
    readonly property color tile: pick("#1e2228", "#e8ecf1")
    readonly property color tileHover: pick("#262b33", "#dfe4ea")
    readonly property color tileLine: pick("#2b323d", "#d3d9e1")
    readonly property color selBg: _a(6)
    readonly property color field: pick("#15181c", "#ffffff")
    readonly property color sep: pick("#272a30", "#dde2e9")
    readonly property color dialog: pick("#1d1f24", "#ffffff")
    readonly property color scrim: pick("#99000000", "#66000000")

    // ---- text ---------------------------------------------------------------------------------
    readonly property color text: pick("#e8edf3", "#1b1f27")
    readonly property color muted: pick("#8892a2", "#5f6877")
    readonly property color stat: pick("#aab3c0", "#4a5361")
    readonly property color disabledText: pick("#5d646e", "#9aa3b0")

    // ---- accent and status --------------------------------------------------------------------
    readonly property color accent: _a(0)
    readonly property color accentInk: pick("#031418", "#ffffff")
    readonly property color accentSoft: _a(2)
    readonly property color accentLine: _a(4)
    readonly property color ok: pick("#34c759", "#1c8a3f")
    readonly property color warn: pick("#ffb347", "#a65f00")
    readonly property color bad: pick("#ff5d6c", "#c0283a")

    // ---- controls -----------------------------------------------------------------------------
    readonly property color button: pick("#171d26", "#ffffff")
    readonly property color buttonLine: pick("#222b37", "#cdd4de")
    readonly property color buttonHover: pick("#1d2530", "#eef2f7")
    readonly property color buttonOff: pick("#17191d", "#eef0f3")
    readonly property color buttonOffLine: pick("#20242a", "#dde1e7")
    readonly property color segBg: pick("#202328", "#e8ecf1")
    readonly property color segLine: pick("#2e3239", "#cdd4de")
    readonly property color segOn: pick("#363c46", "#ffffff")
    readonly property color segOnInk: pick("#ffffff", "#171b22")
    readonly property color groove: pick("#2a2e35", "#cfd5de")
    readonly property color fill: pick("#6f7f96", "#8a98ad")
    readonly property color knob: pick("#aab6c8", "#c3ccd9")
    readonly property color knobLine: pick("#3a404a", "#8795a8")
    readonly property color switchTrack: pick("#2a2e35", "#d5dbe3")
    readonly property color switchOff: pick("#4a505a", "#9aa5b5")
    readonly property color swatchLine: pick("#3a4452", "#aab4c3")
    readonly property color focusRing: _a(8)

    // ---- charts -------------------------------------------------------------------------------
    readonly property color chartLine: accent
    readonly property color chartFill: Qt.rgba(accent.r, accent.g, accent.b, dark ? 0.15 : 0.2)
    readonly property color chartGrid: pick("#2b3038", "#d5dbe3")

    // ---- metrics ------------------------------------------------------------------------------
    readonly property int radius: 10
    readonly property int radiusSmall: 7
    readonly property int radiusLarge: 14
    // Settings > Appearance > Density: compact takes about a fifth less room.
    readonly property bool compact: Ui.density === "compact"
    readonly property int spacing: compact ? 8 : 12
    readonly property int pad: compact ? 12 : 16
    readonly property int sidebarWide: 256
    readonly property int sidebarNarrow: 64
    readonly property int controlHeight: compact ? 32 : 38
    readonly property int tileHeight: compact ? 56 : 68
    readonly property int pageMaxWidth: 920
    // Window widths where the layout changes (APP-DESIGN 4.1).
    readonly property int breakIcons: 720
    readonly property int breakMenu: 520
    readonly property int animMs: Ui.reducedMotion ? 0 : 140

    // ---- type (scaled by the font scale setting) ----------------------------------------------
    function fs(px) { return Math.round(px * Ui.fontScale) }
    readonly property int fontBody: fs(14)
    readonly property int fontSmall: fs(12)
    readonly property int fontTitle: fs(22)
    readonly property int fontSection: fs(15)
    readonly property int fontBig: fs(28)
    readonly property string fontFamily: ""   // empty: the system's default font (other scripts fall back by themselves)
}
