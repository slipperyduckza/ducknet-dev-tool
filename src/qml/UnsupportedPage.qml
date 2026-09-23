import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: unsupportedPage
    title: "Unsupported System"
    property var devCertsManager: null
    readonly property string distroId: devCertsManager ? devCertsManager.distroId : "unknown"
    readonly property string distroFamily: devCertsManager ? devCertsManager.distroFamily : "unsupported"

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.largeSpacing

        Kirigami.Icon { source: "dialog-warning"; Layout.preferredWidth: 64; Layout.preferredHeight: 64; Layout.alignment: Qt.AlignHCenter }

        Kirigami.Heading {
            text: "Unsupported Distribution"
            level: 2
            Layout.alignment: Qt.AlignHCenter
            color: Kirigami.Theme.negativeTextColor
        }

        Controls.Label {
            text: "This tool currently only supports Debian and Fedora."
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            font.bold: true
        }

        Kirigami.Separator { Layout.fillWidth: true }

        Controls.Label {
            text: "Detected: " + unsupportedPage.distroId + " (family: " + unsupportedPage.distroFamily + ")"
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            opacity: 0.7
            font.pointSize: 9
        }

        Controls.Label {
            text: "Checked /etc/os-release (ID / ID_LIKE). Debian stores the CA at /usr/local/share/ca-certificates/myCA.crt with update-ca-certificates, Fedora at /etc/pki/ca-trust/source/anchors/myCA.crt with update-ca-trust."
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            opacity: 0.6
            font.pointSize: 8
        }

        Controls.Label {
            text: devCertsManager ? devCertsManager.statusMessage : ""
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            color: Kirigami.Theme.disabledTextColor
            font.pointSize: 8
            visible: devCertsManager && devCertsManager.statusMessage
        }

        Item { Layout.fillHeight: true }
    }
}
