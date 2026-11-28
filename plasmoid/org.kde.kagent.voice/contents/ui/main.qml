// KAgent Voice: polls kagentd's voice levels endpoint and draws the waveform of whoever
// is speaking (you or the agent), plus what the agent is doing.
//
// Polling instead of a WebSocket: the QtWebSockets QML module is not part of a default
// Plasma install. A 50 ms poll of a ~1 KB JSON on localhost is negligible.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.plasma5support as P5Support
import org.kde.kirigami as Kirigami

PlasmoidItem {
    id: root

    readonly property string baseUrl: "http://127.0.0.1:" + Plasmoid.configuration.port
    property string token: ""
    property string agentState: "offline"
    property string detail: ""
    property int seq: 0
    property bool busy: false
    property int failures: 0
    property var samples: []
    property int alertsOpen: 0
    // The running screen watch (screen-watch.md): the minutes left (0: none runs) and its goal, from GET /api/screen-watch.
    property int watchMinutes: 0
    property string watchGoal: ""
    readonly property int maxSamples: 256

    readonly property var stateLabels: ({
        offline: i18n("KAgent is not running"),
        idle: i18n("Idle"),
        thinking: i18n("Thinking..."),
        tool: i18n("Running a tool"),
        approval: i18n("Waiting for your approval"),
        speaking: i18n("Speaking"),
        listening: i18n("Listening"),
    })
    readonly property string statusText: {
        const base = stateLabels[agentState] || agentState
        return detail ? base + ": " + detail : base
    }

    toolTipMainText: i18n("KAgent")
    toolTipSubText: {
        let text = alertsOpen > 0 ? statusText + "\n" + i18np("%1 open alert", "%1 open alerts", alertsOpen) : statusText
        if (watchMinutes > 0)
            text += "\n" + i18n("Watching the screen, %1 min left: %2", watchMinutes, watchGoal)
        return text
    }
    Plasmoid.icon: agentState === "approval" ? "dialog-warning"
        : agentState === "offline" ? "audio-input-microphone-muted" : "audio-input-microphone"
    Plasmoid.status: agentState === "approval" || alertsOpen > 0 ? PlasmaCore.Types.NeedsAttentionStatus
        : agentState === "offline" ? PlasmaCore.Types.PassiveStatus : PlasmaCore.Types.ActiveStatus

    // ---------------------------------------------------------------- token

    function expandPath(p) {
        // The executable engine runs through a shell; quote everything except a leading ~.
        const rest = p.startsWith("~/") ? p.slice(2) : p
        const quoted = "'" + rest.replace(/'/g, "'\\''") + "'"
        return p.startsWith("~/") ? "\"$HOME\"/" + quoted : quoted
    }

    P5Support.DataSource {
        id: shell
        engine: "executable"
        connectedSources: []
        onNewData: (source, data) => {
            disconnectSource(source)
            const t = (data["stdout"] || "").trim()
            if (/^[0-9a-f]{32,128}$/.test(t)) {
                root.token = t
                root.failures = 0
            } else {
                root.token = ""
                root.agentState = "offline"
            }
        }
    }

    function loadToken() {
        shell.connectSource("cat " + expandPath(Plasmoid.configuration.tokenPath) + " 2>/dev/null")
    }

    Connections {
        target: Plasmoid.configuration
        function onTokenPathChanged() { root.loadToken() }
        function onPortChanged() { root.seq = 0; root.loadToken() }
    }

    // ---------------------------------------------------------------- polling

    function request(method, path, onDone) {
        const xhr = new XMLHttpRequest()
        xhr.open(method, baseUrl + path)
        xhr.setRequestHeader("Authorization", "Bearer " + token)
        xhr.timeout = 2000
        xhr.onreadystatechange = function () {
            if (xhr.readyState === XMLHttpRequest.DONE)
                onDone(xhr)
        }
        xhr.send()
    }

    function pushSamples(frames) {
        // A fresh array each tick so `samplesChanged` fires and every Waveform repaints.
        const out = samples.slice(Math.max(0, samples.length - maxSamples))
        if (frames.length === 0) {
            out.push({ v: 0, who: "" })        // keep scrolling: silence
        } else {
            for (const f of frames)
                for (const v of f.bands)
                    out.push({ v: v, who: f.who })
        }
        if (out.length > maxSamples)
            out.splice(0, out.length - maxSamples)
        samples = out
    }

    function poll() {
        if (busy || token === "")
            return
        busy = true
        request("GET", "/api/voice/levels?since=" + seq, function (xhr) {
            busy = false
            if (xhr.status === 200) {
                failures = 0
                const d = JSON.parse(xhr.responseText)
                if (d.seq < seq)              // daemon restarted
                    seq = 0
                else
                    seq = d.seq
                agentState = d.state || "idle"
                detail = d.detail || ""
                pushSamples(d.frames || [])
            } else {
                failures += 1
                agentState = "offline"
                detail = ""
                if (xhr.status === 401)       // token rotated: re-read it
                    loadToken()
                pushSamples([])
            }
        })
    }

    Timer {
        // Back off to 2 s while the daemon is unreachable.
        interval: root.failures > 3 ? 2000 : Math.round(1000 / Math.max(5, Math.min(30, Plasmoid.configuration.fps)))
        running: root.token !== ""
        repeat: true
        onTriggered: root.poll()
    }
    Timer {
        // Retry reading the token while it is missing (daemon not started yet).
        interval: 5000
        running: root.token === ""
        repeat: true
        triggeredOnStart: true
        onTriggered: root.loadToken()
    }

    // Open alerts (watchers / intrusion / network) for the badge.
    Timer {
        interval: 5000
        running: root.token !== ""
        repeat: true
        triggeredOnStart: true
        onTriggered: root.request("GET", "/api/state", function (xhr) {
            if (xhr.status === 200)
                root.alertsOpen = JSON.parse(xhr.responseText).alertsOpen || 0
        })
        root.request("GET", "/api/screen-watch", function (xhr) {
            if (xhr.status !== 200)
                return
            const w = JSON.parse(xhr.responseText)
            if (w && w.state === "watching") {
                root.watchMinutes = Math.max(1, Math.ceil((Date.parse(w.until) - Date.now()) / 60000))
                root.watchGoal = w.goal || ""
            } else {
                root.watchMinutes = 0
                root.watchGoal = ""
            }
        })
    }

    function stopAll() {
        if (token !== "")
            request("POST", "/api/stop-all", function () {})
    }
    function demo() {
        if (token !== "")
            request("POST", "/api/voice/demo", function () {})
    }
    function openUi() {
        Qt.openUrlExternally(baseUrl + "/#token=" + token)
    }

    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18n("Open KAgent")
            icon.name: "internet-web-browser"
            enabled: root.token !== ""
            onTriggered: root.openUi()
        },
        PlasmaCore.Action {
            text: i18n("Stop everything")
            icon.name: "process-stop"
            enabled: root.token !== ""
            onTriggered: root.stopAll()
        },
        PlasmaCore.Action {
            text: i18n("Test waveform")
            icon.name: "media-playback-start"
            enabled: root.token !== ""
            onTriggered: root.demo()
        }
    ]

    // ---------------------------------------------------------------- views

    compactRepresentation: MouseArea {
        Layout.minimumWidth: Kirigami.Units.gridUnit * 4
        Layout.preferredWidth: Kirigami.Units.gridUnit * 6
        Layout.minimumHeight: Kirigami.Units.gridUnit
        hoverEnabled: true
        onClicked: root.expanded = !root.expanded

        Waveform {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing / 2
            samples: root.samples
            agentState: root.agentState
            style: Plasmoid.configuration.style
        }
        // The eye of a running screen watch: the owner is never watched without seeing it.
        Row {
            visible: root.watchMinutes > 0
            anchors.top: parent.top
            anchors.left: parent.left
            spacing: 2
            Kirigami.Icon {
                source: "view-visible"
                width: Math.round(parent.parent.height * 0.5)
                height: width
            }
            PlasmaComponents.Label {
                text: i18n("%1 min", root.watchMinutes)
                font.pixelSize: parent.parent.height * 0.4
                font.bold: true
            }
        }
        Rectangle {
            visible: root.alertsOpen > 0
            anchors.top: parent.top
            anchors.right: parent.right
            width: Math.max(height, badgeText.implicitWidth + 6)
            height: Math.round(parent.height * 0.55)
            radius: height / 2
            color: Kirigami.Theme.negativeTextColor
            PlasmaComponents.Label {
                id: badgeText
                anchors.centerIn: parent
                text: root.alertsOpen > 99 ? "99+" : root.alertsOpen
                color: "white"
                font.pixelSize: parent.height * 0.7
                font.bold: true
            }
        }
    }

    fullRepresentation: ColumnLayout {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 20
        Layout.preferredHeight: Kirigami.Units.gridUnit * 8
        Layout.minimumWidth: Kirigami.Units.gridUnit * 10
        Layout.minimumHeight: Kirigami.Units.gridUnit * 4
        spacing: Kirigami.Units.smallSpacing

        Waveform {
            Layout.fillWidth: true
            Layout.fillHeight: true
            samples: root.samples
            agentState: root.agentState
            style: Plasmoid.configuration.style
        }

        RowLayout {
            Layout.fillWidth: true
            visible: Plasmoid.configuration.showLabel

            Kirigami.Icon {
                source: Plasmoid.icon
                Layout.preferredWidth: Kirigami.Units.iconSizes.small
                Layout.preferredHeight: Kirigami.Units.iconSizes.small
            }
            PlasmaComponents.Label {
                Layout.fillWidth: true
                text: root.statusText
                elide: Text.ElideRight
                color: root.agentState === "approval" ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            }
            Kirigami.Icon {
                visible: root.watchMinutes > 0
                source: "view-visible"
                Layout.preferredWidth: Kirigami.Units.iconSizes.small
                Layout.preferredHeight: Kirigami.Units.iconSizes.small
            }
            PlasmaComponents.Label {
                visible: root.watchMinutes > 0
                text: i18n("watching, %1 min", root.watchMinutes)
                elide: Text.ElideRight
            }
            PlasmaComponents.Label {
                visible: root.alertsOpen > 0
                text: i18np("%1 alert", "%1 alerts", root.alertsOpen)
                color: Kirigami.Theme.negativeTextColor
            }
            PlasmaComponents.ToolButton {
                icon.name: "internet-web-browser"
                enabled: root.token !== ""
                onClicked: root.openUi()
                PlasmaComponents.ToolTip { text: i18n("Open KAgent") }
            }
            PlasmaComponents.ToolButton {
                icon.name: "process-stop"
                enabled: root.token !== "" && root.agentState !== "idle" && root.agentState !== "offline"
                onClicked: root.stopAll()
                PlasmaComponents.ToolTip { text: i18n("Stop everything") }
            }
        }
    }
}
