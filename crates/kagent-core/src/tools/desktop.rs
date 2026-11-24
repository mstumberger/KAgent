//! Desktop and hardware control (docs/specs/tools.md section 9): audio, bluetooth,
//! wifi/vpn, display, windows, notifications, open. All unprivileged; mutating calls
//! are journaled with an undo where one exists. KDE Plasma 6 first, freedesktop CLIs
//! as the transport (wpctl, bluetoothctl, nmcli, qdbus6, KWin scripting).

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use super::privop::record_user;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

pub(super) fn contract(name: &str, description: &str, schema: Value) -> ToolContract {
    ToolContract::read_only(name, description, schema).mutating(SideEffect::System, Risk::Low)
}

pub(super) fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub(super) fn cmd_undo(argv: &[&str]) -> Value {
    json!({"kind": "user", "action": "command", "argv": argv})
}

// ---------------------------------------------------------------- audio

#[derive(Debug, Clone, PartialEq)]
pub struct WpNode {
    pub id: u32,
    pub name: String,
    pub kind: &'static str, // sink | source
    pub default: bool,
    pub volume: Option<f32>,
}

/// Parse the Audio section of `wpctl status` into sinks and sources.
pub fn parse_wpctl(status: &str) -> Vec<WpNode> {
    let mut out = Vec::new();
    let mut in_audio = false;
    let mut kind: Option<&'static str> = None;
    for line in status.lines() {
        let t = line.trim_start_matches([' ', '│', '├', '└', '─']).trim();
        if !line.starts_with(' ') && !line.starts_with('│') && !line.trim().is_empty() {
            in_audio = line.trim() == "Audio";
            kind = None;
            continue;
        }
        if !in_audio {
            continue;
        }
        if t.ends_with(':') {
            kind = match t {
                "Sinks:" => Some("sink"),
                "Sources:" => Some("source"),
                _ => None,
            };
            continue;
        }
        let Some(k) = kind else { continue };
        let default = t.starts_with('*');
        let t = t.trim_start_matches('*').trim();
        let Some((id, rest)) = t.split_once(". ") else { continue };
        let Ok(id) = id.trim().parse::<u32>() else { continue };
        let (name, volume) = match rest.rfind("[vol:") {
            Some(i) => (rest[..i].trim(), rest[i + 5..].trim_end_matches(']').split_whitespace().next().and_then(|v| v.parse().ok())),
            None => (rest.trim(), None),
        };
        out.push(WpNode { id, name: name.to_string(), kind: k, default, volume });
    }
    out
}

fn find_node<'a>(nodes: &'a [WpNode], target: &str, kind: Option<&str>) -> Option<&'a WpNode> {
    let t = target.trim().to_lowercase();
    let pool = nodes.iter().filter(|n| kind.is_none_or(|k| n.kind == k));
    if let Ok(id) = t.parse::<u32>() {
        return nodes.iter().find(|n| n.id == id);
    }
    pool.clone().find(|n| n.name.to_lowercase() == t).or_else(|| pool.clone().find(|n| n.name.to_lowercase().contains(&t)))
}

pub struct AudioControl {
    contract: ToolContract,
}

impl AudioControl {
    pub fn new() -> Self {
        Self {
            contract: contract(
                "audio_control",
                "Change audio: set the default output/input device, set volume, mute/unmute. `target` is a device id or part of its name (see the audio tool). Undoable.",
                json!({"type":"object","required":["action","target"],"properties":{
                    "action":{"type":"string","enum":["set_default","set_volume","mute","unmute"]},
                    "target":{"type":"string","description":"device id or name substring, e.g. 'headphones', 'A05', 'HDMI'"},
                    "kind":{"type":"string","enum":["sink","source"],"description":"sink = output (default), source = microphone"},
                    "volume":{"type":"number","description":"set_volume: 0.0-1.5 (1.0 = 100%)"}
                }}),
            ),
        }
    }
}

#[async_trait]
impl Tool for AudioControl {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let summary = format!("audio {} {}", s(input, "action"), s(input, "target"));
        CallAssessment::from_contract(&self.contract, summary, format!("audio_control {}", s(input, "action")))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let status = exec(&ctx, &["wpctl", "status"]).run().await?;
        let nodes = parse_wpctl(&status.stdout);
        let kind = match s(&input, "kind") { "source" => "source", _ => "sink" };
        let Some(node) = find_node(&nodes, s(&input, "target"), Some(kind)).or_else(|| find_node(&nodes, s(&input, "target"), None)).cloned() else {
            let names: Vec<String> = nodes.iter().map(|n| format!("{} {} ({})", n.id, n.name, n.kind)).collect();
            return Ok(ToolOutput::error(format!("no audio device matches '{}'. Devices: {}", s(&input, "target"), names.join("; "))));
        };
        let id = node.id.to_string();
        let (argv, undo, before): (Vec<String>, Option<Value>, String) = match s(&input, "action") {
            "set_default" => {
                let prev = nodes.iter().find(|n| n.default && n.kind == node.kind).map(|n| n.id.to_string());
                let undo = prev.as_ref().filter(|p| **p != id).map(|p| cmd_undo(&["wpctl", "set-default", p]));
                (vec!["wpctl".into(), "set-default".into(), id.clone()], undo, format!("default {}: {}", node.kind, prev.unwrap_or_default()))
            }
            "set_volume" => {
                let v = input.get("volume").and_then(Value::as_f64).unwrap_or(-1.0);
                if !(0.0..=1.5).contains(&v) {
                    return Ok(ToolOutput::error("volume must be 0.0-1.5"));
                }
                let prev = node.volume.map(|p| format!("{p:.2}"));
                let undo = prev.as_ref().map(|p| cmd_undo(&["wpctl", "set-volume", &id, p]));
                (vec!["wpctl".into(), "set-volume".into(), id.clone(), format!("{v:.2}")], undo, format!("volume {}", prev.unwrap_or("?".into())))
            }
            a @ ("mute" | "unmute") => {
                let flag = if a == "mute" { "1" } else { "0" };
                let back = if a == "mute" { "0" } else { "1" };
                (vec!["wpctl".into(), "set-mute".into(), id.clone(), flag.into()], Some(cmd_undo(&["wpctl", "set-mute", &id, back])), String::new())
            }
            other => return Ok(ToolOutput::error(format!("unknown action {other}"))),
        };
        let r = exec(&ctx, &argv).run().await?;
        record_user(&ctx, "audio_control", &format!("{} {} ({})", s(&input, "action"), node.name, node.id), json!({"kind": "user", "action": "command", "argv": argv}), r.success(), undo, Some(before), None, &r.render())
    }
}

// ---------------------------------------------------------------- bluetooth

pub struct BluetoothControl {
    contract: ToolContract,
}

impl BluetoothControl {
    pub fn new() -> Self {
        Self {
            contract: contract(
                "bluetooth_control",
                "Bluetooth: power the adapter on/off, connect or disconnect a paired device (by MAC or name). Undoable.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["power_on","power_off","connect","disconnect"]},
                    "device":{"type":"string","description":"MAC address or part of the device name"}
                }}),
            ),
        }
    }
}

/// `bluetoothctl devices` lines: `Device AA:BB:CC:DD:EE:FF Name`.
pub fn parse_bt_devices(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.strip_prefix("Device "))
        .filter_map(|l| l.split_once(' ').map(|(m, n)| (m.to_string(), n.trim().to_string())))
        .collect()
}

#[async_trait]
impl Tool for BluetoothControl {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let summary = format!("bluetooth {} {}", s(input, "action"), s(input, "device"));
        CallAssessment::from_contract(&self.contract, summary.trim().to_string(), format!("bluetooth {}", s(input, "action")))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let action = s(&input, "action").to_string();
        let (argv, undo, label): (Vec<String>, Option<Value>, String) = match action.as_str() {
            "power_on" => (vec!["bluetoothctl".into(), "power".into(), "on".into()], Some(cmd_undo(&["bluetoothctl", "power", "off"])), "adapter".into()),
            "power_off" => (vec!["bluetoothctl".into(), "power".into(), "off".into()], Some(cmd_undo(&["bluetoothctl", "power", "on"])), "adapter".into()),
            "connect" | "disconnect" => {
                let devs = parse_bt_devices(&exec(&ctx, &["bluetoothctl", "devices"]).run().await?.stdout);
                let want = s(&input, "device").to_lowercase();
                let Some((mac, name)) = devs.iter().find(|(m, n)| m.to_lowercase() == want || n.to_lowercase().contains(&want)).cloned().filter(|_| !want.is_empty()) else {
                    let list: Vec<String> = devs.iter().map(|(m, n)| format!("{n} ({m})")).collect();
                    return Ok(ToolOutput::error(format!("no paired device matches '{want}'. Paired: {}", list.join(", "))));
                };
                let back = if action == "connect" { "disconnect" } else { "connect" };
                (vec!["bluetoothctl".into(), action.clone(), mac.clone()], Some(cmd_undo(&["bluetoothctl", back, &mac])), format!("{name} ({mac})"))
            }
            other => return Ok(ToolOutput::error(format!("unknown action {other}"))),
        };
        let r = exec(&ctx, &argv).run().await?;
        // bluetoothctl exits 0 even on "Failed to connect": check the text too.
        let ok = r.success() && !r.stdout.contains("Failed") && !r.stdout.contains("not available");
        record_user(&ctx, "bluetooth_control", &format!("bluetooth {action} {label}"), json!({"kind": "user", "action": "command", "argv": argv}), ok, undo, None, None, &r.render())
    }
}

// ---------------------------------------------------------------- wifi / vpn

pub struct Wifi {
    contract: ToolContract,
}

impl Wifi {
    pub fn new() -> Self {
        Self {
            contract: contract(
                "wifi",
                "Wi-Fi and VPN via NetworkManager: list networks (read-only), connect to a network, disconnect Wi-Fi, bring a saved VPN/connection up or down.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["list","connect","disconnect","vpn_up","vpn_down"]},
                    "ssid":{"type":"string"},
                    "password":{"type":"string","description":"only for new networks; never logged"},
                    "connection":{"type":"string","description":"vpn_up/vpn_down: saved connection name"}
                }}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Wifi {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = s(input, "action");
        // Never put the password in summary/rule_key (audit redacts input, not summaries).
        let target = if action.starts_with("vpn") { s(input, "connection") } else { s(input, "ssid") };
        let a = CallAssessment::from_contract(&self.contract, format!("wifi {action} {target}").trim().to_string(), format!("wifi {action} {target}"));
        if action == "list" { a.as_read_only() } else { a }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let action = s(&input, "action").to_string();
        match action.as_str() {
            "list" => {
                let r = exec(&ctx, &["nmcli", "-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY,CHAN", "dev", "wifi", "list"]).run().await?;
                let rows: Vec<String> = r.stdout.lines().map(|l| l.replace(':', "  ")).collect();
                Ok(ToolOutput::ok(format!("IN-USE SSID SIGNAL SECURITY CHAN\n{}", rows.join("\n"))))
            }
            "connect" => {
                let ssid = s(&input, "ssid");
                if ssid.is_empty() {
                    return Ok(ToolOutput::error("ssid required"));
                }
                let mut argv = vec!["nmcli".to_string(), "dev".into(), "wifi".into(), "connect".into(), ssid.into()];
                if !s(&input, "password").is_empty() {
                    argv.extend(["password".into(), s(&input, "password").into()]);
                }
                let r = exec(&ctx, &argv).run().await?;
                let logged = json!({"kind": "user", "action": "command", "argv": ["nmcli", "dev", "wifi", "connect", ssid]});
                record_user(&ctx, "wifi", &format!("connect Wi-Fi {ssid}"), logged, r.success(), None, None, None, &r.render())
            }
            "disconnect" => {
                let dev = exec(&ctx, &["nmcli", "-t", "-f", "DEVICE,TYPE,STATE", "dev"]).run().await?.stdout;
                let Some(wdev) = dev.lines().find(|l| l.contains(":wifi:connected")).and_then(|l| l.split(':').next()).map(String::from) else {
                    return Ok(ToolOutput::error("no connected Wi-Fi device"));
                };
                let r = exec(&ctx, &["nmcli", "dev", "disconnect", &wdev]).run().await?;
                record_user(&ctx, "wifi", &format!("disconnect Wi-Fi ({wdev})"), json!({"kind": "user", "action": "command", "argv": ["nmcli", "dev", "disconnect", wdev]}), r.success(), Some(cmd_undo(&["nmcli", "dev", "connect", &wdev])), None, None, &r.render())
            }
            a @ ("vpn_up" | "vpn_down") => {
                let conn = s(&input, "connection");
                if conn.is_empty() {
                    return Ok(ToolOutput::error("connection name required (see `nmcli con show`)"));
                }
                let (verb, back) = if a == "vpn_up" { ("up", "down") } else { ("down", "up") };
                let r = exec(&ctx, &["nmcli", "con", verb, "id", conn]).run().await?;
                record_user(&ctx, "wifi", &format!("connection {conn} {verb}"), json!({"kind": "user", "action": "command", "argv": ["nmcli", "con", verb, "id", conn]}), r.success(), Some(cmd_undo(&["nmcli", "con", back, "id", conn])), None, None, &r.render())
            }
            other => Ok(ToolOutput::error(format!("unknown action {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WPCTL: &str = "PipeWire 'pipewire-0' [1.6.2]\n └─ Clients:\n        33. WirePlumber [1.6.2]\n\nAudio\n ├─ Devices:\n │      51. GA106 High Definition Audio Controller [alsa]\n │  \n ├─ Sinks:\n │      35. Zgmicro AUDIO Analog Stereo         [vol: 0.14]\n │  *   87. A05                                 [vol: 0.30 MUTED]\n │      98. GA106 High Definition Audio Controller Digital Stereo (HDMI) [vol: 0.20]\n │  \n ├─ Sources:\n │  *   47. C505 HD Webcam Mono                 [vol: 0.55]\n │  \n └─ Streams:\n\nVideo\n ├─ Devices:\n │      60. C505 [v4l2]\n";

    #[test]
    fn parses_wpctl_sinks_and_sources() {
        let n = parse_wpctl(WPCTL);
        assert_eq!(n.len(), 4, "{n:?}");
        assert_eq!(n[1], WpNode { id: 87, name: "A05".into(), kind: "sink", default: true, volume: Some(0.30) });
        assert_eq!(n[3].kind, "source");
        assert!(n[3].default);
        assert_eq!(find_node(&n, "hdmi", Some("sink")).unwrap().id, 98);
        assert_eq!(find_node(&n, "35", None).unwrap().name, "Zgmicro AUDIO Analog Stereo");
        assert!(find_node(&n, "webcam", Some("sink")).is_none());
    }

    #[test]
    fn parses_bt_devices_and_client_list() {
        let d = parse_bt_devices("Device 3D:4F:95:75:31:5F A05\nDevice 00:11:22:33:44:55 Mouse X\n");
        assert_eq!(d[1], ("00:11:22:33:44:55".into(), "Mouse X".into()));
    }

}
