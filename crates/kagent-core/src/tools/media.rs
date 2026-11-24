//! Media players over MPRIS on the session bus (docs/specs/tools.md section 14):
//! status, play/pause/stop/next/previous, seek, volume, open a file or URL in a player.
//! Uses `busctl --user --json=short`, which ships with systemd. Also used by the voice
//! bridge to pause playback while the user talks.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const ROOT: &str = "org.mpris.MediaPlayer2";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Player {
    pub bus: String,
    pub identity: String,
    pub status: String,
    pub title: String,
    pub artist: String,
    pub url: String,
    pub volume: Option<f64>,
    pub position_us: i64,
    pub length_us: i64,
}

impl Player {
    pub fn render(&self) -> String {
        let mut s = format!("{} [{}]: {}", if self.identity.is_empty() { &self.bus } else { &self.identity }, self.bus.trim_start_matches("org.mpris.MediaPlayer2."), self.status);
        let what = match (self.artist.is_empty(), self.title.is_empty()) {
            (_, true) => String::new(),
            (true, false) => self.title.clone(),
            (false, false) => format!("{} - {}", self.artist, self.title),
        };
        if !what.is_empty() {
            s.push_str(&format!(" - {what}"));
        }
        if self.length_us > 0 {
            s.push_str(&format!(" ({} / {})", mmss(self.position_us), mmss(self.length_us)));
        }
        if let Some(v) = self.volume {
            s.push_str(&format!(", volume {}%", (v * 100.0).round()));
        }
        if !self.url.is_empty() && self.title.is_empty() {
            s.push_str(&format!(" {}", self.url));
        }
        s
    }
}

fn mmss(us: i64) -> String {
    let s = us.max(0) / 1_000_000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// `{"type":"s","data":"Playing"}` -> `"Playing"`.
fn data(v: &Value) -> &Value {
    v.get("data").unwrap_or(&Value::Null)
}

/// MPRIS metadata `a{sv}` (busctl JSON) -> (title, artist, url, length in us).
pub fn parse_metadata(v: &Value) -> (String, String, String, i64) {
    let m = data(v);
    let get = |k: &str| m.get(k).map(data).cloned().unwrap_or(Value::Null);
    let title = get("xesam:title").as_str().unwrap_or("").to_string();
    let artist = match get("xesam:artist") {
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Value::String(s) => s,
        _ => String::new(),
    };
    let url = get("xesam:url").as_str().unwrap_or("").to_string();
    let len = get("mpris:length").as_i64().or_else(|| get("mpris:length").as_u64().map(|u| u as i64)).unwrap_or(0);
    (title, artist, url, len)
}

/// Bus names of all MPRIS players from `busctl --user --json=short list`.
pub fn parse_bus_list(out: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(out).unwrap_or(Value::Null);
    let mut names: Vec<String> = v
        .as_array()
        .map(|a| a.iter().filter_map(|e| e.get("name").and_then(Value::as_str)).filter(|n| n.starts_with("org.mpris.MediaPlayer2.")).map(String::from).collect())
        .unwrap_or_default();
    names.sort();
    names
}

async fn prop(ctx: &ToolContext, bus: &str, iface: &str, name: &str) -> Option<Value> {
    let r = exec(ctx, &["busctl", "--user", "--json=short", "get-property", bus, PATH, iface, name]).run().await.ok()?;
    if !r.success() {
        return None;
    }
    serde_json::from_str(r.stdout.trim()).ok()
}

pub async fn players(ctx: &ToolContext) -> Vec<Player> {
    let Ok(r) = exec(ctx, &["busctl", "--user", "--json=short", "list"]).run().await else { return vec![] };
    let mut out = Vec::new();
    for bus in parse_bus_list(&r.stdout) {
        let mut p = Player { bus: bus.clone(), ..Default::default() };
        p.identity = prop(ctx, &bus, ROOT, "Identity").await.and_then(|v| data(&v).as_str().map(String::from)).unwrap_or_default();
        p.status = prop(ctx, &bus, PLAYER, "PlaybackStatus").await.and_then(|v| data(&v).as_str().map(String::from)).unwrap_or_else(|| "Unknown".into());
        if let Some(m) = prop(ctx, &bus, PLAYER, "Metadata").await {
            (p.title, p.artist, p.url, p.length_us) = parse_metadata(&m);
        }
        p.volume = prop(ctx, &bus, PLAYER, "Volume").await.and_then(|v| data(&v).as_f64());
        p.position_us = prop(ctx, &bus, PLAYER, "Position").await.and_then(|v| data(&v).as_i64()).unwrap_or(0);
        out.push(p);
    }
    out
}

/// Player matching `needle` (bus name or identity, case-insensitive); without one the
/// playing player, then a paused one, then the first.
pub fn pick<'a>(players: &'a [Player], needle: &str) -> Option<&'a Player> {
    let n = needle.trim().to_lowercase();
    if !n.is_empty() {
        return players.iter().find(|p| p.identity.to_lowercase().contains(&n) || p.bus.to_lowercase().contains(&n));
    }
    players
        .iter()
        .find(|p| p.status == "Playing")
        .or_else(|| players.iter().find(|p| p.status == "Paused" && !p.title.is_empty()))
        .or_else(|| players.iter().find(|p| p.status == "Paused"))
        .or_else(|| players.first())
}

async fn call(ctx: &ToolContext, bus: &str, method: &str, sig_args: &[&str]) -> anyhow::Result<super::exec::ExecResult> {
    // `--` so negative seek offsets are not read as options.
    let mut argv = vec!["busctl", "--user", "--", "call", bus, PATH, PLAYER, method];
    argv.extend_from_slice(sig_args);
    exec(ctx, &argv).run().await
}

/// Pause every playing player; returns their bus names so they can be resumed.
pub async fn pause_playing(ctx: &ToolContext) -> Vec<String> {
    let mut paused = Vec::new();
    for p in players(ctx).await.into_iter().filter(|p| p.status == "Playing") {
        if call(ctx, &p.bus, "Pause", &[]).await.is_ok_and(|r| r.success()) {
            paused.push(p.bus);
        }
    }
    paused
}

pub async fn resume(ctx: &ToolContext, buses: &[String]) {
    for b in buses {
        let _ = call(ctx, b, "Play", &[]).await;
    }
}

/// Absolute path or URL -> URI for `OpenUri` (percent-encodes paths).
pub fn to_uri(target: &str) -> String {
    if target.contains("://") {
        return target.to_string();
    }
    let mut s = String::from("file://");
    for b in target.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

pub struct Media {
    contract: ToolContract,
}

impl Media {
    pub fn new() -> Self {
        let mut contract = ToolContract::read_only(
            "media",
            "Control media players (VLC, Haruna, Spotify, browsers... over MPRIS). status lists players and what is playing. Without `player` the playing one is used. seek: relative seconds (negative = back). volume: 0-100. open: play a file path or URL in a running player (to start a player with a file use `open` with `app`).",
            json!({"type":"object","required":["action"],"properties":{
                "action":{"type":"string","enum":["status","play","pause","toggle","stop","next","previous","seek","volume","open"]},
                "player":{"type":"string","description":"part of the player name, e.g. vlc, spotify, brave"},
                "seconds":{"type":"number"},
                "volume":{"type":"number","minimum":0,"maximum":100},
                "target":{"type":"string","description":"open: absolute file path or URL"}
            }}),
        )
        .mutating(SideEffect::System, Risk::Low);
        // Playback control is harmless and reversible: no approval card (still denied in
        // read-only mode).
        contract.needs_approval = false;
        Self { contract }
    }
}

#[async_trait]
impl Tool for Media {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let player = input.get("player").and_then(Value::as_str).unwrap_or("");
        let a = CallAssessment::from_contract(&self.contract, format!("media {action} {player}").trim().to_string(), format!("media {action}"));
        if action == "status" { a.as_read_only() } else { a }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let needle = input.get("player").and_then(Value::as_str).unwrap_or("");
        let all = players(&ctx).await;
        if action == "status" {
            if all.is_empty() {
                return Ok(ToolOutput::ok("no media players running"));
            }
            return Ok(ToolOutput::ok(all.iter().map(Player::render).collect::<Vec<_>>().join("\n")));
        }
        let Some(p) = pick(&all, needle) else {
            let names: Vec<&str> = all.iter().map(|p| p.identity.as_str()).collect();
            return Ok(ToolOutput::error(if all.is_empty() {
                "no media players running; start one with `open` (target + app)".to_string()
            } else {
                format!("no player matches '{needle}'; running: {}", names.join(", "))
            }));
        };
        let r = match action {
            "play" => call(&ctx, &p.bus, "Play", &[]).await?,
            "pause" => call(&ctx, &p.bus, "Pause", &[]).await?,
            "toggle" => call(&ctx, &p.bus, "PlayPause", &[]).await?,
            "stop" => call(&ctx, &p.bus, "Stop", &[]).await?,
            "next" => call(&ctx, &p.bus, "Next", &[]).await?,
            "previous" => call(&ctx, &p.bus, "Previous", &[]).await?,
            "seek" => {
                let us = (input.get("seconds").and_then(Value::as_f64).unwrap_or(0.0) * 1e6) as i64;
                call(&ctx, &p.bus, "Seek", &["x", &us.to_string()]).await?
            }
            "volume" => {
                let v = (input.get("volume").and_then(Value::as_f64).unwrap_or(50.0).clamp(0.0, 100.0) / 100.0).to_string();
                exec(&ctx, &["busctl", "--user", "set-property", &p.bus, PATH, PLAYER, "Volume", "d", &v]).run().await?
            }
            "open" => {
                let t = input.get("target").and_then(Value::as_str).unwrap_or("").trim();
                if t.is_empty() || t.starts_with('-') || !(t.starts_with('/') || t.contains("://")) {
                    return Ok(ToolOutput::error("target must be an absolute path or a URL"));
                }
                call(&ctx, &p.bus, "OpenUri", &["s", &to_uri(t)]).await?
            }
            _ => return Ok(ToolOutput::error(format!("unknown action {action}"))),
        };
        if !r.success() {
            return Ok(ToolOutput::error(format!("{} refused {action}: {}", p.identity, r.render())));
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let now = players(&ctx).await.into_iter().find(|x| x.bus == p.bus).unwrap_or_else(|| p.clone());
        Ok(ToolOutput::ok(format!("{action} sent. Now: {}", now.render())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_busctl_output() {
        let list = r#"[{"name":"org.freedesktop.DBus"},{"name":"org.mpris.MediaPlayer2.vlc"},{"name":"org.mpris.MediaPlayer2.brave.instance1"}]"#;
        assert_eq!(parse_bus_list(list), vec!["org.mpris.MediaPlayer2.brave.instance1", "org.mpris.MediaPlayer2.vlc"]);
        let m: Value = serde_json::from_str(r#"{"type":"a{sv}","data":{"xesam:title":{"type":"s","data":"Song"},"xesam:artist":{"type":"as","data":["A","B"]},"mpris:length":{"type":"x","data":185000000},"xesam:url":{"type":"s","data":"file:///m.mp3"}}}"#).unwrap();
        assert_eq!(parse_metadata(&m), ("Song".into(), "A, B".into(), "file:///m.mp3".into(), 185_000_000));
    }

    #[test]
    fn picks_playing_then_named() {
        let mk = |bus: &str, id: &str, st: &str| Player { bus: bus.into(), identity: id.into(), status: st.into(), ..Default::default() };
        let ps = vec![mk("org.mpris.MediaPlayer2.brave", "Brave", "Stopped"), mk("org.mpris.MediaPlayer2.vlc", "VLC media player", "Playing")];
        assert_eq!(pick(&ps, "").unwrap().identity, "VLC media player");
        assert_eq!(pick(&ps, "BRAVE").unwrap().identity, "Brave");
        assert!(pick(&ps, "spotify").is_none());
        let p = Player { identity: "VLC".into(), status: "Playing".into(), title: "T".into(), artist: "A".into(), length_us: 125_000_000, position_us: 5_000_000, volume: Some(0.5), ..Default::default() };
        assert_eq!(p.render(), "VLC []: Playing - A - T (0:05 / 2:05), volume 50%");
    }

    #[test]
    fn uri_encoding() {
        assert_eq!(to_uri("/home/u/My Film (1).mkv"), "file:///home/u/My%20Film%20%281%29.mkv");
        assert_eq!(to_uri("https://x.y/a b"), "https://x.y/a b");
    }
}
