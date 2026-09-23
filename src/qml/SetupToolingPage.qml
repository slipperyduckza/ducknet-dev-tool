import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.ducknetdevtool

Kirigami.Page {
    id: setupToolingPage
    title: "Setup Tooling"
    property var leftPage: null

    function notifyLeft(msg, duration, type) {
        if (leftPage && leftPage.showLeftNotification)
            leftPage.showLeftNotification(msg, duration, type)
    }

    ToolingManager {
        id: toolingman
    }

    Component.onCompleted: toolingman.refreshStatus()

    Controls.ScrollView {
        id: toolingScroll
        anchors.fill: parent
        contentWidth: availableWidth
        Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
        Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

        ColumnLayout {
            // -24 keeps content clear of the overlaid AlwaysOn scrollbar.
            width: toolingScroll.availableWidth - 24
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading { text: "Development Environment Packages Setup and Configuration"; level: 2; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Controls.Label {
                text: "In this section we can set up various configurations that may be required for your Developers, in this section we can also install the required packages that make this system a development system."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Access Permissions ----
            Kirigami.Heading { text: "Access Permissions"; level: 3 }
            Controls.Label {
                text: "These buttons give your DEV user passwordless privilege access on the console and in KDE Plasma - USE WITH CAUTION"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "PREREQUISITE TOOLING:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: toolingman.prereqOk ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: "Activate Prerequisites"
                    icon.name: "system-software-install"
                    enabled: !toolingman.prereqOk
                    onClicked: {
                        // Fresh Debian installs leave the user outside the
                        // sudo group — that must be fixed (via root + reboot)
                        // before any package install can run.
                        if (toolingman.needsSudoGroup) {
                            sudoDialog.open()
                            return
                        }
                        // Fedora with SELinux still enforcing gets its own
                        // confirm dialog (install + disable + reboot).
                        if (toolingman.prereqVariant === "Fedora" && !toolingman.selinuxOff) {
                            selinuxDialog.open()
                            return
                        }
                        var ok = toolingman.installPrerequisites(false)
                        toolingman.refreshStatus()
                        setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 6000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
            }
            Controls.Label {
                text: toolingman.prereqVariant === "Fedora" ? "Requires: " + toolingman.prereqPackages + " + SELINUX=disabled (Fedora) — installs in a terminal window so progress stays visible." : toolingman.prereqVariant !== "" ? "Requires: " + toolingman.prereqPackages + " (" + toolingman.prereqVariant + ") — installs in a terminal window so progress stays visible." : "Prerequisites are only wired for Debian/Fedora-family systems — this host reports '" + toolingman.distroFamily + "'."
                visible: !toolingman.prereqOk
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "PASSWORDLESS SUDO (CONSOLE):"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: toolingman.sudoNopasswd ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: "Enable Passwordless Sudo"
                    icon.name: "dialog-password"
                    enabled: !toolingman.sudoNopasswd
                    onClicked: {
                        var ok = toolingman.enablePasswordlessSudo()
                        toolingman.refreshStatus()
                        setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
                Controls.Button {
                    text: "Disable Passwordless Sudo"
                    icon.name: "dialog-cancel"
                    enabled: toolingman.sudoNopasswd
                    onClicked: {
                        var ok = toolingman.disablePasswordlessSudo()
                        toolingman.refreshStatus()
                        setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "PASSWORDLESS PKEXEC (PLASMA):"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: toolingman.pkexecNopasswd ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: toolingman.pkexecVariant !== "" ? "Enable Passwordless Pkexec (" + toolingman.pkexecVariant + ")" : "Enable Passwordless Pkexec"
                    icon.name: "dialog-password"
                    enabled: !toolingman.pkexecNopasswd && toolingman.pkexecVariant !== ""
                    onClicked: {
                        var ok = toolingman.enablePasswordlessPkexec()
                        toolingman.refreshStatus()
                        setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
                Controls.Button {
                    text: "Disable Passwordless Pkexec"
                    icon.name: "dialog-cancel"
                    enabled: toolingman.pkexecNopasswd && toolingman.pkexecVariant !== ""
                    onClicked: {
                        var ok = toolingman.disablePasswordlessPkexec()
                        toolingman.refreshStatus()
                        setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
            }
            Controls.Label {
                text: "Passwordless pkexec is only wired for Debian/Fedora-family systems — this host reports '" + toolingman.distroFamily + "'."
                visible: toolingman.pkexecVariant === ""
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }
            Controls.Label {
                text: "pkexec is not installed — it will be installed first when you click the button above (needs root)."
                visible: !toolingman.pkexecInstalled && toolingman.pkexecVariant !== ""
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.6
                font.pointSize: 8
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Install NGINX and PHP ----
            Kirigami.Heading { text: "Install NGINX and PHP"; level: 3 }
            Controls.Label {
                text: "This tool will help you install NGINX and PHP (8.4) on your Development System"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "NGINX + PHP INSTALLED:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: toolingman.stackInstalled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: "Install NGINX PHP"
                    icon.name: "system-software-install"
                    enabled: !toolingman.stackInstalled && !toolingman.opActive
                    onClicked: {
                        if (toolingman.startPackageOp("install")) {
                            opDialog.open()
                        } else {
                            setupToolingPage.notifyLeft(toolingman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
                Controls.Button {
                    text: "Uninstall NGINX PHP"
                    icon.name: "dialog-cancel"
                    enabled: !toolingman.opActive
                    onClicked: {
                        if (toolingman.startPackageOp("uninstall")) {
                            opDialog.open()
                        } else {
                            setupToolingPage.notifyLeft(toolingman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Development Environment tools ----
            Kirigami.Heading { text: "Development Environment tools"; level: 3 }
            Controls.Label {
                text: "This button will install the tools to develop and compile in Languages such as C, C++, Python and Rust - click the button to install"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Button {
                    text: "Install Dev Env"
                    icon.name: "system-software-install"
                    enabled: !toolingman.opActive
                    onClicked: {
                        if (toolingman.startPackageOp("devenv")) {
                            opDialog.open()
                        } else {
                            setupToolingPage.notifyLeft(toolingman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            // ---- Developer Places/Folders & Optional tools ----
            Kirigami.Heading { text: "Developer Places/Folders & Optional tools"; level: 3 }
            Controls.Label {
                text: "This button will add a Coding, MyApps and WebRoots folder to your FileBrowser to help standardize output locations for your developers."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.7
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "DEV FOLDERS ADDED:"; font.bold: true }
                Rectangle {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    radius: 7
                    color: toolingman.devPlacesAdded ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.negativeTextColor
                }
                Controls.Button {
                    text: "Add Dev Folders"
                    icon.name: "folder-new"
                    enabled: !toolingman.devPlacesAdded
                    onClicked: {
                        var ok = toolingman.addDevFolders()
                        toolingman.refreshStatus()
                        setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 4000 : 8000,
                            ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "Install VSCodium:"; font.bold: true }
                Controls.Button {
                    text: "Install VSCodium"
                    icon.name: "system-software-install"
                    enabled: !toolingman.opActive
                    onClicked: {
                        if (toolingman.startPackageOp("vscodium")) {
                            opDialog.open()
                        } else {
                            setupToolingPage.notifyLeft(toolingman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.Label { text: "Install Opencode CLI:"; font.bold: true }
                Controls.Button {
                    text: "Install Opencode"
                    icon.name: "system-software-install"
                    enabled: !toolingman.opActive
                    onClicked: {
                        if (toolingman.startPackageOp("opencode")) {
                            opDialog.open()
                        } else {
                            setupToolingPage.notifyLeft(toolingman.statusMessage, 8000, Kirigami.MessageType.Error)
                        }
                    }
                }
            }
        }
    }

    // Sudo-group gate (fresh Debian installs): the user must join the
    // sudo group via the ROOT password first, then reboot immediately —
    // group membership only takes effect on a fresh login.
    Controls.Dialog {
        id: sudoDialog
        title: "SUDO Group Membership Required"
        modal: true
        width: Math.min(480, setupToolingPage.width - 40)
        // Explicit height like opDialog below: content-driven height would
        // loop with the centering y binding.
        height: Math.min(280, setupToolingPage.height - 40)
        x: (setupToolingPage.width - width) / 2
        y: (setupToolingPage.height - height) / 2
        standardButtons: Controls.Dialog.NoButton
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                text: "You must be a member of the SUDO group for this tool."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                font.bold: true
            }
            Controls.Label {
                text: "Note: A reboot is required immediately after adding you to the SUDO group."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.8
            }
            Controls.Label {
                text: "Do you wish to continue?"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.8
            }
        }
        footer: Controls.DialogButtonBox {
            Controls.Button {
                text: "Add to SUDO Group"
                icon.name: "dialog-password"
                Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.AcceptRole
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.RejectRole
            }
        }
        onAccepted: {
            var ok = toolingman.addUserToSudoGroup()
            toolingman.refreshStatus()
            setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 6000 : 8000,
                ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
        }
    }

    // SELinux gate (Fedora): our tooling needs SELinux disabled. The
    // terminal installs the packages, disables SELinux, then reboots
    // immediately — the config change is only permanent after a reboot.
    Controls.Dialog {
        id: selinuxDialog
        title: "SELINUX Disable Required"
        modal: true
        width: Math.min(480, setupToolingPage.width - 40)
        height: Math.min(300, setupToolingPage.height - 40)
        x: (setupToolingPage.width - width) / 2
        y: (setupToolingPage.height - height) / 2
        standardButtons: Controls.Dialog.NoButton
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                text: "SELINUX needs to be disabled for this tooling to work in your DEV environment."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                font.bold: true
            }
            Controls.Label {
                text: "Running this script will disable SELINUX and Reboot immediately to ensure that it is permanent."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.8
            }
            Controls.Label {
                text: "Do you wish to continue?"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                opacity: 0.8
            }
        }
        footer: Controls.DialogButtonBox {
            Controls.Button {
                text: "Yes Disable and Reboot"
                icon.name: "dialog-password"
                Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.AcceptRole
            }
            Controls.Button {
                text: "Cancel"
                icon.name: "dialog-cancel"
                Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.RejectRole
            }
        }
        onAccepted: {
            var ok = toolingman.installPrerequisites(true)
            toolingman.refreshStatus()
            setupToolingPage.notifyLeft(toolingman.statusMessage, ok ? 6000 : 8000,
                ok ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
        }
    }

    // Package-op progress dialog + poller. The op runs for minutes in the
    // background; the timer streams the log and auto-closes on completion.
    // No cancel: killing apt/dnf mid-transaction risks a dpkg/rpm lock mess.
    Controls.Dialog {
        id: opDialog
        title: toolingman.opName
        modal: true
        width: Math.min(720, setupToolingPage.width - 40)
        height: Math.min(480, setupToolingPage.height - 40)
        x: (setupToolingPage.width - width) / 2
        y: (setupToolingPage.height - height) / 2
        closePolicy: toolingman.opActive ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.BusyIndicator { running: toolingman.opActive; Layout.preferredWidth: 22; Layout.preferredHeight: 22 }
                Controls.Label {
                    text: toolingman.opActive ? "Working — this takes a few minutes, please wait…" : (toolingman.opOk ? "Done." : "Finished with errors — see log above.")
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    opacity: 0.8
                }
            }
            Controls.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                Controls.ScrollBar.vertical.policy: Controls.ScrollBar.AlwaysOn
                Controls.TextArea {
                    id: opLogView
                    text: toolingman.opLog
                    readOnly: true
                    font.family: "monospace"
                    font.pointSize: 8
                    wrapMode: TextEdit.NoWrap
                    selectByMouse: true
                    onTextChanged: cursorPosition = length
                }
            }
        }
        footer: Controls.DialogButtonBox {
            Controls.Button {
                text: "Close"
                enabled: !toolingman.opActive
                Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.RejectRole
            }
        }
    }
    Timer {
        id: opTimer
        interval: 1000
        running: opDialog.visible || toolingman.opActive
        repeat: true
        triggeredOnStart: false
        property bool wasActive: false
        onTriggered: {
            toolingman.pollPackageOp()
            if (wasActive && !toolingman.opActive) {
                opDialog.close()
                setupToolingPage.notifyLeft(toolingman.statusMessage, toolingman.opOk ? 4000 : 8000,
                    toolingman.opOk ? Kirigami.MessageType.Positive : Kirigami.MessageType.Error)
                toolingman.refreshStatus()
            }
            wasActive = toolingman.opActive
        }
    }
}
