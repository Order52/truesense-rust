// The Direction control of the keyboard's Wave: its six directions as icon tiles. The numbers are
// the stored ones (0 left to right ... 5 counter-clockwise; the keyboard receives them plus one,
// confirmed on a PH16-71). `activated` fires only for the person's own pick, like TileRow.
//
//   DirectionPicker { current: tab.direction; onActivated: (d) => tab.setDirection(d) }
import QtQuick
import org.truesense.app

TileRow {
    id: root

    variant: "outlined"
    tileHeight: 72
    wrapLabels: true
    minTileWidth: 104

    model: [
        //% "Left to right"
        { value: 0, label: qsTrId("light_kb_dir_ltr"), icon: "arrow-right", turn: 0 },
        //% "Right to left"
        { value: 1, label: qsTrId("light_kb_dir_rtl"), icon: "arrow-right", turn: 180 },
        //% "Bottom to top"
        { value: 2, label: qsTrId("light_kb_dir_btt"), icon: "arrow-right", turn: 270 },
        //% "Top to bottom"
        { value: 3, label: qsTrId("light_kb_dir_ttb"), icon: "arrow-right", turn: 90 },
        //% "Clockwise"
        { value: 4, label: qsTrId("light_kb_dir_cw"), icon: "rotate-cw" },
        //% "Counter-clockwise"
        { value: 5, label: qsTrId("light_kb_dir_ccw"), icon: "rotate-cw", mirror: true }
    ]

    //% "Direction"
    Accessible.name: qsTrId("light_kb_direction")
}
