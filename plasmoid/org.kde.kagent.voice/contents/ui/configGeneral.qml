import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.SimpleKCM {
    property alias cfg_port: port.value
    property alias cfg_tokenPath: tokenPath.text
    property alias cfg_fps: fps.value
    property alias cfg_showLabel: showLabel.checked
    property string cfg_style

    Kirigami.FormLayout {
        QQC2.ComboBox {
            id: style
            Kirigami.FormData.label: i18n("Style:")
            textRole: "text"
            valueRole: "value"
            model: [
                { text: i18n("Bars"), value: "bars" },
                { text: i18n("Line"), value: "line" }
            ]
            Component.onCompleted: currentIndex = Math.max(0, indexOfValue(cfg_style))
            onActivated: cfg_style = currentValue
        }
        QQC2.SpinBox {
            id: fps
            Kirigami.FormData.label: i18n("Updates per second:")
            from: 5
            to: 30
        }
        QQC2.CheckBox {
            id: showLabel
            Kirigami.FormData.label: i18n("Status line:")
            text: i18n("Show what the agent is doing")
        }
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
        }
        QQC2.SpinBox {
            id: port
            Kirigami.FormData.label: i18n("kagentd port:")
            from: 1
            to: 65535
        }
        QQC2.TextField {
            id: tokenPath
            Kirigami.FormData.label: i18n("Token file:")
            placeholderText: "~/.config/kagent/token"
        }
    }
}
