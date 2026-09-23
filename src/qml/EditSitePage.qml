import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

// Full-page site-config editor. Replaces the old fixed-size edit dialog:
// the editor fills the window, so no resize grip is needed.
Kirigami.Page {
    id: editSitePage
    title: "Edit " + domain + ".conf"
    property var leftPage: null
    property string domain: ""
    property string initialText: ""

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification)
            leftPage.showLeftNotification(msg, duration, type)
    }
    function goBack() {
        if (leftPage && leftPage.closeSiteEditor)
            leftPage.closeSiteEditor()
    }

    NginxManager {
        id: editNginxman
    }

    Shortcut {
        sequence: "Ctrl+F"
        context: Qt.WindowShortcut
        onActivated: findBar.open()
    }

    Component.onCompleted: {
        siteEditor.text = initialText
        lintTimer.restart()
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.smallSpacing

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            Controls.Button {
                text: "Back"
                icon.name: "go-previous"
                onClicked: {
                    editNginxman.cancelSiteEdit(domain)
                    goBack()
                }
            }
            Item {
                Layout.fillWidth: true
            }
            Controls.Button {
                text: "Save"
                icon.name: "document-save"
                onClicked: {
                    var ok = editNginxman.saveSiteConfig(domain, siteEditor.text)
                    notifyLeft(editNginxman.statusMessage, ok ? 4000 : 8000,
                        ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    if (ok) {
                        goBack()
                    }
                }
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                onClicked: {
                    editNginxman.cancelSiteEdit(domain)
                    goBack()
                }
            }
            Controls.Button {
                text: "Find"
                icon.name: "edit-find"
                onClicked: findBar.open()
            }
        }

        // Find bar (Ctrl+F) — shared component, see EditorFindBar.qml.
        EditorFindBar {
            id: findBar
            Layout.fillWidth: true
            editor: siteEditor
        }

        Controls.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            Controls.TextArea {
                id: siteEditor
                font.family: "monospace"
                wrapMode: TextEdit.NoWrap
                selectByMouse: true
                persistentSelection: true
                // Explicit editor surface: the AppImage deliberately skips
                // the org.kde.desktop style (Qt version-skew risk, see
                // main.rs), and the fallback style renders TextArea flat in
                // the window color. Fixed dark colors keep the editor
                // identical under every style.
                color: "#e8e8e8"
                selectionColor: "#3a6ea5"
                selectedTextColor: "#ffffff"
                background: Rectangle {
                    color: "#1e1e1e"
                    border.color: "#4a4a4a"
                    border.width: 1
                    radius: 4
                }
                onTextChanged: {
                    lintTimer.restart()
                    if (findBar.visible)
                        findBar.refresh()
                }
            }
        }

        // Live lint, debounced — nginx -t remains the gate at Save.
        Timer {
            id: lintTimer
            interval: 400
            repeat: false
            onTriggered: {
                var t = editNginxman.lintSiteDraft(siteEditor.text)
                lintOutput.text = t
                lintDot.color = t === "" ? "#27ae60"
                    : (t.indexOf("[error]") >= 0 ? "#c0392b" : "#e67e22")
                lintStatus.text = t === "" ? "Lint: clean"
                    : (t.indexOf("[error]") >= 0 ? "Lint: errors" : "Lint: warnings")
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            Rectangle {
                id: lintDot
                width: 10
                height: 10
                radius: 5
                color: "#27ae60"
            }
            Controls.Label {
                id: lintStatus
                text: "Lint: clean"
                font.pointSize: 8
            }
        }
        Controls.ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: 84
            visible: lintOutput.text !== ""
            clip: true
            Controls.Label {
                id: lintOutput
                font.family: "monospace"
                font.pointSize: 8
                wrapMode: Text.Wrap
            }
        }
        Controls.Label {
            text: "Save tests with nginx -t first — an invalid file is reverted automatically."
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            opacity: 0.6
            font.pointSize: 8
        }
        Controls.Label {
            text: 'Live lint powered by <a href="https://github.com/walf443/nginx-lint">nginx-lint</a> (MIT).'
            textFormat: Text.RichText
            onLinkActivated: function(link) { Qt.openUrlExternally(link) }
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
            opacity: 0.6
            font.pointSize: 8
        }
    }
}
