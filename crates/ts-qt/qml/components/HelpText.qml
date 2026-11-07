// A paragraph of explanation under a control. Muted, wraps, selectable text is not needed.
import QtQuick
import QtQuick.Layouts
import org.truesense.app

Text {
    color: Theme.muted
    font.pixelSize: Theme.fontSmall + 1
    wrapMode: Text.Wrap
    lineHeight: 1.15
    Layout.fillWidth: true
}
