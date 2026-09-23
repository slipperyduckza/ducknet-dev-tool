import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// Shared incremental find bar for the config editors (EditSitePage,
// EditNginxBasePage, EditPhpfpmBasePage). The host page sets `editor` to
// the TextArea being searched and calls open() from Ctrl+F / its Find
// button — e.g.:
//
//   EditorFindBar { id: findBar; Layout.fillWidth: true; editor: siteEditor }
//   Shortcut { sequence: "Ctrl+F"; context: Qt.WindowShortcut; onActivated: findBar.open() }
//
// Case-insensitive, wraps around; Enter = next match, Esc = close. The
// cursor jumps to each match (cursorPosition first so the ScrollView
// follows, then select() highlights — the host editor needs
// persistentSelection so the highlight stays visible while this field
// holds focus).
RowLayout {
    id: root
    spacing: Kirigami.Units.smallSpacing
    visible: false

    // TextArea being searched (required — set by the host page).
    property var editor: null
    property var findOffsets: []
    property int findIndex: -1
    property string findLastQuery: ""

    function refresh() {
        if (root.editor === null)
            return
        var q = findField.text
        if (q === "") {
            root.findOffsets = []
            root.findIndex = -1
            findStatus.text = ""
            return
        }
        var hay = root.editor.text.toLowerCase()
        var needle = q.toLowerCase()
        var offsets = []
        var pos = 0
        while (pos <= hay.length - needle.length) {
            var i = hay.indexOf(needle, pos)
            if (i < 0)
                break
            offsets.push(i)
            pos = i + Math.max(1, needle.length)
        }
        root.findOffsets = offsets
        if (offsets.length === 0) {
            root.findIndex = -1
            findStatus.text = "No matches"
        } else {
            // A new query restarts at the first match; edits keep position.
            if (q !== root.findLastQuery || root.findIndex < 0 || root.findIndex >= offsets.length)
                root.findIndex = 0
            showMatch()
        }
        root.findLastQuery = q
    }
    function showMatch() {
        if (root.editor === null)
            return
        var start = root.findOffsets[root.findIndex]
        var len = findField.text.length
        root.editor.cursorPosition = start
        root.editor.select(start, start + len)
        findStatus.text = (root.findIndex + 1) + " of " + root.findOffsets.length
    }
    function step(dir) {
        if (root.findOffsets.length === 0)
            return
        root.findIndex = (root.findIndex + dir + root.findOffsets.length) % root.findOffsets.length
        showMatch()
    }
    function open() {
        root.visible = true
        if (root.editor !== null && root.editor.selectedText !== "")
            findField.text = root.editor.selectedText
        findField.forceActiveFocus()
        findField.selectAll()
        refresh()
    }
    function close() {
        root.visible = false
        if (root.editor !== null) {
            root.editor.deselect()
            root.editor.forceActiveFocus()
        }
    }

    Controls.TextField {
        id: findField
        Layout.fillWidth: true
        placeholderText: "Find…"
        onTextChanged: root.refresh()
        onAccepted: root.step(1)
        Keys.onEscapePressed: root.close()
    }
    Controls.Label {
        id: findStatus
        font.pointSize: 8
        opacity: 0.7
    }
    Controls.ToolButton {
        icon.name: "go-up"
        Accessible.name: "Previous match (wraps around)"
        enabled: root.findOffsets.length > 0
        onClicked: root.step(-1)
    }
    Controls.ToolButton {
        icon.name: "go-down"
        Accessible.name: "Next match (wraps around)"
        enabled: root.findOffsets.length > 0
        onClicked: root.step(1)
    }
    Controls.ToolButton {
        icon.name: "dialog-close"
        Accessible.name: "Close find bar"
        onClicked: root.close()
    }
}
