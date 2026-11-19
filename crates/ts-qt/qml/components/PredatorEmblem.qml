// The PREDATOR emblem: Acer's own artwork (the outline of PredatorSense's header logo, 26 x 32;
// see THIRD_PARTY.md), filled in one colour. Scales to its height.
//
//   PredatorEmblem { height: 20; color: Theme.accent }
import QtQuick
import QtQuick.Shapes

Shape {
    id: emblem

    property color color: "white"
    readonly property real k: height / 32

    implicitHeight: 32
    width: 26 * k
    Accessible.ignored: true

    ShapePath {
        strokeWidth: -1
        fillColor: emblem.color
        scale: Qt.size(emblem.k, emblem.k)
        PathSvg {
            path: "M26 0V18.221l-5.557 5.25V18.8l1.788 -1.69V12.59l-2.919 4.271V32l-2.627 -6.252L13.7 22.929V3.781l3.411 8.891 5.651 -5.341ZM0 0V18.221l5.556 5.25V18.8l-1.787 -1.69V12.59l2.918 4.271V32l2.627 -6.252L12.3 22.929V3.781l-3.41 8.891 -5.652 -5.341Z"
        }
    }
}
