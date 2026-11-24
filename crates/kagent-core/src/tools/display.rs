//! `display` tool (docs/specs/tools.md section 9): brightness and dark/light color
//! scheme on KDE Plasma (plasma-apply-colorscheme) and GNOME (gsettings), brightness
//! via brightnessctl anywhere or gnome-settings-daemon. Changes are undoable.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::desktop::{cmd_undo, contract, s};
use super::exec::{exec, which};
use super::privop::record_user;
use crate::platform::{desktop as current_desktop, desktop::gtk_theme_for, desktop::gvariant_str, Desktop};
use crate::tool::{CallAssessment, Tool, ToolContext, ToolContract, ToolOutput};

pub struct Display {
    contract: ToolContract,
}

impl Display {
    pub fn new() -> Self {
        Self {
            contract: contract(
                "display",
                "Display: screen brightness (laptops / DDC monitors), dark or light mode, or a named color scheme (KDE: e.g. BreezeDark; GNOME: GTK theme name, e.g. Adwaita, Yaru). Undoable.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["brightness","dark_mode","light_mode","color_scheme"]},
                    "value":{"type":"integer","description":"brightness percent 1-100"},
                    "scheme":{"type":"string","description":"color_scheme: KDE scheme or GNOME GTK theme name"}
                }}),
            ),
        }
    }
}

fn safe_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn kde_scheme(ctx: &ToolContext) -> String {
    exec(ctx, &["kreadconfig6", "--file", "kdeglobals", "--group", "General", "--key", "ColorScheme"]).run().await.map(|r| r.stdout.trim().to_string()).unwrap_or_default()
}

async fn gsetting(ctx: &ToolContext, key: &str) -> String {
    exec(ctx, &["gsettings", "get", "org.gnome.desktop.interface", key]).run().await.map(|r| gvariant_str(&r.stdout)).unwrap_or_default()
}

/// GNOME: color-scheme (+ the GTK3 theme for legacy apps); one undo restores both.
async fn gnome_scheme(ctx: &ToolContext, action: &str, scheme: &str) -> anyhow::Result<ToolOutput> {
    let prev_cs = gsetting(ctx, "color-scheme").await;
    let prev_gtk = gsetting(ctx, "gtk-theme").await;
    let (cs, gtk): (Option<&str>, Option<String>) = match action {
        "dark_mode" => (Some("prefer-dark"), gtk_theme_for(&prev_gtk, true)),
        "light_mode" => (Some("default"), gtk_theme_for(&prev_gtk, false)),
        _ => {
            if !safe_name(scheme) {
                return Ok(ToolOutput::error("scheme: GTK theme name required (e.g. Adwaita, Adwaita-dark, Yaru-dark)"));
            }
            (None, Some(scheme.to_string()))
        }
    };
    let mut cmds: Vec<Vec<String>> = Vec::new();
    if let Some(cs) = cs {
        cmds.push(vec!["gsettings".into(), "set".into(), "org.gnome.desktop.interface".into(), "color-scheme".into(), cs.into()]);
    }
    if let Some(g) = &gtk {
        cmds.push(vec!["gsettings".into(), "set".into(), "org.gnome.desktop.interface".into(), "gtk-theme".into(), g.clone()]);
    }
    let mut ok = true;
    let mut detail = String::new();
    for c in &cmds {
        let r = exec(ctx, c).run().await?;
        ok &= r.success();
        detail.push_str(&r.render());
    }
    // Undo as one shell line over validated names (gsettings values are single words).
    let mut undo_parts = Vec::new();
    if cs.is_some() && safe_name(&prev_cs) {
        undo_parts.push(format!("gsettings set org.gnome.desktop.interface color-scheme {prev_cs}"));
    }
    if gtk.is_some() && safe_name(&prev_gtk) {
        undo_parts.push(format!("gsettings set org.gnome.desktop.interface gtk-theme {prev_gtk}"));
    }
    let undo = (!undo_parts.is_empty()).then(|| cmd_undo(&["sh", "-c", &undo_parts.join(" && ")]));
    let after = format!("{} / {}", cs.unwrap_or(&prev_cs), gtk.as_deref().unwrap_or(&prev_gtk));
    record_user(ctx, "display", &format!("GNOME {action} -> {after}"), json!({"kind":"user","action":"command","argv": cmds}), ok, undo, Some(format!("{prev_cs} / {prev_gtk}")), Some(after.clone()), &detail)
}

/// `(<int32 40>,)` -> 40.
pub fn gvariant_int(out: &str) -> Option<i64> {
    let rest = &out[out.find("int32 ")? + 6..];
    rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect::<String>().parse().ok()
}

const GSD: [&str; 4] = ["--dest", "org.gnome.SettingsDaemon.Power", "--object-path", "/org/gnome/SettingsDaemon/Power"];

async fn gnome_brightness(ctx: &ToolContext, pct: i64) -> anyhow::Result<Option<ToolOutput>> {
    let mut get = vec!["gdbus", "call", "--session"];
    get.extend_from_slice(&GSD);
    get.extend_from_slice(&["--method", "org.freedesktop.DBus.Properties.Get", "org.gnome.SettingsDaemon.Power.Screen", "Brightness"]);
    let prev = exec(ctx, &get).run().await?;
    if !prev.success() {
        return Ok(None); // newer GNOME moved brightness into the shell
    }
    let prev_pct = gvariant_int(&prev.stdout).map(|v| v.to_string()).unwrap_or_default();
    let set_args = |v: &str| -> Vec<String> {
        let mut a: Vec<String> = ["gdbus", "call", "--session"].iter().map(|s| s.to_string()).collect();
        a.extend(GSD.iter().map(|s| s.to_string()));
        a.extend(["--method", "org.freedesktop.DBus.Properties.Set", "org.gnome.SettingsDaemon.Power.Screen", "Brightness"].iter().map(|s| s.to_string()));
        a.push(format!("<int32 {v}>"));
        a
    };
    let r = exec(ctx, &set_args(&pct.to_string())).run().await?;
    let undo_argv = set_args(&prev_pct);
    let undo = (!prev_pct.is_empty()).then(|| json!({"kind": "user", "action": "command", "argv": undo_argv}));
    Ok(Some(record_user(ctx, "display", &format!("brightness {pct}%"), json!({"kind":"user","action":"command","argv": set_args(&pct.to_string())}), r.success(), undo, Some(prev_pct), None, &r.render())?))
}

#[async_trait]
impl Tool for Display {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let summary = format!("display {} {}", s(input, "action"), input.get("value").map(|v| v.to_string()).unwrap_or_default());
        CallAssessment::from_contract(&self.contract, summary.trim().to_string(), format!("display {}", s(input, "action")))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let de = current_desktop();
        match s(&input, "action") {
            "brightness" => {
                let pct = input.get("value").and_then(Value::as_i64).unwrap_or(-1);
                if !(1..=100).contains(&pct) {
                    return Ok(ToolOutput::error("value must be 1-100"));
                }
                if which("brightnessctl") {
                    let prev = exec(&ctx, &["brightnessctl", "-m"]).run().await?.stdout;
                    let prev_pct = prev.split(',').nth(3).unwrap_or("").to_string();
                    let r = exec(&ctx, &["brightnessctl".to_string(), "set".into(), format!("{pct}%")]).run().await?;
                    let undo = (!prev_pct.is_empty()).then(|| cmd_undo(&["brightnessctl", "set", &prev_pct]));
                    return record_user(&ctx, "display", &format!("brightness {pct}%"), json!({"kind":"user","action":"command","argv":["brightnessctl","set",format!("{pct}%")]}), r.success(), undo, Some(prev_pct), None, &r.render());
                }
                if de == Desktop::Gnome {
                    if let Some(out) = gnome_brightness(&ctx, pct).await? {
                        return Ok(out);
                    }
                    return Ok(ToolOutput::error("this GNOME version has no brightness D-Bus API; install brightnessctl"));
                }
                if de == Desktop::Kde {
                    let displays = exec(&ctx, &["qdbus6", "org.kde.ScreenBrightness", "/org/kde/ScreenBrightness", "org.kde.ScreenBrightness.DisplaysDBusNames"]).run().await?;
                    if displays.stdout.trim().is_empty() {
                        return Ok(ToolOutput::error("no display with software brightness control (desktop monitors need DDC/CI support; laptops need a backlight)"));
                    }
                    return Ok(ToolOutput::error(format!("absolute brightness on {} is not supported yet; use the monitor's buttons or install brightnessctl", displays.stdout.trim())));
                }
                Ok(ToolOutput::error("brightness needs brightnessctl on this desktop"))
            }
            a @ ("dark_mode" | "light_mode" | "color_scheme") => match de {
                Desktop::Gnome => gnome_scheme(&ctx, a, s(&input, "scheme")).await,
                Desktop::Kde => {
                    let target = match a {
                        "dark_mode" => "BreezeDark".to_string(),
                        "light_mode" => "BreezeLight".to_string(),
                        _ => s(&input, "scheme").to_string(),
                    };
                    if !safe_name(&target) {
                        return Ok(ToolOutput::error("scheme name required (see `plasma-apply-colorscheme --list-schemes`)"));
                    }
                    let prev = kde_scheme(&ctx).await;
                    let r = exec(&ctx, &["plasma-apply-colorscheme", &target]).run().await?;
                    let undo = (!prev.is_empty() && prev != target).then(|| cmd_undo(&["plasma-apply-colorscheme", &prev]));
                    record_user(&ctx, "display", &format!("color scheme -> {target}"), json!({"kind":"user","action":"command","argv":["plasma-apply-colorscheme", target]}), r.success(), undo, Some(prev), Some(target.clone()), &r.render())
                }
                Desktop::Other => Ok(ToolOutput::error("color schemes are supported on KDE Plasma and GNOME")),
            },
            other => Ok(ToolOutput::error(format!("unknown action {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gvariant_and_names() {
        assert_eq!(gvariant_int("(<int32 40>,)\n"), Some(40));
        assert_eq!(gvariant_int("(<int32 -1>,)"), Some(-1));
        assert_eq!(gvariant_int("()"), None);
        assert!(safe_name("Yaru-dark") && safe_name("BreezeDark") && !safe_name("x; rm") && !safe_name(""));
    }
}
