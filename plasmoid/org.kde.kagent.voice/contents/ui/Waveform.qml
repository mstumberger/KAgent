// Scrolling waveform of the voice envelope, colored by speaker. While the agent is
// thinking or running a tool and nobody speaks, a scanner sweeps the baseline.
import QtQuick
import org.kde.kirigami as Kirigami

Canvas {
    id: canvas

    // Newest sample last: [{v: 0..1, who: "user"|"assistant"|""}]
    property var samples: []
    property string agentState: "offline"   // speaking|listening|thinking|tool|approval|idle|offline
    property string style: "bars"      // bars | line
    property real scanPhase: 0

    readonly property color assistantColor: Kirigami.Theme.highlightColor
    readonly property color userColor: Kirigami.Theme.positiveTextColor
    readonly property color idleColor: Qt.alpha(Kirigami.Theme.textColor, 0.35)
    readonly property color approvalColor: Kirigami.Theme.negativeTextColor
    readonly property bool scanning: agentState === "thinking" || agentState === "tool" || agentState === "approval"

    function colorFor(who) {
        return who === "user" ? userColor : who === "assistant" ? assistantColor : idleColor
    }

    NumberAnimation on scanPhase {
        running: canvas.scanning && canvas.visible
        from: 0; to: 1
        duration: canvas.agentState === "approval" ? 1400 : 1100
        loops: Animation.Infinite
    }
    onScanPhaseChanged: if (scanning) requestPaint()
    onAgentStateChanged: requestPaint()
    onSamplesChanged: requestPaint()
    onStyleChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()

    onPaint: {
        const ctx = getContext("2d")
        ctx.reset()
        const w = width, h = height, mid = h / 2
        if (w <= 0 || h <= 0)
            return
        const step = Math.max(2, Math.round(w / 64))     // px per sample
        const n = Math.floor(w / step)
        const data = samples.slice(Math.max(0, samples.length - n))
        const x0 = w - data.length * step

        // Baseline.
        ctx.strokeStyle = agentState === "offline" ? Qt.alpha(idleColor, 0.15) : idleColor
        ctx.lineWidth = 1
        ctx.beginPath(); ctx.moveTo(0, mid); ctx.lineTo(w, mid); ctx.stroke()

        if (style === "line") {
            ctx.lineWidth = Math.max(1.5, h / 28)
            ctx.lineJoin = "round"
            for (let i = 1; i < data.length; i++) {
                const a = data[i - 1], b = data[i]
                const sign = (i % 2) ? 1 : -1
                ctx.strokeStyle = colorFor(b.who)
                ctx.beginPath()
                ctx.moveTo(x0 + (i - 1) * step, mid - sign * a.v * mid * 0.9)
                ctx.lineTo(x0 + i * step, mid + sign * b.v * mid * 0.9)
                ctx.stroke()
            }
        } else {
            const bw = Math.max(1, step * 0.6)
            for (let i = 0; i < data.length; i++) {
                const s = data[i]
                if (s.v <= 0.01)
                    continue
                const bh = Math.max(1, s.v * (h - 2))
                ctx.fillStyle = colorFor(s.who)
                ctx.fillRect(x0 + i * step, mid - bh / 2, bw, bh)
            }
        }

        // Scanner (KITT-style) while the agent works silently.
        if (scanning) {
            const sx = (scanPhase < 0.5 ? scanPhase * 2 : (1 - scanPhase) * 2) * w
            const sw = Math.max(12, w / 6)
            const g = ctx.createLinearGradient(sx - sw, 0, sx + sw, 0)
            const c = agentState === "approval" ? approvalColor : assistantColor
            g.addColorStop(0, Qt.alpha(c, 0))
            g.addColorStop(0.5, Qt.alpha(c, 0.9))
            g.addColorStop(1, Qt.alpha(c, 0))
            ctx.fillStyle = g
            ctx.fillRect(sx - sw, mid - Math.max(1.5, h / 14), sw * 2, Math.max(3, h / 7))
        }
    }
}
